//! Bounded lexical and exact-vector retrieval; artifact liveness is checked
//! before any preview is returned. Deleted artifacts are reconciled out.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::sync::Arc;

use finstack_ai_embeddings::vector::{EmbeddingVector, similarity_score, truncate_to_bytes};
use finstack_ai_kernel::Digest;
use finstack_ai_runtime::artifact::ArtifactError;
use finstack_ai_search_core::{
    LexicalKind, SearchError, SearchHit, SearchProvenance, SearchQuery, SearchStrategy, SourceRef,
    SourceResult, SourceStatus, preview,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::indexing::sql_error;
use crate::{DocumentIndexReport, DocumentInput, DocumentSearchSource};

struct Candidate {
    document_key: String,
    input: DocumentInput,
    ordinal: u32,
    start: u32,
    end: u32,
    heading: Option<String>,
    text: String,
    digest: Digest,
    score: u32,
}

struct Retrieved {
    candidates: Vec<Candidate>,
    examined: usize,
    total_chunks: usize,
    indexed_vectors: usize,
    incomplete_documents: bool,
    old_chunkers: bool,
    truncated: bool,
}

impl DocumentSearchSource {
    pub(crate) async fn retrieve(
        &self,
        query: SearchQuery,
        limit: usize,
    ) -> Result<SourceResult, SearchError> {
        let vector = self.query_vector(&query).await?;
        let scope = self.scope_digest.clone();
        let chunker = self.chunker_digest.to_hex();
        let limits = self.config.limits;
        let selected = query.clone();
        let retrieved = self
            .database
            .call(move |db| retrieve(db, &scope, &chunker, &selected, vector.as_ref(), limits))
            .await?;
        let mut result = SourceResult::completed(Vec::new(), retrieved.examined as u64);
        if retrieved.truncated {
            result.truncate("document_scan_or_result_limit");
        }
        if retrieved.incomplete_documents {
            result.truncate("document_extraction_incomplete");
        }
        if retrieved.old_chunkers {
            result.truncate("document_chunker_reindex_pending");
        }
        if matches!(query.strategy, SearchStrategy::Semantic { .. })
            && retrieved.indexed_vectors < retrieved.total_chunks
        {
            result.truncate("document_embeddings_pending");
            if retrieved.indexed_vectors == 0 {
                result.status = SourceStatus::Unavailable;
            }
        }
        if retrieved.total_chunks == 0 && (retrieved.incomplete_documents || retrieved.old_chunkers)
        {
            result.status = SourceStatus::Unavailable;
        }
        let mut checked = BTreeMap::new();
        let mut unavailable = 0;
        for candidate in retrieved.candidates {
            self.validate_input(&candidate.input)?;
            let alive = if let Some(alive) = checked.get(&candidate.document_key) {
                *alive
            } else {
                let alive = match self
                    .artifacts
                    .get(
                        candidate.input.artifact_scope.clone(),
                        candidate.input.artifact.clone(),
                    )
                    .await
                {
                    Ok(_) => true,
                    Err(ArtifactError::NotFound) => {
                        self.remove_document(candidate.document_key.clone()).await?;
                        result.truncate("document_sources_removed");
                        false
                    }
                    Err(ArtifactError::ScopeMismatch { .. } | ArtifactError::Integrity { .. }) => {
                        return Err(SearchError::SearchScopeDenied);
                    }
                    Err(_) => {
                        unavailable += 1;
                        result.truncate("document_sources_unavailable");
                        false
                    }
                };
                checked.insert(candidate.document_key.clone(), alive);
                alive
            };
            if !alive {
                continue;
            }
            if result.hits.len() == limit {
                result.truncate("document_result_limit");
                break;
            }
            result.hits.push(self.hit(candidate)?);
        }
        if unavailable > 0 && result.hits.is_empty() {
            result.status = SourceStatus::Unavailable;
        }
        Ok(result)
    }

    pub(crate) async fn exact_evidence(
        &self,
        reference: SourceRef,
    ) -> Result<Option<finstack_ai_search_core::SearchEvidence>, SearchError> {
        let SourceRef::ArtifactChunk {
            artifact,
            ordinal,
            chunker,
        } = reference
        else {
            return Err(SearchError::SearchUnsupported);
        };
        if chunker != self.chunker_digest {
            return Ok(None);
        }
        let artifact_key =
            finstack_ai_search_core::configuration_digest("indexed-artifact-reference", &artifact)?
                .to_hex();
        let scope = self.scope_digest.clone();
        let candidate=self.database.call(move |db| {
            let id:Option<i64>=db.query_row("SELECT c.rowid FROM chunks c JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1 AND d.artifact_key=?2 AND d.chunker_digest=?3 AND c.ordinal=?4", params![scope,artifact_key,chunker.to_hex(),ordinal],|r|r.get(0)).optional().map_err(sql_error)?;
            id.map(|id|load_candidate(db,id,0)).transpose()
        }).await?;
        let Some(candidate) = candidate else {
            return Ok(None);
        };
        self.validate_input(&candidate.input)?;
        match self
            .artifacts
            .get(
                candidate.input.artifact_scope.clone(),
                candidate.input.artifact.clone(),
            )
            .await
        {
            Ok(_) => {}
            Err(ArtifactError::NotFound) => {
                self.remove_document(candidate.document_key).await?;
                return Ok(None);
            }
            Err(error) => return Err(crate::indexing::artifact_error(error)),
        }
        let text = Arc::from(candidate.text.as_str());
        let evidence = finstack_ai_search_core::SearchEvidence {
            hit: self.hit(candidate)?,
            text,
            complete: true,
        };
        evidence.validate(&self.config.limits)?;
        Ok(Some(evidence))
    }

    async fn query_vector(
        &self,
        query: &SearchQuery,
    ) -> Result<Option<EmbeddingVector>, SearchError> {
        Ok(match &query.strategy {
            SearchStrategy::Semantic { .. } => {
                let embedder = self
                    .embedder
                    .as_ref()
                    .ok_or(SearchError::SearchUnsupported)?;
                let descriptor = embedder.descriptor();
                let vectors = embedder
                    .embed(vec![Arc::from(truncate_to_bytes(
                        &query.text,
                        descriptor.max_input_bytes,
                    ))])
                    .await
                    .map_err(|_| SearchError::SearchUnavailable)?;
                if vectors.len() != 1 {
                    return Err(SearchError::invalid("document_query_embedding"));
                }
                let vector = vectors
                    .into_iter()
                    .next()
                    .ok_or_else(|| SearchError::invalid("document_query_embedding"))?;
                if vector.dimensions() != descriptor.dimensions {
                    return Err(SearchError::invalid("document_embedding_dimensions"));
                }
                Some(vector.unit_normalized())
            }
            _ => None,
        })
    }

    fn hit(&self, candidate: Candidate) -> Result<SearchHit, SearchError> {
        let mut locators: Vec<Arc<str>> = vec![
            format!(
                "parsed_markdown_characters:{}..{}",
                candidate.start, candidate.end
            )
            .into(),
        ];
        if let Some(heading) = candidate.heading {
            locators.push(format!("heading:{}", truncate_to_bytes(&heading, 1016)).into());
        }
        if let Some(name) = candidate.input.artifact.blob().name() {
            locators.push(format!("artifact:{}", truncate_to_bytes(name, 1015)).into());
        }
        let hit = SearchHit {
            entity_label: None,
            source: Arc::clone(&self.config.source_id),
            reference: SourceRef::ArtifactChunk {
                artifact: Box::new(candidate.input.artifact),
                ordinal: candidate.ordinal,
                chunker: self.chunker_digest,
            },
            score: candidate.score,
            preview: preview(&candidate.text, self.config.limits.max_preview_chars),
            sensitivity: candidate.input.artifact_scope.sensitivity,
            provenance: SearchProvenance {
                scope: self.config.scope.clone(),
                content_digest: candidate.digest,
                locators,
                citations: Vec::new(),
            },
        };
        hit.validate(&self.config.limits)?;
        Ok(hit)
    }
}

fn retrieve(
    db: &Connection,
    scope: &str,
    chunker: &str,
    query: &SearchQuery,
    vector: Option<&EmbeddingVector>,
    limits: finstack_ai_search_core::SearchLimits,
) -> Result<Retrieved, SearchError> {
    let total_chunks: usize = db.query_row("SELECT count(*) FROM chunks c JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1 AND d.chunker_digest=?2",params![scope,chunker],|r|r.get(0)).map_err(sql_error)?;
    let incomplete: usize = db.query_row("SELECT count(*) FROM documents WHERE scope_digest=?1 AND chunker_digest=?2 AND (json_extract(report_json,'$.requires_ocr')=1 OR json_extract(report_json,'$.truncated')=1)",params![scope,chunker],|r|r.get(0)).map_err(sql_error)?;
    let old: usize = db.query_row("SELECT count(*) FROM documents old WHERE old.scope_digest=?1 AND old.chunker_digest<>?2 AND NOT EXISTS (SELECT 1 FROM documents current WHERE current.scope_digest=old.scope_digest AND current.artifact_key=old.artifact_key AND current.chunker_digest=?2)",params![scope,chunker],|r|r.get(0)).map_err(sql_error)?;
    let mut result = Retrieved {
        candidates: Vec::new(),
        examined: 0,
        total_chunks,
        indexed_vectors: 0,
        incomplete_documents: incomplete > 0,
        old_chunkers: old > 0,
        truncated: false,
    };
    let retention = limits.max_results.min(limits.max_scan_records);
    let ranked = match &query.strategy {
        SearchStrategy::Semantic { space } => {
            let vector = vector.ok_or_else(|| SearchError::invalid("document_query_embedding"))?;
            let dimension: Option<usize> = db
                .query_row(
                    "SELECT dimensions FROM embedding_spaces WHERE space=?1",
                    [space.as_ref()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sql_error)?;
            if dimension.is_some_and(|dimension| dimension != vector.dimensions()) {
                return Err(SearchError::invalid("document_embedding_dimensions"));
            }
            result.indexed_vectors = db.query_row("SELECT count(*) FROM chunk_vectors v JOIN chunks c ON c.rowid=v.chunk_id JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1 AND d.chunker_digest=?2 AND v.space=?3 AND v.source_digest=c.content_digest",params![scope,chunker,space.as_ref()],|r|r.get(0)).map_err(sql_error)?;
            vector_search(
                db,
                scope,
                chunker,
                space,
                vector,
                limits.max_scan_records,
                retention,
                &mut result,
            )?
        }
        SearchStrategy::Lexical(LexicalKind::Keyword | LexicalKind::Bm25) => fts_search(
            db,
            scope,
            chunker,
            &query.text,
            (limits.max_scan_records, retention),
            &mut result,
        )?,
        SearchStrategy::Lexical(LexicalKind::Literal | LexicalKind::Regex) => {
            scan_search(db, scope, chunker, query, limits, retention, &mut result)?
        }
        _ => return Err(SearchError::SearchUnsupported),
    };
    for rank in ranked {
        result
            .candidates
            .push(load_candidate(db, rank.rowid, rank.score)?);
    }
    Ok(result)
}

#[derive(Debug, Eq, PartialEq)]
struct Ranked {
    score: u32,
    key: String,
    ordinal: u32,
    rowid: i64,
}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.key.cmp(&other.key))
            .then_with(|| self.ordinal.cmp(&other.ordinal))
            .then_with(|| self.rowid.cmp(&other.rowid))
    }
}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn retain(heap: &mut BinaryHeap<Ranked>, candidate: Ranked, limit: usize) {
    if heap.len() < limit {
        heap.push(candidate);
    } else if heap.peek().is_some_and(|worst| candidate < *worst) {
        heap.pop();
        heap.push(candidate);
    }
}

