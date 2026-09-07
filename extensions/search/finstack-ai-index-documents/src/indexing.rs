use std::sync::Arc;

use finstack_ai_embeddings::vector::truncate_to_bytes;
use finstack_ai_kernel::{ArtifactRef, Digest};
use finstack_ai_runtime::artifact::{ArtifactError, ArtifactScope};
use finstack_ai_search_core::{SearchError, configuration_digest};
use finstack_ai_tools_document::parser::{DocumentFormat, DocumentLimits, ParsedDocument, parse};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{DocumentChunk, DocumentSearchSource, chunk_document};

/// Explicit host-authorized reconstruction input. Agent-driven ingestion passes
/// only an artifact; the indexing tool derives this read scope from committed context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentInput {
    /// Exact artifact-store authority, not inferred from index metadata.
    pub artifact_scope: ArtifactScope,
    /// Immutable content/reference integrity binding.
    pub artifact: ArtifactRef,
}

/// Document extraction status, preserved even when no searchable text exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    /// Parsed text was fully indexed under configured limits.
    Indexed,
    /// OCR is required for some/all pages; no OCR is performed by this indexer.
    OcrRequired,
    /// Parser output was truncated at its explicit byte limit.
    Truncated,
}

/// Stable idempotent indexing receipt. Repeated indexing of the same artifact
/// and chunker returns the same receipt, without adding rows or embeddings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentIndexReport {
    /// Exact indexed artifact.
    pub artifact: ArtifactRef,
    /// Versioned extraction and chunking configuration fingerprint.
    pub chunker_digest: Digest,
    /// Exact parsed Markdown fingerprint, used to verify reconstruction.
    pub parsed_digest: Digest,
    /// Number of searchable chunks (may be zero for a scanned document).
    pub chunks: usize,
    /// Indexed, OCR-required, or truncated extraction.
    pub status: DocumentStatus,
    /// Whether any source content needs OCR (never silently dropped).
    pub requires_ocr: bool,
    /// Whether parsed Markdown was truncated.
    pub truncated: bool,
    /// Page count when the existing parser can determine it.
    pub page_count: Option<u32>,
}

/// Explicit bounded source reconciliation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileReport {
    /// Catalog entries checked against the artifact system of record.
    pub examined: usize,
    /// Missing/deleted artifact entries removed with all derived chunks/vectors.
    pub removed: usize,
    /// Entries whose source was unavailable; retained for a future retry.
    pub unavailable: usize,
    /// Whether the catalog had more entries than the requested bounded batch.
    pub truncated: bool,
    /// Last examined document key; pass as `after` to continue a bounded scan.
    pub next_after: Option<String>,
}

impl DocumentSearchSource {
    /// Read an existing receipt for an exact live artifact and current processing
    /// configuration, without indexing or embedding. Used by hosts to verify that
    /// an agent's explicit indexing tool effect actually completed.
    ///
    /// # Errors
    /// Rejects unauthorized/missing artifacts or unavailable index storage.
    pub async fn indexed_document(
        &self,
        input: DocumentInput,
    ) -> Result<Option<DocumentIndexReport>, SearchError> {
        self.validate_input(&input)?;
        self.artifacts
            .get(input.artifact_scope.clone(), input.artifact.clone())
            .await
            .map_err(artifact_error)?;
        let key = document_key(&self.scope_digest, &input, self.chunker_digest)?;
        self.database.call(move |db| receipt(db, &key)).await
    }

