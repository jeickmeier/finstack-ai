//! Scoped coverage queries for the derived embedding index.

use finstack_ai_kernel::Timestamp;
use rusqlite::{Connection, params};

use crate::record::MemoryScope;
use crate::store::{EmbeddingCoverage, MemoryStoreError, validate_embedder_id, validate_scope};

use super::queries::sqlite_unavailable;

/// Two counts over the scope's live rows: all of them, and those whose row
/// in the space carries the record's current stored source digest.
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
    let live_records: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM memory_records r
             WHERE r.scope_digest = ?1 AND r.tombstoned = 0 AND r.superseded_by IS NULL
               AND (r.expires_at IS NULL OR r.expires_at > ?2)",
            params![digest, now.as_unix_ms()],
            |row| row.get(0),
        )
        .map_err(|_| sqlite_unavailable())?;
    let indexed_records: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM memory_records r
             JOIN memory_embeddings e
               ON e.scope_digest = r.scope_digest AND e.id = r.id AND e.embedder_id = ?1
             WHERE r.scope_digest = ?2 AND r.tombstoned = 0 AND r.superseded_by IS NULL
               AND (r.expires_at IS NULL OR r.expires_at > ?3)
               AND e.source_digest IS r.source_digest",
            params![space, digest, now.as_unix_ms()],
            |row| row.get(0),
        )
        .map_err(|_| sqlite_unavailable())?;
    Ok(Some(EmbeddingCoverage {
        live_records: usize::try_from(live_records).unwrap_or(usize::MAX),
        indexed_records: usize::try_from(indexed_records).unwrap_or(usize::MAX),
    }))
}
