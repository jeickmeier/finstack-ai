//! File-owned schema versioning for [`super::SqliteMemoryStore`].

use std::sync::Arc;

use rusqlite::{Connection, Transaction, params};

use crate::record::MemoryRecord;

use super::super::MemoryStoreError;
use super::queries::{legacy_record_from_row, sqlite_unavailable, write_record};

/// Applied `PRAGMA user_version` for the current schema.
const SCHEMA_USER_VERSION: i32 = 3;

/// `PRAGMA user_version` of the last schema without the embedding index.
const V2_USER_VERSION: i32 = 2;

/// The v3 addition: the per-space embedding index. Shared verbatim between
/// the fresh-create DDL and the additive v2→v3 migration.
const EMBEDDINGS_DDL: &str = "
CREATE TABLE memory_embeddings (
  scope_digest TEXT NOT NULL, id TEXT NOT NULL, embedder_id TEXT NOT NULL,
  dimensions INTEGER NOT NULL, source_digest TEXT NOT NULL,
  vector BLOB NOT NULL, embedded_at INTEGER NOT NULL,
  PRIMARY KEY (scope_digest, id, embedder_id)
);
CREATE INDEX memory_embeddings_space ON memory_embeddings(embedder_id, scope_digest);
";

const V3_BASE_DDL: &str = "
CREATE TABLE memory_records (
  scope_digest TEXT NOT NULL,
  id TEXT NOT NULL,
  tenant TEXT NOT NULL,
  user TEXT, agent TEXT, workspace TEXT,
  body_inline TEXT,
  blob_ref_json TEXT,
  preview TEXT NOT NULL,
  sensitivity TEXT NOT NULL,
  keywords_json TEXT NOT NULL,
  provenance_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  last_confirmed_at INTEGER NOT NULL,
  supersedes TEXT, superseded_by TEXT,
  retention_json TEXT NOT NULL,
  expires_at INTEGER,
  tombstoned INTEGER NOT NULL DEFAULT 0
    CHECK (tombstoned IN (0, 1)),
  CHECK ((body_inline IS NULL) != (blob_ref_json IS NULL)),
  PRIMARY KEY (scope_digest, id)
);
CREATE INDEX memory_records_scope ON memory_records(scope_digest, id);
CREATE INDEX memory_records_expiry ON memory_records(expires_at) WHERE expires_at IS NOT NULL;
CREATE VIRTUAL TABLE memory_fts USING fts5(
  scope_digest UNINDEXED, id UNINDEXED, preview, body, keywords
);
CREATE TABLE memory_keywords (
  scope_digest TEXT NOT NULL,
  id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  keyword TEXT NOT NULL,
  keyword_folded TEXT NOT NULL,
  PRIMARY KEY (scope_digest, id, ordinal)
);
CREATE INDEX memory_keywords_lookup
  ON memory_keywords(scope_digest, keyword_folded, id);
