//! Sqlite-backed adapter tables for the workflow worker.
//!
//! [`SqliteWorkerStore`] owns three tables in one file: the wake index, the
//! cron-fire table, and the response inbox. None of them touch `PRAGMA
//! user_version` — they are adapter state, not kernel journal records.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{Digest, Id, LaneId, RunId, SessionId, Timestamp};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::WorkerError;
use crate::fires::{FireRow, FireStartOutcome, FireStatus, FireStore};
use crate::inbox::{
    DeadLetterRow, InboxInsertOutcome, InboxKind, InboxRow, InboxStore, MAX_INBOX_PAYLOAD_BYTES,
};
use crate::wake::{WakeIndexStore, WakeReason, WakeRow, lease_deadline};

/// Current schema version owned by this adapter.
const WORKER_SCHEMA_VERSION: i64 = 1;

/// Schema for the worker's adapter tables.
///
/// The worker owns an explicit metadata table rather than reusing
/// `PRAGMA user_version`, which belongs to the journal in shared files. This
/// release intentionally requires a fresh adapter schema: opening an
/// unversioned historical worker table fails with a reset-required error.
const WORKER_DDL: &str = "
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_schema (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  version INTEGER NOT NULL
);
INSERT OR IGNORE INTO finstack_workflow_worker_schema (singleton, version) VALUES (1, 1);
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_wake (
  tenant_scope TEXT NOT NULL,
  session_id TEXT NOT NULL,
  lane_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  workflow_kind TEXT NOT NULL,
  reason TEXT NOT NULL,
  wake_at_unix_ms INTEGER,
  expires_at_unix_ms INTEGER,
  pending_id TEXT NOT NULL,
  leased_by TEXT,
  lease_expires_unix_ms INTEGER,
  attempts INTEGER NOT NULL,
  PRIMARY KEY (tenant_scope, session_id)
);
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_fires (
  tenant_scope TEXT NOT NULL,
  schedule_id TEXT NOT NULL,
  fire_count INTEGER NOT NULL,
  fired_unix_ms INTEGER NOT NULL,
  status TEXT NOT NULL,
  started_session TEXT,
  PRIMARY KEY (tenant_scope, schedule_id, fire_count)
);
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_inbox (
  tenant_scope TEXT NOT NULL,
  session_id TEXT NOT NULL,
  pending_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload BLOB NOT NULL,
  payload_digest TEXT NOT NULL,
  received_unix_ms INTEGER NOT NULL,
  PRIMARY KEY (tenant_scope, session_id, pending_id)
);
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_dead_letters (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  tenant_scope TEXT NOT NULL,
  session_id TEXT NOT NULL,
  pending_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload BLOB NOT NULL,
  payload_digest TEXT NOT NULL,
  received_unix_ms INTEGER NOT NULL,
  reason_code TEXT NOT NULL,
  rejected_unix_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS finstack_workflow_worker_dead_letters_age
ON finstack_workflow_worker_dead_letters (rejected_unix_ms, id);
";

/// Sqlite tables backing the workflow worker's adapter state.
///
/// Rows are hints for the tick loop; the kernel journal stays authoritative.
/// A single connection is guarded by a mutex so callers may share one store
/// across threads, and separate processes coordinate through sqlite's own
/// file locking (`BEGIN IMMEDIATE` for CAS operations).
pub struct SqliteWorkerStore {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl SqliteWorkerStore {
    /// Open or create the adapter tables in `path`.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreUnavailable`] when the file cannot be
    /// opened, `busy_timeout`/WAL cannot be configured, or the schema cannot
    /// be created.
    pub fn try_open(path: impl AsRef<Path>) -> Result<Self, WorkerError> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|_| WorkerError::StoreUnavailable {
            code: "sqlite_worker_open",
        })?;
        conn.busy_timeout(Duration::from_secs(1))
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_worker_busy_timeout",
            })?;
        if !is_memory_sqlite_path(&path) {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_worker_wal",
                })?;
        }
        let has_schema = table_exists(&conn, "finstack_workflow_worker_schema")?;
        if !has_schema && has_legacy_worker_tables(&conn)? {
            return Err(WorkerError::InvalidConfiguration {
                code: "workflow_schema_reset_required",
            });
        }
        conn.execute_batch(WORKER_DDL)
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_worker_schema",
            })?;
        let version: i64 = conn
            .query_row(
                "SELECT version FROM finstack_workflow_worker_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| WorkerError::StoreIntegrity {
                code: "workflow_schema_version",
            })?;
        if version != WORKER_SCHEMA_VERSION {
            return Err(WorkerError::InvalidConfiguration {
                code: "workflow_schema_version",
            });
        }
        Ok(Self {
            path,
            conn: Mutex::new(conn),
        })
    }

    /// Configured database file, including `:memory:`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with_conn<T>(
        &self,
        body: impl FnOnce(&mut Connection) -> Result<T, WorkerError>,
    ) -> Result<T, WorkerError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_worker_lock_poisoned",
            })?;
        body(&mut conn)
    }
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, WorkerError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
    .map_err(|_| WorkerError::StoreUnavailable {
        code: "sqlite_worker_schema",
    })
}

