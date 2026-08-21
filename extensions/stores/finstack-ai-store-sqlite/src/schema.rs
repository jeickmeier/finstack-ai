use rusqlite::Connection;

use finstack_ai_runtime::StoreError;

use crate::config::{SqliteDurability, SqliteSynchronous};
use crate::error::map_sqlite_error;

/// Applied `PRAGMA user_version` for the v1 schema. Other versions fail closed.
pub const SCHEMA_USER_VERSION: i32 = 1;

const V1_DDL: &str = "
CREATE TABLE sessions (
  session_id BLOB PRIMARY KEY,
  current_sequence INTEGER NOT NULL,
  head_checksum BLOB,
  chain_anchor_sequence INTEGER NOT NULL DEFAULT 1,
  chain_anchor_checksum BLOB,
  snapshot_sequence INTEGER,
  metadata BLOB NOT NULL
);
CREATE TABLE batches (
  batch_id BLOB PRIMARY KEY,
  session_id BLOB NOT NULL REFERENCES sessions(session_id),
  first_sequence INTEGER NOT NULL,
  last_sequence INTEGER NOT NULL,
  expected_sequence INTEGER NOT NULL,
  request_cbor BLOB NOT NULL,
  UNIQUE (session_id, first_sequence)
);
CREATE TABLE records (
  session_id BLOB NOT NULL,
  sequence INTEGER NOT NULL,
  record_id BLOB NOT NULL UNIQUE,
  batch_id BLOB NOT NULL REFERENCES batches(batch_id),
  lane_id BLOB NOT NULL,
  run_id BLOB,
  kind TEXT NOT NULL,
  format_version INTEGER NOT NULL,
  kind_version INTEGER NOT NULL,
  payload_cbor BLOB NOT NULL,
  timestamp INTEGER NOT NULL,
  committed_at INTEGER,
  payload_digest BLOB NOT NULL,
  previous_checksum BLOB,
  envelope_checksum BLOB NOT NULL,
  derived_event_ids BLOB NOT NULL,
  PRIMARY KEY (session_id, sequence)
);
CREATE TABLE snapshots (
  session_id BLOB PRIMARY KEY,
  sequence INTEGER NOT NULL,
  payload_cbor BLOB NOT NULL,
  digest BLOB NOT NULL,
  timestamp INTEGER NOT NULL
);
CREATE INDEX records_batch_id ON records(batch_id);
CREATE INDEX batches_session_first ON batches(session_id, first_sequence);
";

pub(crate) fn apply_durability(
    connection: &Connection,
    durability: SqliteDurability,
    memory: bool,
) -> Result<(), StoreError> {
    match durability {
        SqliteDurability::Durable => {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(map_sqlite_error)?;
            connection
                .pragma_update(None, "synchronous", "FULL")
                .map_err(map_sqlite_error)?;
            #[cfg(target_os = "macos")]
            {
                connection
                    .pragma_update(None, "fullfsync", "ON")
                    .map_err(map_sqlite_error)?;
                connection
                    .pragma_update(None, "checkpoint_fullfsync", "ON")
                    .map_err(map_sqlite_error)?;
            }
        }
        SqliteDurability::Relaxed { synchronous } => {
            if !memory {
                connection
                    .pragma_update(None, "journal_mode", "WAL")
                    .map_err(map_sqlite_error)?;
            }
            let value = match synchronous {
                SqliteSynchronous::Normal => "NORMAL",
                SqliteSynchronous::Off => "OFF",
            };
            connection
                .pragma_update(None, "synchronous", value)
                .map_err(map_sqlite_error)?;
        }
    }
    Ok(())
}

pub(crate) fn apply_schema(connection: &Connection) -> Result<(), StoreError> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(map_sqlite_error)?;
    let result = (|| {
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(map_sqlite_error)?;
        match version {
            0 => {
                connection.execute_batch(V1_DDL).map_err(map_sqlite_error)?;
                connection
                    .pragma_update(None, "user_version", SCHEMA_USER_VERSION)
                    .map_err(map_sqlite_error)
            }
            SCHEMA_USER_VERSION => Ok(()),
            _ => Err(StoreError::Integrity {
                reason_code: "sqlite_schema_unsupported",
            }),
        }
    })();
    match result {
        Ok(()) => connection.execute_batch("COMMIT").map_err(map_sqlite_error),
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

pub(crate) fn quick_check(connection: &Connection) -> Result<(), StoreError> {
    let status: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(map_sqlite_error)?;
    if status == "ok" {
        Ok(())
    } else {
        Err(StoreError::Integrity {
            reason_code: "sqlite_quick_check",
        })
    }
}