CREATE TABLE memory_idempotency (
  scope_digest TEXT NOT NULL,
  key TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  applied_at INTEGER NOT NULL,
  PRIMARY KEY (scope_digest, key)
);
CREATE TABLE memory_artifact_outbox (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  action_id TEXT NOT NULL UNIQUE,
  action_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
";

const MIGRATE_V1_DDL: &str = "
ALTER TABLE memory_records RENAME TO memory_records_v1;
DROP INDEX IF EXISTS memory_records_tenant;
DROP TABLE memory_fts;
CREATE TABLE memory_records (
  scope_digest TEXT NOT NULL, id TEXT NOT NULL,
  tenant TEXT NOT NULL, user TEXT, agent TEXT, workspace TEXT,
  body_inline TEXT, blob_ref_json TEXT, preview TEXT NOT NULL,
  sensitivity TEXT NOT NULL, keywords_json TEXT NOT NULL,
  provenance_json TEXT NOT NULL, created_at INTEGER NOT NULL,
  last_confirmed_at INTEGER NOT NULL, supersedes TEXT,
  superseded_by TEXT, retention_json TEXT NOT NULL,
  expires_at INTEGER, tombstoned INTEGER NOT NULL DEFAULT 0
    CHECK (tombstoned IN (0, 1)),
  CHECK ((body_inline IS NULL) != (blob_ref_json IS NULL)),
  PRIMARY KEY (scope_digest, id)
);
CREATE INDEX memory_records_scope ON memory_records(scope_digest, id);
CREATE INDEX memory_records_expiry
  ON memory_records(expires_at) WHERE expires_at IS NOT NULL;
CREATE VIRTUAL TABLE memory_fts USING fts5(
  scope_digest UNINDEXED, id UNINDEXED, preview, body, keywords
);
CREATE TABLE memory_keywords (
  scope_digest TEXT NOT NULL,
  id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  keyword TEXT NOT NULL,
  keyword_folded TEXT NOT NULL,
  PRIMARY KEY (scope_digest, id, ordinal)
);
CREATE INDEX memory_keywords_lookup
  ON memory_keywords(scope_digest, keyword_folded, id);
ALTER TABLE memory_idempotency RENAME TO memory_idempotency_v1;
CREATE TABLE memory_idempotency (
  scope_digest TEXT NOT NULL, key TEXT NOT NULL,
  fingerprint TEXT NOT NULL, applied_at INTEGER NOT NULL,
  PRIMARY KEY (scope_digest, key)
);
INSERT INTO memory_idempotency (scope_digest, key, fingerprint, applied_at)
SELECT '*', key, 'legacy-unverifiable', applied_at FROM memory_idempotency_v1;
DROP TABLE memory_idempotency_v1;
CREATE TABLE memory_artifact_outbox (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  action_id TEXT NOT NULL UNIQUE,
  action_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
";

/// Create or migrate the store's schema to [`SCHEMA_USER_VERSION`], keyed by
/// the file-owned `PRAGMA user_version`. A version newer than this build
/// understands fails closed (`memory_store_sqlite_schema_unsupported`).
pub(super) fn apply_schema(connection: &Connection) -> Result<(), MemoryStoreError> {
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| sqlite_unavailable())?;
    match version {
        0 => create_v3_schema(connection),
        1 => {
            migrate_v1_schema(connection)?;
            migrate_v2_to_v3_schema(connection)
        }
        V2_USER_VERSION => migrate_v2_to_v3_schema(connection),
        SCHEMA_USER_VERSION => Ok(()),
        _ => Err(MemoryStoreError::Unavailable {
            message: Arc::from("memory_store_sqlite_schema_unsupported"),
        }),
    }
}

fn create_v3_schema(connection: &Connection) -> Result<(), MemoryStoreError> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| sqlite_unavailable())?;
    transaction
        .execute_batch(V3_BASE_DDL)
        .map_err(|_| sqlite_unavailable())?;
    transaction
        .execute_batch(EMBEDDINGS_DDL)
        .map_err(|_| sqlite_unavailable())?;
    finish_schema_transaction(transaction, SCHEMA_USER_VERSION)
}

/// Additive v2→v3 step: create the embedding index, verify integrity, bump
/// the version. Pre-existing records gain no rows, so the schema upgrade
/// itself makes every record pending for every space (backfill by drain).
fn migrate_v2_to_v3_schema(connection: &Connection) -> Result<(), MemoryStoreError> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| sqlite_unavailable())?;
    transaction
        .execute_batch(EMBEDDINGS_DDL)
        .map_err(|_| sqlite_unavailable())?;
    let quick_check: String = transaction
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| sqlite_unavailable())?;
    if quick_check != "ok" {
        return Err(sqlite_unavailable());
    }
    finish_schema_transaction(transaction, SCHEMA_USER_VERSION)
}