    /// Idempotently index an explicitly authorized artifact. This is the trusted
    /// host/rebuild entrypoint used by [`crate::DocumentIndexToolset`]; agent and
    /// CLI ingestion should invoke that tool through a committed SDK run.
    ///
    /// No embeddings are requested here. [`Self::reconcile_embeddings`] performs
    /// separately bounded best-effort derived-index maintenance when opted in.
    ///
    /// # Errors
    /// Rejects unauthorized/missing artifacts, parse failures, unsupported schemas,
    /// or exhausted capacities. SQL commits the receipt and all chunks atomically.
    pub async fn index_document(
        &self,
        input: DocumentInput,
    ) -> Result<DocumentIndexReport, SearchError> {
        self.validate_input(&input)?;
        let bytes = self
            .artifacts
            .get(input.artifact_scope.clone(), input.artifact.clone())
            .await
            .map_err(artifact_error)?;
        let artifact_key =
            configuration_digest("indexed-artifact-reference", &input.artifact)?.to_hex();
        let key = document_key(&self.scope_digest, &input, self.chunker_digest)?;
        let lookup = key.clone();
        if let Some(report) = self.database.call(move |db| receipt(db, &lookup)).await? {
            return Ok(report);
        }
        let config = self.config.clone();
        let scope_digest = self.scope_digest.clone();
        let chunker_digest = self.chunker_digest;
        self.database
            .call(move |db| {
                if let Some(report) = receipt(db, &key)? {
                    return Ok(report);
                }
                let parsed = extract(
                    &bytes,
                    input.artifact.blob().media_type(),
                    &config.parse_limits,
                )?;
                let chunks = chunk_document(&parsed.markdown, config.chunker, config.max_chunks)?;
                let report = DocumentIndexReport {
                    artifact: input.artifact.clone(),
                    chunker_digest,
                    parsed_digest: configuration_digest("document-parsed-text", &parsed.markdown)?,
                    chunks: chunks.len(),
                    status: if parsed.requires_ocr {
                        DocumentStatus::OcrRequired
                    } else if parsed.truncated {
                        DocumentStatus::Truncated
                    } else {
                        DocumentStatus::Indexed
                    },
                    requires_ocr: parsed.requires_ocr,
                    truncated: parsed.truncated,
                    page_count: parsed.page_count,
                };
                write_document(
                    db,
                    &key,
                    &scope_digest,
                    &artifact_key,
                    &input,
                    &report,
                    &chunks,
                    config.max_chunks,
                    config.max_documents,
                )?;
                Ok(report)
            })
            .await
    }

    pub(crate) fn validate_input(&self, input: &DocumentInput) -> Result<(), SearchError> {
        if input.artifact_scope.tenant_scope != self.config.scope.tenant
            || input.artifact_scope.digest().map_err(artifact_error)?
                != input.artifact.scope_digest()
        {
            return Err(SearchError::SearchScopeDenied);
        }
        if input.artifact.blob().length() > self.config.parse_limits.max_input_bytes {
            return Err(SearchError::SearchCapacityExceeded {
                resource: "document_input_bytes".into(),
            });
        }
        let encoded =
            serde_json::to_vec(input).map_err(|_| SearchError::invalid("document_input"))?;
        if encoded.len() > 8192 {
            return Err(SearchError::invalid("document_reference_bytes"));
        }
        Ok(())
    }

    /// Read the explicit catalog across processing versions for this scope. Callers
    /// must retain these references or own another authorized artifact enumeration;
    /// this API introduces no global discovery on the artifact port.
    ///
    /// # Errors
    /// Rejects bad cursors/bounds or unavailable storage.
    pub async fn inputs(
        &self,
        after: Option<String>,
        limit: usize,
    ) -> Result<Vec<(String, DocumentInput)>, SearchError> {
        if limit == 0 || limit > 256 || after.as_ref().is_some_and(|cursor| cursor.len() > 256) {
            return Err(SearchError::invalid("document_catalog_page"));
        }
        let scope = self.scope_digest.clone();
        self.database.call(move |db| {
            let mut statement = db.prepare("SELECT document_key,input_json FROM documents WHERE scope_digest=?1 AND document_key>?2 ORDER BY document_key LIMIT ?3").map_err(sql_error)?;
            let rows = statement.query_map(params![scope,after.unwrap_or_default(),limit], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).map_err(sql_error)?;
            rows.map(|row| { let (key,json) = row.map_err(sql_error)?; Ok((key, serde_json::from_str(&json).map_err(|_| SearchError::invalid("document_catalog"))?)) }).collect()
        }).await
    }

