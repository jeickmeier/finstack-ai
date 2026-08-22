//! Sqlite-backed inbox for the HITL router battery.
//!
//! [`SqliteHitlStore`] owns exactly one table, `finstack_workflow_hitl_inbox`,
//! and may share its database file with `SqliteWorkerStore` (from
//! `finstack-ai-workflow-worker`) or other adapter stores. It owns a separate
//! schema-version table and never changes the journal's `PRAGMA user_version`.
//!
//! Every failure code this store raises is prefixed `sqlite_hitl_`, matching
//! the backend-prefix scheme [`crate::MemoryHitlStore`] uses (`memory_hitl_`)
//! and the worker's own sqlite store (`sqlite_wake_`, `sqlite_worker_`). The
//! prefix tells a caller which backend failed; the backend-agnostic codes
//! raised by the router itself (`hitl_locator`, `hitl_request_decode`, …)
//! deliberately carry no prefix. These codes stabilize at publish.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AuthorizationEvidence, Id, LaneId, PrincipalRef, RunId, SessionId, Timestamp,
};
use finstack_ai_workflow_worker::is_memory_sqlite_path;
use rusqlite::{Connection, params};

use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus, InteractionSummary};
use crate::store::{HitlInboxStore, InteractionTransition};

const HITL_SCHEMA_VERSION: i64 = 1;

const HITL_DDL: &str = "
CREATE TABLE IF NOT EXISTS finstack_workflow_hitl_schema (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL
);
INSERT OR IGNORE INTO finstack_workflow_hitl_schema (singleton, version) VALUES (1, 1);
CREATE TABLE IF NOT EXISTS finstack_workflow_hitl_inbox (
    tenant_scope       TEXT    NOT NULL,
    session_id         TEXT    NOT NULL,
    lane_id            TEXT    NOT NULL,
    run_id             TEXT    NOT NULL,
    interaction_id     TEXT    NOT NULL,
    kind               TEXT    NOT NULL,
    requested_at_unix_ms  INTEGER NOT NULL,
    expires_at_unix_ms    INTEGER,
    request            BLOB    NOT NULL,
    accepted_principal BLOB    NOT NULL,
    accepted_evidence  BLOB    NOT NULL,
    status             TEXT    NOT NULL,
    resolved_by        TEXT,
    outcome_code       TEXT,
    updated_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (tenant_scope, interaction_id)
);
CREATE INDEX IF NOT EXISTS finstack_workflow_hitl_inbox_active
ON finstack_workflow_hitl_inbox (status, requested_at_unix_ms);
";

