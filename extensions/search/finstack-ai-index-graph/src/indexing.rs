use finstack_ai_search_core::{
    SearchError, SearchEvidence, SearchHit, SourceRef, configuration_digest,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{GraphSearchSource, vocabulary::Extraction};

/// Result of replacing facts derived from one exact source reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphIndexReport {
    /// Stable source-reference key, usable as a reconciliation cursor.
    pub source_key: String,
    /// Number of entities supported by this reference after extraction.
    pub entities: usize,
    /// Number of directed relationships supported by this reference.
    pub edges: usize,
    /// The source no longer exists and its derived evidence was removed.
    pub removed: bool,
    /// Whether the exact source supplied all of its authoritative text.
    pub complete: bool,
}

/// Bounded reconciliation progress. Continue using `next_cursor` until absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphReconcileReport {
    /// Number of exact references checked.
    pub examined: usize,
    /// Number of absent references removed.
    pub removed: usize,
    /// References unavailable for checking; their evidence remains unservable.
    pub unavailable: usize,
    /// Last checked key when another stored reference remains.
    pub next_cursor: Option<String>,
}

#[derive(Clone)]
pub(crate) struct StoredSource {
    pub(crate) key: String,
    pub(crate) source: String,
    pub(crate) reference: SourceRef,
    pub(crate) hit: SearchHit,
    pub(crate) extraction: String,
}

impl GraphSearchSource {
    /// Read current exact source evidence and atomically replace all facts it
    /// supports. Repeating the same input is idempotent. Missing sources remove
    /// their own support, retaining facts independently supported elsewhere.
    ///
    /// # Errors
    /// Rejects unknown sources, unavailable evidence, excessive extraction or
    /// index capacities, and concurrent maintenance on this handle.
    pub async fn index_reference(
        &self,
        source: &str,
        reference: SourceRef,
    ) -> Result<GraphIndexReport, SearchError> {
        let _guard = self
            .maintenance
            .try_lock()
            .map_err(|_| SearchError::SearchUnavailable)?;
        self.index_one(source, reference).await
    }

    async fn index_one(
        &self,
        source: &str,
        reference: SourceRef,
    ) -> Result<GraphIndexReport, SearchError> {
        finstack_ai_search_core::validate_id(source)?;
        if !self.sources.contains_key(source) {
            return Err(SearchError::SearchUnsupported);
        }
        let key = configuration_digest(
            "graph-source",
            &(&self.scope_digest, source, reference.key()?),
        )?
        .to_hex();
        let Some(evidence) = self.evidence(source, reference.clone()).await? else {
            self.remove_source(key.clone(), None).await?;
            return Ok(GraphIndexReport {
                source_key: key,
                entities: 0,
                edges: 0,
                removed: true,
                complete: true,
            });
        };
        let extraction = self.compiled.extract(
            &self.scope_digest,
            &evidence.text,
            self.config.max_entities_per_source,
            self.config.max_edges_per_source,
        )?;
        let report = GraphIndexReport {
            source_key: key.clone(),
            entities: extraction.entities.len(),
            edges: extraction.edges.len(),
            removed: false,
            complete: evidence.complete,
        };
        let this = self.clone();
        let source = source.to_owned();
        self.database
            .call(move |connection| {
                this.replace(
                    connection,
                    &key,
                    &source,
                    &reference,
                    &evidence,
                    &extraction,
                )
            })
            .await?;
        Ok(report)
    }

    fn replace(
        &self,
        connection: &mut Connection,
        key: &str,
        source: &str,
        reference: &SourceRef,
        evidence: &SearchEvidence,
        extraction: &Extraction,
    ) -> Result<(), SearchError> {
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        tx.execute(
            "DELETE FROM sources WHERE scope=? AND source_key=?",
            params![self.scope_digest, key],
        )
        .map_err(db_error)?;
        cleanup(&tx)?;
        tx.execute(
            "INSERT INTO sources VALUES(?,?,?,?,?,?,?)",
            params![
                key,
                self.scope_digest,
                source,
                reference.key()?,
                serde_json::to_string(&evidence.hit)
                    .map_err(|_| SearchError::invalid("graph_evidence_encoding"))?,
                evidence.complete,
                self.extraction_digest.to_hex()
            ],
        )
        .map_err(db_error)?;
        for entity in extraction.entities.values() {
            tx.execute(
                "INSERT OR IGNORE INTO entities VALUES(?,?,?,?)",
                params![
                    entity.id,
                    self.scope_digest,
                    entity.kind.as_ref(),
                    entity.label.as_ref()
                ],
            )
            .map_err(db_error)?;
            tx.execute(
                "INSERT INTO entity_sources VALUES(?,?)",
                params![entity.id, key],
            )
            .map_err(db_error)?;
            for alias in &entity.aliases {
                tx.execute(
                    "INSERT INTO aliases VALUES(?,?,?)",
                    params![entity.id, key, alias],
                )
                .map_err(db_error)?;
            }
            capacity(
                &tx,
                "SELECT count(*) FROM entity_sources WHERE entity=?",
                &entity.id,
                self.config.max_support_per_item,
                "graph_entity_support",
            )?;
        }
        for edge in extraction.edges.values() {
            tx.execute(
                "INSERT OR IGNORE INTO edges VALUES(?,?,?,?,?)",
                params![
                    edge.id,
                    self.scope_digest,
                    edge.source,
                    edge.kind.as_ref(),
                    edge.target
                ],
            )
            .map_err(db_error)?;
            tx.execute(
                "INSERT INTO edge_sources VALUES(?,?)",
                params![edge.id, key],
            )
            .map_err(db_error)?;
            capacity(
                &tx,
                "SELECT count(*) FROM edge_sources WHERE edge=?",
                &edge.id,
                self.config.max_support_per_item,
                "graph_edge_support",
            )?;
        }
        for (sql, max, resource) in [
            (
                "SELECT count(*) FROM sources WHERE scope=?",
                self.config.max_sources,
                "graph_sources",
            ),
            (
                "SELECT count(*) FROM entities WHERE scope=?",
                self.config.max_entities,
                "graph_entities",
            ),
            (
                "SELECT count(*) FROM edges WHERE scope=?",
                self.config.max_edges,
                "graph_edges",
            ),
        ] {
            capacity(&tx, sql, &self.scope_digest, max, resource)?;
        }
        tx.commit().map_err(db_error)
    }