    /// Remove missing/deleted artifacts from all derived tables; unavailable
    /// sources remain visible as unavailable rather than being forgotten.
    ///
    /// # Errors
    /// Rejects invalid bounded pages or unavailable index storage.
    pub async fn reconcile_sources(
        &self,
        after: Option<String>,
        limit: usize,
    ) -> Result<ReconcileReport, SearchError> {
        let inputs = self.inputs(after, limit).await?;
        let mut report = ReconcileReport {
            examined: 0,
            removed: 0,
            unavailable: 0,
            truncated: inputs.len() == limit,
            next_after: None,
        };
        for (key, input) in inputs {
            report.examined += 1;
            report.next_after = Some(key.clone());
            self.validate_input(&input)?;
            match self
                .artifacts
                .get(input.artifact_scope, input.artifact)
                .await
            {
                Ok(_) => {}
                Err(ArtifactError::NotFound) => {
                    self.remove_document(key).await?;
                    report.removed += 1;
                }
                Err(ArtifactError::ScopeMismatch { .. } | ArtifactError::Integrity { .. }) => {
                    return Err(SearchError::SearchScopeDenied);
                }
                Err(_) => report.unavailable += 1,
            }
        }
        Ok(report)
    }

    pub(crate) async fn remove_document(&self, key: String) -> Result<(), SearchError> {
        let scope = self.scope_digest.clone();
        self.database
            .call(move |db| {
                db.execute(
                    "DELETE FROM documents WHERE document_key=?1 AND scope_digest=?2",
                    params![key, scope],
                )
                .map_err(sql_error)?;
                Ok(())
            })
            .await
    }