/// SQLite-backed inbox. May share a database file with `SqliteWorkerStore`;
/// owns only its own table.
///
/// A single connection is guarded by a mutex so callers may share one store
/// across threads; separate processes coordinate through sqlite's own file
/// locking.
pub struct SqliteHitlStore {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl SqliteHitlStore {
    /// Open or create the inbox table in `path`.
    ///
    /// # Errors
    ///
    /// Returns [`HitlError::StoreUnavailable`] when the file cannot be
    /// opened, `busy_timeout`/WAL cannot be configured, or the schema cannot
    /// be created.
    pub fn try_open(path: impl AsRef<Path>) -> Result<Self, HitlError> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|_| HitlError::StoreUnavailable {
            code: "sqlite_hitl_open",
        })?;
        conn.busy_timeout(Duration::from_secs(1))
            .map_err(|_| HitlError::StoreUnavailable {
                code: "sqlite_hitl_busy_timeout",
            })?;
        if !is_memory_sqlite_path(&path) {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| HitlError::StoreUnavailable {
                    code: "sqlite_hitl_wal",
                })?;
        }
        let has_schema = table_exists(&conn, "finstack_workflow_hitl_schema")?;
        if !has_schema && table_exists(&conn, "finstack_workflow_hitl_inbox")? {
            return Err(HitlError::StoreIntegrity {
                code: "hitl_schema_reset_required",
            });
        }
        conn.execute_batch(HITL_DDL)
            .map_err(|_| HitlError::StoreUnavailable {
                code: "sqlite_hitl_schema",
            })?;
        let version: i64 = conn
            .query_row(
                "SELECT version FROM finstack_workflow_hitl_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| HitlError::StoreIntegrity {
                code: "hitl_schema_version",
            })?;
        if version != HITL_SCHEMA_VERSION {
            return Err(HitlError::StoreIntegrity {
                code: "hitl_schema_version",
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
        body: impl FnOnce(&Connection) -> Result<T, HitlError>,
    ) -> Result<T, HitlError> {
        let conn = self.conn.lock().map_err(|_| HitlError::StoreUnavailable {
            code: "sqlite_hitl_lock_poisoned",
        })?;
        body(&conn)
    }
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, HitlError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
    .map_err(|_| HitlError::StoreUnavailable {
        code: "sqlite_hitl_schema",
    })
}

/// Raw columns for one inbox row, as read from sqlite before decoding.
struct RawInteractionRow {
    tenant_scope: String,
    session_id: String,
    lane_id: String,
    run_id: String,
    interaction_id: String,
    kind: String,
    requested_at_unix_ms: i64,
    expires_at_unix_ms: Option<i64>,
    request: Vec<u8>,
    accepted_principal: Vec<u8>,
    accepted_evidence: Vec<u8>,
    status: String,
    resolved_by: Option<String>,
    outcome_code: Option<String>,
    updated_at_unix_ms: i64,
}

/// Decode one row, mapping parse failures to [`HitlError::StoreIntegrity`].
fn decode_row(raw: RawInteractionRow) -> Result<InteractionRow, HitlError> {
    let session_id: SessionId =
        Id::parse(&raw.session_id).map_err(|_| HitlError::StoreIntegrity {
            code: "sqlite_hitl_id",
        })?;
    let lane_id: LaneId = Id::parse(&raw.lane_id).map_err(|_| HitlError::StoreIntegrity {
        code: "sqlite_hitl_id",
    })?;
    let run_id: RunId = Id::parse(&raw.run_id).map_err(|_| HitlError::StoreIntegrity {
        code: "sqlite_hitl_id",
    })?;
    let requested_at = Timestamp::from_unix_ms(raw.requested_at_unix_ms).map_err(|_| {
        HitlError::StoreIntegrity {
            code: "sqlite_hitl_time",
        }
    })?;
    let expires_at = raw
        .expires_at_unix_ms
        .map(Timestamp::from_unix_ms)
        .transpose()
        .map_err(|_| HitlError::StoreIntegrity {
            code: "sqlite_hitl_time",
        })?;
    let updated_at =
        Timestamp::from_unix_ms(raw.updated_at_unix_ms).map_err(|_| HitlError::StoreIntegrity {
            code: "sqlite_hitl_time",
        })?;
    let status = InteractionStatus::parse(&raw.status).map_err(|_| HitlError::StoreIntegrity {
        code: "sqlite_hitl_status",
    })?;
    let accepted_principal: PrincipalRef = serde_json::from_slice(&raw.accepted_principal)
        .map_err(|_| HitlError::StoreIntegrity {
            code: "sqlite_hitl_security",
        })?;
    let accepted_evidence: AuthorizationEvidence = serde_json::from_slice(&raw.accepted_evidence)
        .map_err(|_| HitlError::StoreIntegrity {
        code: "sqlite_hitl_security",
    })?;
    Ok(InteractionRow {
        tenant_scope: raw.tenant_scope.into(),
        session_id,
        lane_id,
        run_id,
        interaction_id: raw.interaction_id.into(),
        kind: raw.kind.into(),
        requested_at,
        expires_at,
        request: Arc::from(raw.request.into_boxed_slice()),
        accepted_principal,
        accepted_evidence,
        status,
        resolved_by: raw.resolved_by.map(Into::into),
        outcome_code: raw.outcome_code.map(Into::into),
        updated_at,
    })
}

const ROW_SELECT: &str = "SELECT tenant_scope, session_id, lane_id, run_id, interaction_id, kind,
       requested_at_unix_ms, expires_at_unix_ms, request, accepted_principal,
       accepted_evidence, status, resolved_by, outcome_code, updated_at_unix_ms
FROM finstack_workflow_hitl_inbox";

/// Query rows matching `sql`/`args`, decoding each into an [`InteractionRow`].
fn query_rows(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<InteractionRow>, HitlError> {
    let mut stmt = conn.prepare(sql).map_err(|_| HitlError::StoreUnavailable {
        code: "sqlite_hitl_row",
    })?;
    let rows = stmt
        .query_map(args, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Vec<u8>>(8)?,
                row.get::<_, Vec<u8>>(9)?,
                row.get::<_, Vec<u8>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, i64>(14)?,
            ))
        })
        .map_err(|_| HitlError::StoreUnavailable {
            code: "sqlite_hitl_row",
        })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            tenant_scope,
            session_id,
            lane_id,
            run_id,
            interaction_id,
            kind,
            requested_at_unix_ms,
            expires_at_unix_ms,
            request,
            accepted_principal,
            accepted_evidence,
            status,
            resolved_by,
            outcome_code,
            updated_at_unix_ms,
        ) = row.map_err(|_| HitlError::StoreIntegrity {
            code: "sqlite_hitl_row",
        })?;
        out.push(decode_row(RawInteractionRow {
            tenant_scope,
            session_id,
            lane_id,
            run_id,
            interaction_id,
            kind,
            requested_at_unix_ms,
            expires_at_unix_ms,
            request,
            accepted_principal,
            accepted_evidence,
            status,
            resolved_by,
            outcome_code,
            updated_at_unix_ms,
        })?);
    }
    Ok(out)
}