fn has_legacy_worker_tables(conn: &Connection) -> Result<bool, WorkerError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table'
               AND name LIKE 'finstack_workflow_worker_%'
         )",
        [],
        |row| row.get(0),
    )
    .map_err(|_| WorkerError::StoreUnavailable {
        code: "sqlite_worker_schema",
    })
}

/// Raw columns for one wake row, as read from sqlite before decoding.
struct RawWakeRow {
    tenant_scope: String,
    session_id: String,
    lane_id: String,
    run_id: String,
    workflow_kind: String,
    reason: String,
    wake_at_unix_ms: Option<i64>,
    expires_at_unix_ms: Option<i64>,
    pending_id: String,
    leased_by: Option<String>,
    lease_expires_unix_ms: Option<i64>,
    attempts: i64,
}

/// Decode one wake row, mapping parse failures to [`WorkerError::StoreIntegrity`].
fn decode_wake_row(raw: RawWakeRow) -> Result<WakeRow, WorkerError> {
    let session_id: SessionId =
        Id::parse(&raw.session_id).map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_wake_id",
        })?;
    let lane_id: LaneId = Id::parse(&raw.lane_id).map_err(|_| WorkerError::StoreIntegrity {
        code: "sqlite_wake_id",
    })?;
    let run_id: RunId = Id::parse(&raw.run_id).map_err(|_| WorkerError::StoreIntegrity {
        code: "sqlite_wake_id",
    })?;
    let reason = WakeReason::parse(&raw.reason)?;
    let wake_at = raw
        .wake_at_unix_ms
        .map(Timestamp::from_unix_ms)
        .transpose()
        .map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_wake_time",
        })?;
    let expires_at = raw
        .expires_at_unix_ms
        .map(Timestamp::from_unix_ms)
        .transpose()
        .map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_wake_time",
        })?;
    let lease_expires_at = raw
        .lease_expires_unix_ms
        .map(Timestamp::from_unix_ms)
        .transpose()
        .map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_wake_time",
        })?;
    let attempts = u32::try_from(raw.attempts).map_err(|_| WorkerError::StoreIntegrity {
        code: "sqlite_wake_row",
    })?;
    Ok(WakeRow {
        tenant_scope: raw.tenant_scope.into(),
        session_id,
        lane_id,
        run_id,
        workflow_kind: raw.workflow_kind.into(),
        reason,
        wake_at,
        expires_at,
        pending_id: raw.pending_id.into(),
        leased_by: raw.leased_by.map(Into::into),
        lease_expires_at,
        attempts,
    })
}

/// Query rows matching `sql`/`args`, decoding each into a [`WakeRow`].
fn query_wake_rows(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<WakeRow>, WorkerError> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|_| WorkerError::StoreUnavailable {
            code: "sqlite_wake_row",
        })?;
    let rows = stmt
        .query_map(args, |row| {
            Ok(RawWakeRow {
                tenant_scope: row.get(0)?,
                session_id: row.get(1)?,
                lane_id: row.get(2)?,
                run_id: row.get(3)?,
                workflow_kind: row.get(4)?,
                reason: row.get(5)?,
                wake_at_unix_ms: row.get(6)?,
                expires_at_unix_ms: row.get(7)?,
                pending_id: row.get(8)?,
                leased_by: row.get(9)?,
                lease_expires_unix_ms: row.get(10)?,
                attempts: row.get(11)?,
            })
        })
        .map_err(|_| WorkerError::StoreUnavailable {
            code: "sqlite_wake_row",
        })?;
    rows.map(|row| {
        decode_wake_row(row.map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_wake_row",
        })?)
    })
    .collect()
}

const WAKE_SELECT: &str = "SELECT tenant_scope, session_id, lane_id, run_id, workflow_kind, reason,
       wake_at_unix_ms, expires_at_unix_ms, pending_id, leased_by, lease_expires_unix_ms,
       attempts
FROM finstack_workflow_worker_wake";