fn migrate_v1_schema(connection: &Connection) -> Result<(), MemoryStoreError> {
    let legacy_blobs: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM memory_records WHERE blob_ref_json IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|_| sqlite_unavailable())?;
    if legacy_blobs != 0 {
        return Err(MemoryStoreError::Unavailable {
            message: Arc::from("memory_store_sqlite_legacy_blob_unverifiable"),
        });
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| sqlite_unavailable())?;
    transaction
        .execute_batch(
            "ALTER TABLE memory_records ADD COLUMN expires_at INTEGER;
             UPDATE memory_records
             SET expires_at = created_at + CAST(
               json_extract(retention_json, '$.expire_after_ms') AS INTEGER
             )
             WHERE json_type(retention_json, '$.expire_after_ms') IS NOT NULL;",
        )
        .map_err(|_| sqlite_unavailable())?;
    let records = load_legacy_records(&transaction)?;
    transaction
        .execute_batch(MIGRATE_V1_DDL)
        .map_err(|_| sqlite_unavailable())?;
    for record in &records {
        write_record(&transaction, record)?;
    }
    transaction
        .execute("DROP TABLE memory_records_v1", [])
        .map_err(|_| sqlite_unavailable())?;
    let quick_check: String = transaction
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| sqlite_unavailable())?;
    if quick_check != "ok" {
        return Err(sqlite_unavailable());
    }
    // The v1 migration produces an exact v2 database; the shared v2→v3 step
    // then finishes the chain.
    finish_schema_transaction(transaction, V2_USER_VERSION)
}

fn load_legacy_records(
    transaction: &Transaction<'_>,
) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
    let mut statement = transaction
        .prepare("SELECT * FROM memory_records ORDER BY id")
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map([], legacy_record_from_row)
        .map_err(|_| sqlite_unavailable())?;
    rows.map(|row| row.map_err(|_| sqlite_unavailable()))
        .collect()
}

fn finish_schema_transaction(
    transaction: Transaction<'_>,
    version: i32,
) -> Result<(), MemoryStoreError> {
    transaction
        .pragma_update(None, "user_version", version)
        .map_err(|_| sqlite_unavailable())?;
    transaction.commit().map_err(|_| sqlite_unavailable())
}

pub(super) fn ensure_v2_auxiliary_tables(connection: &Connection) -> Result<(), MemoryStoreError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS memory_keywords (
               scope_digest TEXT NOT NULL,
               id TEXT NOT NULL,
               ordinal INTEGER NOT NULL,
               keyword TEXT NOT NULL,
               keyword_folded TEXT NOT NULL,
               PRIMARY KEY (scope_digest, id, ordinal)
             );
             CREATE INDEX IF NOT EXISTS memory_keywords_lookup
               ON memory_keywords(scope_digest, keyword_folded, id);
             INSERT OR IGNORE INTO memory_keywords
               (scope_digest, id, ordinal, keyword, keyword_folded)
             SELECT r.scope_digest, r.id, CAST(j.key AS INTEGER),
                    CAST(j.value AS TEXT), lower(CAST(j.value AS TEXT))
             FROM memory_records r, json_each(r.keywords_json) j;",
        )
        .map_err(|_| sqlite_unavailable())
}

pub(super) fn ensure_store_identity(
    connection: &Connection,
    proposed_store_id: &str,
) -> Result<Arc<str>, MemoryStoreError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS memory_meta (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               store_id TEXT NOT NULL
             );",
        )
        .map_err(|_| sqlite_unavailable())?;
    connection
        .execute(
            "INSERT OR IGNORE INTO memory_meta (singleton, store_id) VALUES (1, ?1)",
            params![proposed_store_id],
        )
        .map_err(|_| sqlite_unavailable())?;
    let stored: String = connection
        .query_row(
            "SELECT store_id FROM memory_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| sqlite_unavailable())?;
    if stored.is_empty() || stored.len() > 256 || stored.as_bytes().contains(&0) {
        return Err(sqlite_unavailable());
    }
    Ok(Arc::from(stored))
}