    /// Re-read a bounded page of indexed references, refreshing corrections and
    /// removing forgotten/deleted evidence. A failed source is counted and kept
    /// for a later retry; queries independently revalidate before serving it.
    ///
    /// # Errors
    /// Rejects invalid bounds/cursors, storage failure or concurrent maintenance.
    pub async fn reconcile(
        &self,
        after: Option<String>,
        limit: usize,
    ) -> Result<GraphReconcileReport, SearchError> {
        if limit == 0
            || limit > self.config.max_evidence_reads
            || after
                .as_ref()
                .is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(SearchError::invalid("graph_reconcile_bounds"));
        }
        let _guard = self
            .maintenance
            .try_lock()
            .map_err(|_| SearchError::SearchUnavailable)?;
        let scope = self.scope_digest.clone();
        let mut rows = self.database.call(move |c| {
            let mut statement = c.prepare("SELECT source_key,source_id,reference,hit_json,extraction_digest FROM sources WHERE scope=? AND source_key>? ORDER BY source_key LIMIT ?").map_err(db_error)?;
            let rows = statement.query_map(params![scope, after.unwrap_or_default(), limit + 1], decode_source).map_err(db_error)?;
            rows.map(|row| row.map_err(db_error).and_then(parse_source)).collect::<Result<Vec<_>, _>>()
        }).await?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        let mut report = GraphReconcileReport {
            examined: rows.len(),
            removed: 0,
            unavailable: 0,
            next_cursor: if more {
                rows.last().map(|r| r.key.clone())
            } else {
                None
            },
        };
        for row in rows {
            match self.index_one(&row.source, row.reference).await {
                Ok(result) => report.removed += usize::from(result.removed),
                Err(_) => report.unavailable += 1,
            }
        }
        Ok(report)
    }

    pub(crate) async fn remove_source(
        &self,
        key: String,
        expected: Option<SearchHit>,
    ) -> Result<(), SearchError> {
        let scope = self.scope_digest.clone();
        self.database
            .call(move |c| {
                let tx = c
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(db_error)?;
                if let Some(expected) = expected {
                    let current: Option<String> = tx
                        .query_row(
                            "SELECT hit_json FROM sources WHERE scope=? AND source_key=?",
                            params![scope, key],
                            |r| r.get(0),
                        )
                        .optional()
                        .map_err(db_error)?;
                    if current
                        != Some(
                            serde_json::to_string(&expected)
                                .map_err(|_| SearchError::invalid("graph_evidence_encoding"))?,
                        )
                    {
                        return Ok(());
                    }
                }
                tx.execute(
                    "DELETE FROM sources WHERE scope=? AND source_key=?",
                    params![scope, key],
                )
                .map_err(db_error)?;
                cleanup(&tx)?;
                tx.commit().map_err(db_error)
            })
            .await
    }
}

pub(crate) fn db_error(_: rusqlite::Error) -> SearchError {
    SearchError::SearchUnavailable
}
fn cleanup(c: &Connection) -> Result<(), SearchError> {
    c.execute(
        "DELETE FROM edges WHERE NOT EXISTS(SELECT 1 FROM edge_sources WHERE edge=edges.id)",
        [],
    )
    .map_err(db_error)?;
    c.execute("DELETE FROM entities WHERE NOT EXISTS(SELECT 1 FROM entity_sources WHERE entity=entities.id)", []).map_err(db_error)?;
    Ok(())
}
fn capacity(
    c: &Connection,
    sql: &str,
    key: &str,
    max: usize,
    resource: &'static str,
) -> Result<(), SearchError> {
    let count: usize = c.query_row(sql, [key], |r| r.get(0)).map_err(db_error)?;
    if count > max {
        return Err(SearchError::SearchCapacityExceeded {
            resource: resource.into(),
        });
    }
    Ok(())
}
type SourceRow = (String, String, String, String, String);
pub(crate) fn decode_source(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}
pub(crate) fn parse_source(row: SourceRow) -> Result<StoredSource, SearchError> {
    Ok(StoredSource {
        key: row.0,
        source: row.1,
        reference: serde_json::from_str(&row.2)
            .map_err(|_| SearchError::invalid("graph_index_reference"))?,
        hit: serde_json::from_str(&row.3)
            .map_err(|_| SearchError::invalid("graph_index_evidence"))?,
        extraction: row.4,
    })
}