impl WakeIndexStore for SqliteWorkerStore {
    fn upsert(&self, row: &WakeRow) -> Result<(), WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO finstack_workflow_worker_wake (
                    tenant_scope, session_id, lane_id, run_id, workflow_kind, reason,
                    wake_at_unix_ms, expires_at_unix_ms, pending_id, leased_by,
                    lease_expires_unix_ms, attempts
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    row.tenant_scope.as_ref(),
                    row.session_id.to_canonical_string(),
                    row.lane_id.to_canonical_string(),
                    row.run_id.to_canonical_string(),
                    row.workflow_kind.as_ref(),
                    row.reason.as_str(),
                    row.wake_at.map(Timestamp::as_unix_ms),
                    row.expires_at.map(Timestamp::as_unix_ms),
                    row.pending_id.as_ref(),
                    row.leased_by.as_deref(),
                    row.lease_expires_at.map(Timestamp::as_unix_ms),
                    i64::from(row.attempts),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_row",
            })?;
            Ok(())
        })
    }

    fn delete(&self, tenant_scope: &str, session_id: SessionId) -> Result<(), WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM finstack_workflow_worker_wake
                 WHERE tenant_scope = ?1 AND session_id = ?2",
                params![tenant_scope, session_id.to_canonical_string()],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_row",
            })?;
            Ok(())
        })
    }

    fn load_due(&self, now: Timestamp, limit: usize) -> Result<Vec<WakeRow>, WorkerError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{WAKE_SELECT}
                 WHERE (leased_by IS NULL
                        OR lease_expires_unix_ms IS NULL
                        OR lease_expires_unix_ms <= ?1)
                   AND (CASE WHEN reason = 'timer'
                             THEN wake_at_unix_ms IS NOT NULL AND wake_at_unix_ms <= ?1
                             ELSE wake_at_unix_ms IS NULL OR wake_at_unix_ms <= ?1
                        END)
                 ORDER BY tenant_scope, session_id
                 LIMIT ?2"
            );
            query_wake_rows(
                conn,
                &sql,
                params![now.as_unix_ms(), i64::try_from(limit).unwrap_or(i64::MAX)],
            )
        })
    }

    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<WakeRow>, WorkerError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{WAKE_SELECT}
                 WHERE tenant_scope = ?1
                 ORDER BY tenant_scope, session_id"
            );
            query_wake_rows(conn, &sql, params![tenant_scope])
        })
    }

    fn contains_interaction(
        &self,
        tenant_scope: &str,
        pending_id: &str,
    ) -> Result<bool, WorkerError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM finstack_workflow_worker_wake
                     WHERE tenant_scope = ?1 AND pending_id = ?2 AND reason = 'interaction'
                 )",
                params![tenant_scope, pending_id],
                |row| row.get(0),
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_query",
            })
        })
    }

    fn try_claim(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let deadline = lease_deadline(now, lease_ttl_ms)?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_wake_begin_immediate",
                })?;
            tx.execute(
                "UPDATE finstack_workflow_worker_wake
                 SET leased_by = ?1, lease_expires_unix_ms = ?2
                 WHERE tenant_scope = ?3 AND session_id = ?4
                   AND (leased_by IS NULL
                        OR lease_expires_unix_ms IS NULL
                        OR lease_expires_unix_ms <= ?5)",
                params![
                    worker_id,
                    deadline.as_unix_ms(),
                    tenant_scope,
                    session_id.to_canonical_string(),
                    now.as_unix_ms(),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_claim",
            })?;
            let won = tx.changes() == 1;
            tx.commit().map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_commit",
            })?;
            Ok(won)
        })
    }

    fn renew(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let deadline = lease_deadline(now, lease_ttl_ms)?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_wake_begin_immediate",
                })?;
            tx.execute(
                "UPDATE finstack_workflow_worker_wake
                 SET leased_by = ?1, lease_expires_unix_ms = ?2
                 WHERE tenant_scope = ?3 AND session_id = ?4 AND leased_by = ?1",
                params![
                    worker_id,
                    deadline.as_unix_ms(),
                    tenant_scope,
                    session_id.to_canonical_string(),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_claim",
            })?;
            let won = tx.changes() == 1;
            tx.commit().map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_commit",
            })?;
            Ok(won)
        })
    }

    fn release(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
    ) -> Result<bool, WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE finstack_workflow_worker_wake
                 SET leased_by = NULL, lease_expires_unix_ms = NULL
                 WHERE tenant_scope = ?1 AND session_id = ?2 AND leased_by = ?3",
                params![tenant_scope, session_id.to_canonical_string(), worker_id,],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_release",
            })?;
            Ok(conn.changes() == 1)
        })
    }

    fn record_failure(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        retry_at: Timestamp,
    ) -> Result<(), WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE finstack_workflow_worker_wake
                 SET attempts = attempts + 1, leased_by = NULL,
                     lease_expires_unix_ms = NULL, wake_at_unix_ms = ?1
                 WHERE tenant_scope = ?2 AND session_id = ?3",
                params![
                    retry_at.as_unix_ms(),
                    tenant_scope,
                    session_id.to_canonical_string(),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_wake_row",
            })?;
            Ok(())
        })
    }
}

