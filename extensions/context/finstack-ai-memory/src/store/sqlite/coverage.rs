//! Scoped coverage queries for the derived embedding index.

use finstack_ai_kernel::Timestamp;
use rusqlite::{Connection, params};

use crate::record::MemoryScope;
use crate::store::{
    EmbeddingCoverage, MemoryStoreError, embedding_source_digest, embedding_source_text,
    validate_embedder_id, validate_scope,
};

use super::queries::{record_from_row, sqlite_unavailable};

pub(super) fn embedding_coverage(
    connection: &Connection,
    scope: &MemoryScope,
    space: &str,
    now: Timestamp,
) -> Result<Option<EmbeddingCoverage>, MemoryStoreError> {
    validate_scope(scope)?;
    validate_embedder_id(space)?;
    let digest = scope
        .digest()
        .map_err(|_| MemoryStoreError::InvalidRequest {
            reason: "memory_scope_invalid",
        })?
        .to_hex();
    let mut statement = connection.prepare(
        "SELECT r.*, e.source_digest AS indexed_source_digest FROM memory_records r
         LEFT JOIN memory_embeddings e ON e.scope_digest=r.scope_digest AND e.id=r.id AND e.embedder_id=?1
         WHERE r.scope_digest=?2 AND r.tombstoned=0 AND r.superseded_by IS NULL
         AND (r.expires_at IS NULL OR r.expires_at>?3)"
    ).map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(params![space, digest, now.as_unix_ms()], |row| {
            let record = record_from_row(row)?;
            let indexed: Option<String> = row.get("indexed_source_digest")?;
            Ok((record, indexed))
        })
        .map_err(|_| sqlite_unavailable())?;
    let mut coverage = EmbeddingCoverage {
        live_records: 0,
        indexed_records: 0,
    };
    for row in rows {
        let (record, indexed) = row.map_err(|_| sqlite_unavailable())?;
        coverage.live_records += 1;
        let digest = embedding_source_digest(&embedding_source_text(&record))?;
        if indexed.is_some_and(|value| value == digest.to_hex()) {
            coverage.indexed_records += 1;
        }
    }
    Ok(Some(coverage))
}