fn fts_search(
    db: &Connection,
    scope: &str,
    chunker: &str,
    text: &str,
    bounds: (usize, usize),
    result: &mut Retrieved,
) -> Result<Vec<Ranked>, SearchError> {
    let (scan_limit, retention) = bounds;
    let expression = fts_expression(text)?;
    // Materialize the bounded candidate identities before full-text matching.
    // A reduced scan ceiling deliberately reports incomplete corpus coverage.
    let prefix = "WITH bounded AS MATERIALIZED (SELECT c.rowid,c.document_key,c.ordinal FROM chunks c JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1 AND d.chunker_digest=?2 ORDER BY c.document_key,c.ordinal LIMIT ?4) ";
    let count_sql = format!(
        "{prefix} SELECT count(*) FROM bounded c JOIN chunks_fts ON chunks_fts.rowid=c.rowid WHERE chunks_fts MATCH ?3"
    );
    let total: usize = db
        .query_row(
            &count_sql,
            params![scope, chunker, expression, scan_limit],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let ranked_sql = format!(
        "{prefix} SELECT c.rowid,c.document_key,c.ordinal FROM bounded c JOIN chunks_fts ON chunks_fts.rowid=c.rowid WHERE chunks_fts MATCH ?3 ORDER BY bm25(chunks_fts),c.document_key,c.ordinal LIMIT ?5"
    );
    let mut statement = db.prepare(&ranked_sql).map_err(sql_error)?;
    let rows = statement
        .query_map(
            params![scope, chunker, expression, scan_limit, retention],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(sql_error)?;
    let mut ranked = Vec::new();
    for (index, row) in rows.enumerate() {
        let (rowid, key, ordinal) = row.map_err(sql_error)?;
        let rank = u32::try_from(index).map_err(|_| SearchError::invalid("document_rank"))?;
        // SearchHit scores promise per-leg ordering, not raw BM25 arithmetic.
        // Encode SQLite's exact sorted order without comparing its floating
        // values with another source. Fusion normalizes these per-leg values.
        ranked.push(Ranked {
            rowid,
            key,
            ordinal,
            score: u32::MAX - rank,
        });
    }
    result.examined = result.total_chunks.min(scan_limit);
    result.truncated = total > ranked.len() || result.total_chunks > scan_limit;
    Ok(ranked)
}

pub(crate) fn fts_expression(text: &str) -> Result<String, SearchError> {
    finstack_ai_search_core::fts_expression(text)
        .map_err(|_| SearchError::invalid("document_lexical_terms"))
}

fn scan_search(
    db: &Connection,
    scope: &str,
    chunker: &str,
    query: &SearchQuery,
    limits: finstack_ai_search_core::SearchLimits,
    retention: usize,
    result: &mut Retrieved,
) -> Result<Vec<Ranked>, SearchError> {
    let regex = if matches!(query.strategy, SearchStrategy::Lexical(LexicalKind::Regex)) {
        Some(query.regex(&limits)?)
    } else {
        None
    };
    let mut statement = db.prepare("SELECT c.rowid,c.document_key,c.ordinal,c.text FROM chunks c JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1 AND d.chunker_digest=?2 ORDER BY c.document_key,c.ordinal LIMIT ?3").map_err(sql_error)?;
    let rows = statement
        .query_map(params![scope, chunker, limits.max_scan_records], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?))
        })
        .map_err(sql_error)?;
    let mut heap = BinaryHeap::new();
    let mut found = 0;
    for row in rows {
        let (rowid, key, ordinal, text) = row.map_err(sql_error)?;
        result.examined += 1;
        let matched = regex.as_ref().map_or_else(
            || text.contains(query.text.as_ref()),
            |regex| regex.is_match(&text),
        );
        if matched {
            found += 1;
            retain(
                &mut heap,
                Ranked {
                    rowid,
                    key,
                    ordinal,
                    score: 1,
                },
                retention,
            );
        }
    }
    result.truncated = result.total_chunks > result.examined || found > retention;
    let mut ranked = heap.into_vec();
    ranked.sort();
    Ok(ranked)
}