    /// Embed a bounded pending batch in the configured space. An absent embedder
    /// is unsupported; source changes/deletion cause stale writes to be ignored.
    /// Lexical indexing remains usable if this maintenance call fails.
    ///
    /// # Errors
    /// Rejects missing/invalid embedding output, dimensionality drift, byte/space
    /// capacity exhaustion, unavailable artifacts, or storage errors.
    pub async fn reconcile_embeddings(&self, limit: usize) -> Result<usize, SearchError> {
        if limit == 0 || limit > 64 {
            return Err(SearchError::invalid("document_embedding_batch"));
        }
        let embedder = self
            .embedder
            .as_ref()
            .ok_or(SearchError::SearchUnsupported)?;
        let descriptor = embedder.descriptor();
        let space = Arc::clone(&descriptor.embedder_id);
        let scope = self.scope_digest.clone();
        let chunker = self.chunker_digest.to_hex();
        let pending_space = Arc::clone(&space);
        let pending = self
            .database
            .call(move |db| pending_vectors(db, &scope, &chunker, &pending_space, limit))
            .await?;
        if pending.is_empty() {
            return Ok(0);
        }
        let mut live = Vec::new();
        for source in pending {
            self.validate_input(&source.input)?;
            match self
                .artifacts
                .get(
                    source.input.artifact_scope.clone(),
                    source.input.artifact.clone(),
                )
                .await
            {
                Ok(_) => live.push(source),
                Err(ArtifactError::NotFound) => self.remove_document(source.document_key).await?,
                Err(error) => return Err(artifact_error(error)),
            }
        }
        if live.is_empty() {
            return Ok(0);
        }
        let texts = live
            .iter()
            .map(|source| Arc::from(truncate_to_bytes(&source.text, descriptor.max_input_bytes)))
            .collect();
        let vectors = embedder
            .embed(texts)
            .await
            .map_err(|_| SearchError::SearchUnavailable)?;
        if vectors.len() != live.len()
            || vectors
                .iter()
                .any(|v| v.dimensions() != descriptor.dimensions)
        {
            return Err(SearchError::invalid("document_embedding_output"));
        }
        let max_bytes = self.config.limits.max_embedding_bytes;
        self.database.call(move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(sql_error)?;
            let existing: Option<usize> = tx.query_row("SELECT dimensions FROM embedding_spaces WHERE space=?1",[space.as_ref()],|r|r.get(0)).optional().map_err(sql_error)?;
            if existing.is_some_and(|dims| dims != descriptor.dimensions) { return Err(SearchError::invalid("document_embedding_dimensions")); }
            if existing.is_none() {
                let spaces: usize = tx.query_row("SELECT count(*) FROM embedding_spaces",[],|r|r.get(0)).map_err(sql_error)?;
                if spaces >= 4 { return Err(SearchError::SearchCapacityExceeded { resource: "document_embedding_spaces".into() }); }
                tx.execute("INSERT INTO embedding_spaces(space,dimensions) VALUES(?1,?2)", params![space.as_ref(),descriptor.dimensions]).map_err(sql_error)?;
            }
            let mut bytes: u64 = tx.query_row("SELECT coalesce(sum(length(vector)),0) FROM chunk_vectors",[],|r|r.get(0)).map_err(sql_error)?;
            let mut written = 0;
            for (source, vector) in live.into_iter().zip(vectors) {
                let current: Option<String> = tx.query_row("SELECT content_digest FROM chunks WHERE rowid=?1 AND document_key=?2",params![source.rowid,source.document_key],|r|r.get(0)).optional().map_err(sql_error)?;
                if current.as_deref() != Some(source.digest.as_str()) { continue; }
                let encoded: Vec<u8> = vector.unit_normalized().as_slice().iter().flat_map(|value| value.to_le_bytes()).collect();
                let previous: u64 = tx.query_row("SELECT coalesce((SELECT length(vector) FROM chunk_vectors WHERE chunk_id=?1 AND space=?2),0)",params![source.rowid,space.as_ref()],|r|r.get(0)).map_err(sql_error)?;
                bytes = bytes.saturating_sub(previous).saturating_add(encoded.len() as u64);
                if bytes > max_bytes { return Err(SearchError::SearchCapacityExceeded { resource: "document_embedding_bytes".into() }); }
                tx.execute("INSERT INTO chunk_vectors(chunk_id,space,dimensions,source_digest,vector) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(chunk_id,space) DO UPDATE SET dimensions=excluded.dimensions,source_digest=excluded.source_digest,vector=excluded.vector",params![source.rowid,space.as_ref(),descriptor.dimensions,source.digest,encoded]).map_err(sql_error)?;
                written += 1;
            }
            tx.commit().map_err(sql_error)?;
            Ok(written)
        }).await
    }
}

pub(crate) fn document_key(
    scope: &str,
    input: &DocumentInput,
    chunker: Digest,
) -> Result<String, SearchError> {
    Ok(configuration_digest("indexed-document", &(scope, &input.artifact, chunker))?.to_hex())
}