/// Convert a `u64` fire count to `i64` for storage, failing closed on
/// overflow.
fn i64_from_fire_count(value: u64) -> Result<i64, WorkerError> {
    i64::try_from(value).map_err(|_| WorkerError::StoreIntegrity {
        code: "sqlite_fire_count",
    })
}

/// Convert a stored `i64` fire count back to `u64`, failing closed on
/// negative values.
fn u64_from_fire_count(value: i64) -> Result<u64, WorkerError> {
    u64::try_from(value).map_err(|_| WorkerError::StoreIntegrity {
        code: "sqlite_fire_count",
    })
}

const FIRE_SELECT: &str = "SELECT tenant_scope, schedule_id, fire_count, fired_unix_ms,
       status, started_session
FROM finstack_workflow_worker_fires";

/// Decode one row from [`FIRE_SELECT`] into a [`FireRow`].
fn decode_fire_row(
    tenant_scope: String,
    schedule_id: String,
    fire_count: i64,
    fired_unix_ms: i64,
    status: &str,
    started_session: Option<String>,
) -> Result<FireRow, WorkerError> {
    let fired_at =
        Timestamp::from_unix_ms(fired_unix_ms).map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_fire_time",
        })?;
    let started_session = started_session
        .map(|value| {
            Id::parse(&value).map_err(|_| WorkerError::StoreIntegrity {
                code: "sqlite_fire_session_id",
            })
        })
        .transpose()?;
    Ok(FireRow {
        tenant_scope: tenant_scope.into(),
        schedule_id: schedule_id.into(),
        fire_count: u64_from_fire_count(fire_count)?,
        fired_at,
        status: FireStatus::parse(status)?,
        started_session,
    })
}

impl FireStore for SqliteWorkerStore {
    fn record_claimed(&self, row: &FireRow) -> Result<(), WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO finstack_workflow_worker_fires (
                    tenant_scope, schedule_id, fire_count, fired_unix_ms,
                    status, started_session
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.tenant_scope.as_ref(),
                    row.schedule_id.as_ref(),
                    i64_from_fire_count(row.fire_count)?,
                    row.fired_at.as_unix_ms(),
                    row.status.as_str(),
                    row.started_session
                        .map(|session| session.to_canonical_string()),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_fire_insert",
            })?;
            Ok(())
        })
    }

    fn mark_started(
        &self,
        tenant_scope: &str,
        schedule_id: &str,
        fire_count: u64,
        started_session: SessionId,
    ) -> Result<FireStartOutcome, WorkerError> {
        let fire_count = i64_from_fire_count(fire_count)?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_fire_begin_immediate",
                })?;
            let existing = tx
                .query_row(
                    "SELECT status, started_session
                     FROM finstack_workflow_worker_fires
                     WHERE tenant_scope = ?1 AND schedule_id = ?2 AND fire_count = ?3",
                    params![tenant_scope, schedule_id, fire_count],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => WorkerError::StoreIntegrity {
                        code: "fire_missing",
                    },
                    _ => WorkerError::StoreUnavailable {
                        code: "sqlite_fire_start",
                    },
                })?;
            let outcome = match (FireStatus::parse(&existing.0)?, existing.1) {
                (FireStatus::Claimed, None) => {
                    tx.execute(
                        "UPDATE finstack_workflow_worker_fires
                         SET status = 'started', started_session = ?4
                         WHERE tenant_scope = ?1 AND schedule_id = ?2 AND fire_count = ?3",
                        params![
                            tenant_scope,
                            schedule_id,
                            fire_count,
                            started_session.to_canonical_string(),
                        ],
                    )
                    .map_err(|_| WorkerError::StoreUnavailable {
                        code: "sqlite_fire_start",
                    })?;
                    FireStartOutcome::Started
                }
                (FireStatus::Started, Some(existing)) => {
                    let existing: SessionId =
                        Id::parse(&existing).map_err(|_| WorkerError::StoreIntegrity {
                            code: "sqlite_fire_session_id",
                        })?;
                    if existing != started_session {
                        return Err(WorkerError::Conflict {
                            code: "fire_start_conflict",
                        });
                    }
                    FireStartOutcome::Idempotent
                }
                _ => {
                    return Err(WorkerError::StoreIntegrity {
                        code: "sqlite_fire_status_session",
                    });
                }
            };
            tx.commit().map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_fire_commit",
            })?;
            Ok(outcome)
        })
    }

    fn load_unstarted(&self, limit: usize) -> Result<Vec<FireRow>, WorkerError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{FIRE_SELECT}
                 WHERE status = 'claimed'
                 ORDER BY tenant_scope, schedule_id, fire_count
                 LIMIT ?1"
            );
            let mut stmt = conn.prepare(&sql).map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_fire_row",
            })?;
            let rows = stmt
                .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                })
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_fire_row",
                })?;
            let mut out = Vec::new();
            for row in rows {
                let (tenant_scope, schedule_id, fire_count, fired_unix_ms, status, started_session) =
                    row.map_err(|_| WorkerError::StoreIntegrity {
                        code: "sqlite_fire_row",
                    })?;
                out.push(decode_fire_row(
                    tenant_scope,
                    schedule_id,
                    fire_count,
                    fired_unix_ms,
                    &status,
                    started_session,
                )?);
            }
            Ok(out)
        })
    }

    fn purge_started(&self, before: Timestamp, limit: usize) -> Result<usize, WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM finstack_workflow_worker_fires
                 WHERE rowid IN (
                     SELECT rowid FROM finstack_workflow_worker_fires
                     WHERE status = 'started' AND fired_unix_ms < ?1
                     ORDER BY fired_unix_ms, tenant_scope, schedule_id, fire_count
                     LIMIT ?2
                 )",
                params![
                    before.as_unix_ms(),
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_fire_purge",
            })?;
            Ok(usize::try_from(conn.changes()).unwrap_or(usize::MAX))
        })
    }
}