impl HitlInboxStore for SqliteHitlStore {
    fn upsert(&self, row: &InteractionRow) -> Result<(), HitlError> {
        let accepted_principal =
            serde_json::to_vec(&row.accepted_principal).map_err(|_| HitlError::StoreIntegrity {
                code: "sqlite_hitl_security",
            })?;
        let accepted_evidence =
            serde_json::to_vec(&row.accepted_evidence).map_err(|_| HitlError::StoreIntegrity {
                code: "sqlite_hitl_security",
            })?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO finstack_workflow_hitl_inbox (
                    tenant_scope, session_id, lane_id, run_id, interaction_id, kind,
                    requested_at_unix_ms, expires_at_unix_ms, request, accepted_principal,
                    accepted_evidence, status, resolved_by, outcome_code, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT (tenant_scope, interaction_id) DO UPDATE SET
                    session_id = excluded.session_id,
                    lane_id = excluded.lane_id,
                    run_id = excluded.run_id,
                    kind = excluded.kind,
                    requested_at_unix_ms = excluded.requested_at_unix_ms,
                    expires_at_unix_ms = excluded.expires_at_unix_ms,
                    request = excluded.request,
                    accepted_principal = CASE
                        WHEN finstack_workflow_hitl_inbox.status = 'closed'
                        THEN excluded.accepted_principal
                        ELSE finstack_workflow_hitl_inbox.accepted_principal
                    END,
                    accepted_evidence = CASE
                        WHEN finstack_workflow_hitl_inbox.status = 'closed'
                        THEN excluded.accepted_evidence
                        ELSE finstack_workflow_hitl_inbox.accepted_evidence
                    END,
                    status = CASE
                        WHEN finstack_workflow_hitl_inbox.status = 'closed'
                        THEN excluded.status
                        ELSE finstack_workflow_hitl_inbox.status
                    END,
                    resolved_by = CASE
                        WHEN finstack_workflow_hitl_inbox.status = 'closed'
                        THEN excluded.resolved_by
                        ELSE finstack_workflow_hitl_inbox.resolved_by
                    END,
                    outcome_code = CASE
                        WHEN finstack_workflow_hitl_inbox.status = 'closed'
                        THEN excluded.outcome_code
                        ELSE finstack_workflow_hitl_inbox.outcome_code
                    END,
                    updated_at_unix_ms = CASE
                        WHEN finstack_workflow_hitl_inbox.status = 'closed'
                        THEN excluded.updated_at_unix_ms
                        ELSE finstack_workflow_hitl_inbox.updated_at_unix_ms
                    END",
                params![
                    row.tenant_scope.as_ref(),
                    row.session_id.to_canonical_string(),
                    row.lane_id.to_canonical_string(),
                    row.run_id.to_canonical_string(),
                    row.interaction_id.as_ref(),
                    row.kind.as_ref(),
                    row.requested_at.as_unix_ms(),
                    row.expires_at.map(Timestamp::as_unix_ms),
                    row.request.as_ref(),
                    accepted_principal,
                    accepted_evidence,
                    row.status.as_str(),
                    row.resolved_by.as_deref(),
                    row.outcome_code.as_deref(),
                    row.updated_at.as_unix_ms(),
                ],
            )
            .map_err(|_| HitlError::StoreUnavailable {
                code: "sqlite_hitl_upsert",
            })?;
            Ok(())
        })
    }

    fn load(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
    ) -> Result<Option<InteractionRow>, HitlError> {
        self.with_conn(|conn| {
            let sql = format!("{ROW_SELECT} WHERE tenant_scope = ?1 AND interaction_id = ?2");
            let rows = query_rows(conn, &sql, params![tenant_scope, interaction_id])?;
            Ok(rows.into_iter().next())
        })
    }

    fn load_open(
        &self,
        tenant_scope: &str,
        limit: usize,
    ) -> Result<Vec<InteractionRow>, HitlError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{ROW_SELECT}
                 WHERE tenant_scope = ?1 AND status IN ('open', 'rejected')
                 ORDER BY requested_at_unix_ms, interaction_id
                 LIMIT ?2"
            );
            query_rows(
                conn,
                &sql,
                params![tenant_scope, i64::try_from(limit).unwrap_or(i64::MAX)],
            )
        })
    }

    fn load_active(&self, limit: usize) -> Result<Vec<InteractionRow>, HitlError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{ROW_SELECT}
                 WHERE status IN ('open', 'buffered')
                 ORDER BY requested_at_unix_ms, interaction_id
                 LIMIT ?1"
            );
            query_rows(
                conn,
                &sql,
                params![i64::try_from(limit).unwrap_or(i64::MAX)],
            )
        })
    }

    fn load_active_summaries(&self, limit: usize) -> Result<Vec<InteractionSummary>, HitlError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT tenant_scope, interaction_id, status, resolved_by, outcome_code
                     FROM finstack_workflow_hitl_inbox
                     WHERE status IN ('open', 'buffered')
                     ORDER BY requested_at_unix_ms, interaction_id
                     LIMIT ?1",
                )
                .map_err(|_| HitlError::StoreUnavailable {
                    code: "sqlite_hitl_row",
                })?;
            let rows = stmt
                .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })
                .map_err(|_| HitlError::StoreUnavailable {
                    code: "sqlite_hitl_row",
                })?;
            let mut out = Vec::new();
            for row in rows {
                let (tenant_scope, interaction_id, status, resolved_by, outcome_code) = row
                    .map_err(|_| HitlError::StoreIntegrity {
                        code: "sqlite_hitl_row",
                    })?;
                let status =
                    InteractionStatus::parse(&status).map_err(|_| HitlError::StoreIntegrity {
                        code: "sqlite_hitl_status",
                    })?;
                out.push(InteractionSummary {
                    tenant_scope: tenant_scope.into(),
                    interaction_id: interaction_id.into(),
                    status,
                    resolved_by: resolved_by.map(Into::into),
                    outcome_code: outcome_code.map(Into::into),
                });
            }
            Ok(out)
        })
    }

    fn transition(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        transition: InteractionTransition<'_>,
    ) -> Result<bool, HitlError> {
        self.with_conn(|conn| {
            let changed = conn
                .execute(
                    "UPDATE finstack_workflow_hitl_inbox
                     SET status = ?1, resolved_by = ?2, outcome_code = ?3,
                         updated_at_unix_ms = ?4
                     WHERE tenant_scope = ?5 AND interaction_id = ?6 AND status = ?7",
                    params![
                        transition.next.as_str(),
                        transition.resolved_by,
                        transition.outcome_code,
                        transition.updated_at.as_unix_ms(),
                        tenant_scope,
                        interaction_id,
                        transition.expected.as_str(),
                    ],
                )
                .map_err(|_| HitlError::StoreUnavailable {
                    code: "sqlite_hitl_status_update",
                })?;
            if changed == 0 {
                let exists: bool = conn
                    .query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM finstack_workflow_hitl_inbox
                             WHERE tenant_scope = ?1 AND interaction_id = ?2
                         )",
                        params![tenant_scope, interaction_id],
                        |row| row.get(0),
                    )
                    .map_err(|_| HitlError::StoreUnavailable {
                        code: "sqlite_hitl_status_update",
                    })?;
                if !exists {
                    return Err(HitlError::UnknownInteraction);
                }
            }
            Ok(changed == 1)
        })
    }
}