fn receipt(db: &Connection, key: &str) -> Result<Option<DocumentIndexReport>, SearchError> {
    let encoded: Option<String> = db
        .query_row(
            "SELECT report_json FROM documents WHERE document_key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    encoded
        .map(|value| {
            serde_json::from_str(&value).map_err(|_| SearchError::invalid("document_receipt"))
        })
        .transpose()
}

#[allow(clippy::too_many_arguments)] // Atomic write receives the already validated immutable inputs.
fn write_document(
    db: &mut Connection,
    key: &str,
    scope: &str,
    artifact_key: &str,
    input: &DocumentInput,
    report: &DocumentIndexReport,
    chunks: &[DocumentChunk],
    max_chunks: usize,
    max_documents: usize,
) -> Result<(), SearchError> {
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    // Another index handle/process may have committed while this one parsed.
    if receipt(&tx, key)?.is_some() {
        return Ok(());
    }
    let documents: usize = tx
        .query_row(
            "SELECT count(*) FROM documents WHERE scope_digest=?1",
            [scope],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let total: usize = tx.query_row("SELECT count(*) FROM chunks c JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1",[scope],|r|r.get(0)).map_err(sql_error)?;
    if documents >= max_documents || total.saturating_add(chunks.len()) > max_chunks {
        return Err(SearchError::SearchCapacityExceeded {
            resource: "document_index_rows".into(),
        });
    }
    let input_json =
        serde_json::to_string(input).map_err(|_| SearchError::invalid("document_input"))?;
    let report_json =
        serde_json::to_string(report).map_err(|_| SearchError::invalid("document_receipt"))?;
    tx.execute(
        "INSERT INTO documents VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            key,
            scope,
            artifact_key,
            report.chunker_digest.to_hex(),
            input_json,
            report_json
        ],
    )
    .map_err(sql_error)?;
    for chunk in chunks {
        tx.execute("INSERT INTO chunks(document_key,ordinal,start_char,end_char,heading,text,content_digest) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![key,chunk.ordinal,chunk.start_char,chunk.end_char,chunk.heading.as_deref(),chunk.text.as_ref(),chunk.content_digest.to_hex()]).map_err(sql_error)?;
    }
    tx.commit().map_err(sql_error)
}

fn extract(
    bytes: &[u8],
    media_type: &str,
    limits: &DocumentLimits,
) -> Result<ParsedDocument, SearchError> {
    if bytes.len() as u64 > limits.max_input_bytes {
        return Err(SearchError::SearchCapacityExceeded {
            resource: "document_input_bytes".into(),
        });
    }
    if media_type == "text/plain" || media_type == "text/markdown" {
        let text = std::str::from_utf8(bytes).map_err(|_| SearchError::invalid("document_utf8"))?;
        let markdown = truncate_to_bytes(
            text,
            usize::try_from(limits.max_output_bytes)
                .map_err(|_| SearchError::invalid("document_output_limit"))?,
        )
        .to_owned();
        return Ok(ParsedDocument {
            truncated: markdown.len() != text.len(),
            markdown,
            format: DocumentFormat::Unknown,
            page_count: None,
            classification: None,
            requires_ocr: false,
        });
    }
    parse(bytes, Some(media_type), limits)
        .map_err(|_| SearchError::invalid("document_parse_failed"))
}

struct VectorSource {
    rowid: i64,
    document_key: String,
    text: String,
    digest: String,
    input: DocumentInput,
}

fn pending_vectors(
    db: &Connection,
    scope: &str,
    chunker: &str,
    space: &str,
    limit: usize,
) -> Result<Vec<VectorSource>, SearchError> {
    let mut statement = db.prepare("SELECT c.rowid,c.document_key,c.text,c.content_digest,d.input_json FROM chunks c JOIN documents d ON d.document_key=c.document_key LEFT JOIN chunk_vectors v ON v.chunk_id=c.rowid AND v.space=?3 AND v.source_digest=c.content_digest WHERE d.scope_digest=?1 AND d.chunker_digest=?2 AND v.chunk_id IS NULL ORDER BY c.rowid LIMIT ?4").map_err(sql_error)?;
    let rows = statement
        .query_map(params![scope, chunker, space, limit], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(sql_error)?;
    rows.map(|row| {
        let (rowid, document_key, text, digest, input) = row.map_err(sql_error)?;
        Ok(VectorSource {
            rowid,
            document_key,
            text,
            digest,
            input: serde_json::from_str(&input)
                .map_err(|_| SearchError::invalid("document_catalog"))?,
        })
    })
    .collect()
}

#[allow(clippy::needless_pass_by_value)] // map_err owns the diagnostic, which is deliberately discarded.
pub(crate) fn sql_error(_error: rusqlite::Error) -> SearchError {
    SearchError::SearchUnavailable
}

#[allow(clippy::needless_pass_by_value)] // Boundary conversion discards source diagnostics.
pub(crate) fn artifact_error(error: ArtifactError) -> SearchError {
    match error {
        ArtifactError::ScopeMismatch { .. } | ArtifactError::Integrity { .. } => {
            SearchError::SearchScopeDenied
        }
        _ => SearchError::SearchUnavailable,
    }
}