const INBOX_SELECT: &str = "SELECT tenant_scope, session_id, pending_id, kind, payload,
       payload_digest, received_unix_ms
FROM finstack_workflow_worker_inbox";

/// Raw columns of one [`INBOX_SELECT`] row, as read from sqlite before
/// decoding. The dead-letter table shares this prefix.
struct RawInboxRow {
    tenant_scope: String,
    session_id: String,
    pending_id: String,
    kind: String,
    payload: Vec<u8>,
    payload_digest: String,
    received_unix_ms: i64,
}

impl RawInboxRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            tenant_scope: row.get(0)?,
            session_id: row.get(1)?,
            pending_id: row.get(2)?,
            kind: row.get(3)?,
            payload: row.get(4)?,
            payload_digest: row.get(5)?,
            received_unix_ms: row.get(6)?,
        })
    }
}

/// Decode one [`RawInboxRow`] into an [`InboxRow`].
fn decode_inbox_row(raw: RawInboxRow) -> Result<InboxRow, WorkerError> {
    let session_id: SessionId =
        Id::parse(&raw.session_id).map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_inbox_row",
        })?;
    let received_at =
        Timestamp::from_unix_ms(raw.received_unix_ms).map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_inbox_row",
        })?;
    if raw.payload.len() > MAX_INBOX_PAYLOAD_BYTES {
        return Err(WorkerError::StoreIntegrity {
            code: "sqlite_inbox_payload_size",
        });
    }
    let payload_digest =
        Digest::from_hex(&raw.payload_digest).map_err(|_| WorkerError::StoreIntegrity {
            code: "sqlite_inbox_digest",
        })?;
    let row = InboxRow {
        tenant_scope: raw.tenant_scope.into(),
        session_id,
        pending_id: raw.pending_id.into(),
        kind: InboxKind::parse(&raw.kind)?,
        payload: Arc::from(raw.payload.into_boxed_slice()),
        payload_digest,
        received_at,
    };
    if !row.digest_is_valid() {
        return Err(WorkerError::StoreIntegrity {
            code: "sqlite_inbox_digest",
        });
    }
    Ok(row)
}