#[allow(clippy::too_many_arguments)] // One bounded source query and its coverage counters.
fn vector_search(
    db: &Connection,
    scope: &str,
    chunker: &str,
    space: &str,
    query: &EmbeddingVector,
    scan_limit: usize,
    retention: usize,
    result: &mut Retrieved,
) -> Result<Vec<Ranked>, SearchError> {
    let mut statement = db.prepare("SELECT c.rowid,c.document_key,c.ordinal,v.vector FROM chunk_vectors v JOIN chunks c ON c.rowid=v.chunk_id JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1 AND d.chunker_digest=?2 AND v.space=?3 AND v.source_digest=c.content_digest ORDER BY c.document_key,c.ordinal LIMIT ?4").map_err(sql_error)?;
    let rows = statement
        .query_map(params![scope, chunker, space, scan_limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, Vec<u8>>(3)?))
        })
        .map_err(sql_error)?;
    let mut heap = BinaryHeap::new();
    for row in rows {
        let (rowid, key, ordinal, encoded) = row.map_err(sql_error)?;
        result.examined += 1;
        if encoded.len() != query.dimensions() * 4 {
            return Err(SearchError::invalid("document_embedding_dimensions"));
        }
        let vector = EmbeddingVector::try_new(
            encoded
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                .collect(),
        )
        .map_err(|_| SearchError::invalid("document_vector"))?;
        let dot = vector
            .dot(query)
            .ok_or_else(|| SearchError::invalid("document_embedding_dimensions"))?;
        retain(
            &mut heap,
            Ranked {
                rowid,
                key,
                ordinal,
                score: similarity_score(dot),
            },
            retention,
        );
    }
    result.truncated = result.indexed_vectors > result.examined || result.examined > retention;
    let mut ranked = heap.into_vec();
    ranked.sort();
    Ok(ranked)
}

fn load_candidate(db: &Connection, rowid: i64, score: u32) -> Result<Candidate, SearchError> {
    let (document_key,input,report,ordinal,start,end,heading,text,digest): (String,String,String,u32,u32,u32,Option<String>,String,String) = db.query_row("SELECT c.document_key,d.input_json,d.report_json,c.ordinal,c.start_char,c.end_char,c.heading,c.text,c.content_digest FROM chunks c JOIN documents d ON d.document_key=c.document_key WHERE c.rowid=?1",[rowid],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).map_err(sql_error)?;
    let _: DocumentIndexReport =
        serde_json::from_str(&report).map_err(|_| SearchError::invalid("document_receipt"))?;
    Ok(Candidate {
        document_key,
        input: serde_json::from_str(&input)
            .map_err(|_| SearchError::invalid("document_catalog"))?,
        ordinal,
        start,
        end,
        heading,
        text,
        digest: Digest::from_hex(&digest)
            .map_err(|_| SearchError::invalid("document_chunk_digest"))?,
        score,
    })
}
