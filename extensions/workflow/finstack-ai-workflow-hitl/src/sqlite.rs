//! Sqlite-backed inbox for the HITL router battery.
//!
//! [`SqliteHitlStore`] owns exactly one table, `finstack_workflow_hitl_inbox`,
//! and may share its database file with `SqliteWorkerStore` (from
//! `finstack-ai-workflow-worker`) or other adapter stores. Like the worker's
//! tables, this one is deliberately **versionless**: it carries no `PRAGMA
//! user_version` guard and is not part of the kernel journal's schema
//! version. It holds a hint only, so a binary that does not understand a
//! column simply ignores it. Future changes must therefore be **additive and
//! nullable**, and must never repurpose or drop an existing column.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{Id, LaneId, RunId, SessionId, Timestamp};
use rusqlite::{Connection, params};

use crate::error::HitlError;
use crate::row::{InteractionRow, InteractionStatus};
use crate::store::HitlInboxStore;

const HITL_DDL: &str = "
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
    status             TEXT    NOT NULL,
    resolved_by        TEXT,
    updated_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (tenant_scope, interaction_id)
);
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
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HitlError> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|_| HitlError::StoreUnavailable {
            code: "hitl_open",
        })?;
        conn.busy_timeout(Duration::from_secs(1))
            .map_err(|_| HitlError::StoreUnavailable {
                code: "hitl_busy_timeout",
            })?;
        if !is_memory_path(&path) {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| HitlError::StoreUnavailable { code: "hitl_wal" })?;
        }
        conn.execute_batch(HITL_DDL)
            .map_err(|_| HitlError::StoreUnavailable {
                code: "hitl_schema",
            })?;
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
            code: "hitl_lock_poisoned",
        })?;
        body(&conn)
    }
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
    status: String,
    resolved_by: Option<String>,
    updated_at_unix_ms: i64,
}

/// Decode one row, mapping parse failures to [`HitlError::StoreIntegrity`].
fn decode_row(raw: RawInteractionRow) -> Result<InteractionRow, HitlError> {
    let session_id: SessionId =
        Id::parse(&raw.session_id).map_err(|_| HitlError::StoreIntegrity { code: "hitl_id" })?;
    let lane_id: LaneId =
        Id::parse(&raw.lane_id).map_err(|_| HitlError::StoreIntegrity { code: "hitl_id" })?;
    let run_id: RunId =
        Id::parse(&raw.run_id).map_err(|_| HitlError::StoreIntegrity { code: "hitl_id" })?;
    let requested_at = Timestamp::from_unix_ms(raw.requested_at_unix_ms)
        .map_err(|_| HitlError::StoreIntegrity { code: "hitl_time" })?;
    let expires_at = raw
        .expires_at_unix_ms
        .map(Timestamp::from_unix_ms)
        .transpose()
        .map_err(|_| HitlError::StoreIntegrity { code: "hitl_time" })?;
    let updated_at = Timestamp::from_unix_ms(raw.updated_at_unix_ms)
        .map_err(|_| HitlError::StoreIntegrity { code: "hitl_time" })?;
    let status = InteractionStatus::parse(&raw.status)?;
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
        status,
        resolved_by: raw.resolved_by.map(Into::into),
        updated_at,
    })
}

const ROW_SELECT: &str = "SELECT tenant_scope, session_id, lane_id, run_id, interaction_id, kind,
       requested_at_unix_ms, expires_at_unix_ms, request, status, resolved_by, updated_at_unix_ms
FROM finstack_workflow_hitl_inbox";

/// Query rows matching `sql`/`args`, decoding each into an [`InteractionRow`].
fn query_rows(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<InteractionRow>, HitlError> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|_| HitlError::StoreUnavailable { code: "hitl_row" })?;
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
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, i64>(11)?,
            ))
        })
        .map_err(|_| HitlError::StoreUnavailable { code: "hitl_row" })?;
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
            status,
            resolved_by,
            updated_at_unix_ms,
        ) = row.map_err(|_| HitlError::StoreIntegrity { code: "hitl_row" })?;
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
            status,
            resolved_by,
            updated_at_unix_ms,
        })?);
    }
    Ok(out)
}

impl HitlInboxStore for SqliteHitlStore {
    fn upsert(&self, row: &InteractionRow) -> Result<(), HitlError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO finstack_workflow_hitl_inbox (
                    tenant_scope, session_id, lane_id, run_id, interaction_id, kind,
                    requested_at_unix_ms, expires_at_unix_ms, request, status, resolved_by,
                    updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                    row.status.as_str(),
                    row.resolved_by.as_deref(),
                    row.updated_at.as_unix_ms(),
                ],
            )
            .map_err(|_| HitlError::StoreUnavailable {
                code: "hitl_upsert",
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

    fn load_open(&self, tenant_scope: &str) -> Result<Vec<InteractionRow>, HitlError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{ROW_SELECT}
                 WHERE tenant_scope = ?1 AND status = 'open'
                 ORDER BY requested_at_unix_ms, interaction_id"
            );
            query_rows(conn, &sql, params![tenant_scope])
        })
    }

    fn load_active(&self) -> Result<Vec<InteractionRow>, HitlError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{ROW_SELECT}
                 WHERE status IN ('open', 'delivered')
                 ORDER BY requested_at_unix_ms, interaction_id"
            );
            query_rows(conn, &sql, &[])
        })
    }

    fn set_status(
        &self,
        tenant_scope: &str,
        interaction_id: &str,
        status: InteractionStatus,
        resolved_by: Option<&str>,
        updated_at: Timestamp,
    ) -> Result<(), HitlError> {
        self.with_conn(|conn| {
            let changed = conn
                .execute(
                    "UPDATE finstack_workflow_hitl_inbox
                     SET status = ?1, resolved_by = ?2, updated_at_unix_ms = ?3
                     WHERE tenant_scope = ?4 AND interaction_id = ?5",
                    params![
                        status.as_str(),
                        resolved_by,
                        updated_at.as_unix_ms(),
                        tenant_scope,
                        interaction_id,
                    ],
                )
                .map_err(|_| HitlError::StoreUnavailable {
                    code: "hitl_status_update",
                })?;
            if changed == 0 {
                return Err(HitlError::UnknownInteraction);
            }
            Ok(())
        })
    }
}

fn is_memory_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text == ":memory:" || text.contains("mode=memory")
}