impl InboxStore for SqliteWorkerStore {
    fn insert(&self, row: &InboxRow) -> Result<InboxInsertOutcome, WorkerError> {
        if !row.digest_is_valid() || row.payload.len() > MAX_INBOX_PAYLOAD_BYTES {
            return Err(WorkerError::StoreIntegrity {
                code: "inbox_digest",
            });
        }
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_begin_immediate",
                })?;
            tx.execute(
                "INSERT OR IGNORE INTO finstack_workflow_worker_inbox (
                    tenant_scope, session_id, pending_id, kind, payload, payload_digest,
                    received_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.tenant_scope.as_ref(),
                    row.session_id.to_canonical_string(),
                    row.pending_id.as_ref(),
                    row.kind.as_str(),
                    row.payload.as_ref(),
                    row.payload_digest.to_hex(),
                    row.received_at.as_unix_ms(),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_inbox_insert",
            })?;
            let inserted = tx.changes() == 1;
            let outcome = if inserted {
                InboxInsertOutcome::Inserted
            } else {
                let existing = tx
                    .query_row(
                        "SELECT kind, payload_digest
                         FROM finstack_workflow_worker_inbox
                         WHERE tenant_scope = ?1 AND session_id = ?2 AND pending_id = ?3",
                        params![
                            row.tenant_scope.as_ref(),
                            row.session_id.to_canonical_string(),
                            row.pending_id.as_ref(),
                        ],
                        |stored| Ok((stored.get::<_, String>(0)?, stored.get::<_, String>(1)?)),
                    )
                    .map_err(|_| WorkerError::StoreUnavailable {
                        code: "sqlite_inbox_query",
                    })?;
                if existing.0 != row.kind.as_str() || existing.1 != row.payload_digest.to_hex() {
                    return Err(WorkerError::Conflict {
                        code: "inbox_conflict",
                    });
                }
                InboxInsertOutcome::Idempotent
            };
            tx.commit().map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_inbox_commit",
            })?;
            Ok(outcome)
        })
    }

    fn load(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
    ) -> Result<Option<InboxRow>, WorkerError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{INBOX_SELECT}
                 WHERE tenant_scope = ?1 AND session_id = ?2 AND pending_id = ?3"
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_query",
                })?;
            stmt.query_row(
                params![tenant_scope, session_id.to_canonical_string(), pending_id],
                RawInboxRow::read,
            )
            .optional()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_inbox_query",
            })?
            .map(decode_inbox_row)
            .transpose()
        })
    }

    fn load_batch(&self, limit: usize) -> Result<Vec<InboxRow>, WorkerError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{INBOX_SELECT}
                 ORDER BY tenant_scope, session_id, pending_id
                 LIMIT ?1"
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_query",
                })?;
            let rows = stmt
                .query_map(
                    params![i64::try_from(limit).unwrap_or(i64::MAX)],
                    RawInboxRow::read,
                )
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_query",
                })?;
            rows.map(|row| {
                decode_inbox_row(row.map_err(|_| WorkerError::StoreIntegrity {
                    code: "sqlite_inbox_row",
                })?)
            })
            .collect()
        })
    }

    fn delete_if_digest(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
        expected_digest: Digest,
    ) -> Result<bool, WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM finstack_workflow_worker_inbox
                 WHERE tenant_scope = ?1 AND session_id = ?2 AND pending_id = ?3
                   AND payload_digest = ?4",
                params![
                    tenant_scope,
                    session_id.to_canonical_string(),
                    pending_id,
                    expected_digest.to_hex(),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_inbox_delete",
            })?;
            Ok(conn.changes() == 1)
        })
    }

    fn dead_letter(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
        expected_digest: Digest,
        reason_code: &str,
        rejected_at: Timestamp,
    ) -> Result<bool, WorkerError> {
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_begin_immediate",
                })?;
            tx.execute(
                "INSERT INTO finstack_workflow_worker_dead_letters (
                    tenant_scope, session_id, pending_id, kind, payload, payload_digest,
                    received_unix_ms, reason_code, rejected_unix_ms
                 )
                 SELECT tenant_scope, session_id, pending_id, kind, payload, payload_digest,
                        received_unix_ms, ?5, ?6
                 FROM finstack_workflow_worker_inbox
                 WHERE tenant_scope = ?1 AND session_id = ?2 AND pending_id = ?3
                   AND payload_digest = ?4",
                params![
                    tenant_scope,
                    session_id.to_canonical_string(),
                    pending_id,
                    expected_digest.to_hex(),
                    reason_code,
                    rejected_at.as_unix_ms(),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_inbox_dead_letter",
            })?;
            let moved = tx.changes() == 1;
            if moved {
                tx.execute(
                    "DELETE FROM finstack_workflow_worker_inbox
                     WHERE tenant_scope = ?1 AND session_id = ?2 AND pending_id = ?3
                       AND payload_digest = ?4",
                    params![
                        tenant_scope,
                        session_id.to_canonical_string(),
                        pending_id,
                        expected_digest.to_hex(),
                    ],
                )
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_dead_letter",
                })?;
            }
            tx.commit().map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_inbox_commit",
            })?;
            Ok(moved)
        })
    }

    fn load_dead_letters(&self, limit: usize) -> Result<Vec<DeadLetterRow>, WorkerError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT tenant_scope, session_id, pending_id, kind, payload,
                            payload_digest, received_unix_ms, reason_code, rejected_unix_ms
                     FROM finstack_workflow_worker_dead_letters
                     ORDER BY rejected_unix_ms, id
                     LIMIT ?1",
                )
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_dead_letter_query",
                })?;
            let rows = stmt
                .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                    Ok((
                        RawInboxRow::read(row)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, i64>(8)?,
                    ))
                })
                .map_err(|_| WorkerError::StoreUnavailable {
                    code: "sqlite_inbox_dead_letter_query",
                })?;
            rows.map(|row| {
                let (raw, reason, rejected) = row.map_err(|_| WorkerError::StoreIntegrity {
                    code: "sqlite_inbox_dead_letter_row",
                })?;
                let rejected_at =
                    Timestamp::from_unix_ms(rejected).map_err(|_| WorkerError::StoreIntegrity {
                        code: "sqlite_inbox_dead_letter_row",
                    })?;
                Ok(DeadLetterRow {
                    response: decode_inbox_row(raw)?,
                    reason_code: reason.into(),
                    rejected_at,
                })
            })
            .collect()
        })
    }

    fn purge_dead_letters(&self, before: Timestamp, limit: usize) -> Result<usize, WorkerError> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM finstack_workflow_worker_dead_letters
                 WHERE id IN (
                     SELECT id FROM finstack_workflow_worker_dead_letters
                     WHERE rejected_unix_ms < ?1
                     ORDER BY rejected_unix_ms, id
                     LIMIT ?2
                 )",
                params![
                    before.as_unix_ms(),
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ],
            )
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "sqlite_inbox_dead_letter_purge",
            })?;
            Ok(usize::try_from(conn.changes()).unwrap_or(usize::MAX))
        })
    }
}

/// True when `path` names an in-memory sqlite database (`:memory:` or a
/// `mode=memory` URI), which must not receive the WAL pragma.
///
/// Public so sibling adapter stores that share a database file (for example
/// the HITL inbox) apply the same detection rule instead of keeping their
/// own copy of it.
#[must_use]
pub fn is_memory_sqlite_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text == ":memory:" || text.contains("mode=memory")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::Timestamp;

    use super::SqliteWorkerStore;
    use crate::wake::{WakeIndexStore, WakeReason, WakeRow};

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        finstack_ai_kernel::Id::from_bytes(bytes)
    }

    fn timer_row(tenant: &str, session: u64, due_ms: i64) -> WakeRow {
        WakeRow {
            tenant_scope: Arc::from(tenant),
            session_id: id(session),
            lane_id: id(2),
            run_id: id(3),
            workflow_kind: Arc::from("research"),
            reason: WakeReason::Timer,
            wake_at: Some(ts(due_ms)),
            expires_at: None,
            pending_id: Arc::from("effect-1"),
            leased_by: None,
            lease_expires_at: None,
            attempts: 0,
        }
    }

    #[test]
    fn two_sqlite_stores_claim_exactly_one_lease() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("worker.sqlite");
        let first = SqliteWorkerStore::try_open(&path).expect("first");
        let second = SqliteWorkerStore::try_open(&path).expect("second");
        first
            .upsert(&timer_row("tenant-a", 1, 1_000))
            .expect("upsert");
        let won_first = first
            .try_claim("tenant-a", id(1), "worker-a", ts(1_500), 60_000)
            .expect("first");
        let won_second = second
            .try_claim("tenant-a", id(1), "worker-b", ts(1_500), 60_000)
            .expect("second");
        assert_eq!(usize::from(won_first) + usize::from(won_second), 1);
    }

    #[test]
    fn sqlite_round_trips_every_column() {
        let dir = tempfile::tempdir().expect("dir");
        let store = SqliteWorkerStore::try_open(dir.path().join("w.sqlite")).expect("open");
        let mut row = timer_row("tenant-a", 1, 2_000);
        row.attempts = 3;
        store.upsert(&row).expect("upsert");
        let loaded = store.load_tenant("tenant-a").expect("load");
        assert_eq!(loaded, vec![row]);
    }

    /// The interaction deadline round-trips and never gates dueness: it is
    /// read by the tick after the fact, not by the store's due predicate.
    #[test]
    fn an_interaction_deadline_round_trips_without_gating_dueness() {
        let dir = tempfile::tempdir().expect("dir");
        let store = SqliteWorkerStore::try_open(dir.path().join("w.sqlite")).expect("open");

        let mut row = timer_row("tenant-a", 7, 0);
        row.reason = WakeReason::Interaction;
        row.wake_at = None;
        row.expires_at = Some(ts(9_000));
        store.upsert(&row).expect("upsert");

        let due = store.load_due(ts(1_000), 10).expect("load_due");
        assert_eq!(
            due,
            vec![row.clone()],
            "still inbox-driven before the deadline"
        );
        assert_eq!(
            store.load_tenant("tenant-a").expect("tenant"),
            vec![row],
            "and the deadline survives the round trip"
        );
    }

    /// Historical versionless adapter tables are rejected with an explicit
    /// reset instruction instead of being guessed forward in place.
    #[test]
    fn an_unversioned_worker_schema_requires_a_fresh_database() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("old.sqlite");
        let legacy = rusqlite::Connection::open(&path).expect("open");
        legacy
            .execute_batch(
                "CREATE TABLE finstack_workflow_worker_wake (
                   tenant_scope TEXT NOT NULL,
                   session_id TEXT NOT NULL,
                   lane_id TEXT NOT NULL,
                   run_id TEXT NOT NULL,
                   workflow_kind TEXT NOT NULL,
                   reason TEXT NOT NULL,
                   wake_at_unix_ms INTEGER,
                   pending_id TEXT NOT NULL,
                   leased_by TEXT,
                   lease_expires_unix_ms INTEGER,
                   attempts INTEGER NOT NULL,
                   PRIMARY KEY (tenant_scope, session_id)
                 );",
            )
            .expect("legacy schema");
        drop(legacy);

        let Err(error) = SqliteWorkerStore::try_open(&path) else {
            panic!("legacy schema must fail closed");
        };
        assert_eq!(error.code(), "workflow_schema_reset_required");
    }

    /// `load_due`'s SQL predicate must agree with [`crate::wake::wake_due`]
    /// row for row: a freshly parked inbox-driven row (`wake_at: None`) is
    /// immediately due, one backed off by `record_failure` sleeps until its
    /// retry instant, and a timer row without a `wake_at` is never due.
    #[test]
    fn sqlite_dueness_matches_the_in_memory_predicate() {
        let dir = tempfile::tempdir().expect("dir");
        let store = SqliteWorkerStore::try_open(dir.path().join("w.sqlite")).expect("open");

        let mut fresh = timer_row("tenant-a", 1, 0);
        fresh.reason = WakeReason::Deferred;
        fresh.wake_at = None;
        let mut backed_off = timer_row("tenant-a", 2, 0);
        backed_off.reason = WakeReason::Interaction;
        backed_off.wake_at = Some(ts(5_000));
        let due_timer = timer_row("tenant-a", 3, 1_000);
        let mut timerless = timer_row("tenant-a", 4, 0);
        timerless.wake_at = None;

        for row in [&fresh, &backed_off, &due_timer, &timerless] {
            store.upsert(row).expect("upsert");
        }

        let now = ts(2_000);
        let due = store.load_due(now, 10).expect("load_due");
        for row in [&fresh, &backed_off, &due_timer, &timerless] {
            assert_eq!(
                due.contains(row),
                crate::wake::wake_due(row, now),
                "sqlite and in-memory dueness disagree for {:?}",
                row.session_id,
            );
        }
        assert_eq!(due.len(), 2, "the fresh non-timer row and the due timer");

        // Past the backoff, the inbox-driven row rejoins the due set.
        let later = ts(6_000);
        let due_later = store.load_due(later, 10).expect("load_due");
        assert!(due_later.contains(&backed_off));
        assert!(crate::wake::wake_due(&backed_off, later));
    }

    #[test]
    fn expired_lease_is_reclaimed_and_renew_requires_holder() {
        let dir = tempfile::tempdir().expect("dir");
        let store = SqliteWorkerStore::try_open(dir.path().join("w.sqlite")).expect("open");
        store
            .upsert(&timer_row("tenant-a", 1, 1_000))
            .expect("upsert");
        assert!(
            store
                .try_claim("tenant-a", id(1), "worker-a", ts(1_000), 1_000)
                .expect("claim")
        );
        assert!(
            store
                .renew("tenant-a", id(1), "worker-a", ts(1_500), 1_000)
                .expect("holder renews")
        );
        assert!(
            !store
                .renew("tenant-a", id(1), "worker-b", ts(1_500), 1_000)
                .expect("stranger cannot renew")
        );
        assert!(
            store
                .try_claim("tenant-a", id(1), "worker-b", ts(9_000), 1_000)
                .expect("expired lease reclaimed")
        );
    }
}
