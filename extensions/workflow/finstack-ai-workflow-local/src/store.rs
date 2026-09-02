use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::Timestamp;
use rusqlite::{Connection, TransactionBehavior, params};

use crate::cron::{CronError, CronSchedule, IntervalSchedule};

/// Adapter-owned schedule table. Not part of the journal schema version.
pub trait CronScheduleStore: Send + Sync {
    /// Insert or replace one tenant-scoped row.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn upsert(&self, schedule: &CronSchedule) -> Result<(), CronError>;

    /// Load every schedule for one tenant.
    ///
    /// # Errors
    ///
    /// Returns store-unavailable or integrity failures.
    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<CronSchedule>, CronError>;

    /// Compare-and-set claim for one due tick.
    ///
    /// Wins only when the stored `next_fire_unix_ms` still equals
    /// `expected_next_unix_ms` and is at or before `now`. Third-party
    /// stores fail closed unless they override this method.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::StoreUnavailable`] by default.
    fn try_claim(
        &self,
        tenant_scope: &str,
        schedule_id: &str,
        expected_next_unix_ms: i64,
        now: Timestamp,
        claimed: &CronSchedule,
    ) -> Result<bool, CronError> {
        let _ = (
            tenant_scope,
            schedule_id,
            expected_next_unix_ms,
            now,
            claimed,
        );
        Err(CronError::StoreUnavailable {
            code: "try_claim_unsupported",
        })
    }

    /// Every schedule (any tenant) due at or before `now`.
    ///
    /// Third-party stores fail closed unless they override this method.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::StoreUnavailable`] by default.
    fn load_due(&self, now: Timestamp, limit: usize) -> Result<Vec<CronSchedule>, CronError> {
        let _ = (now, limit);
        Err(CronError::StoreUnavailable {
            code: "load_due_unsupported",
        })
    }
}

type MemoryCronRows = BTreeMap<(Arc<str>, Arc<str>), CronSchedule>;

/// In-process table used with a non-file journal store.
#[derive(Debug, Default)]
pub struct MemoryCronStore {
    inner: Mutex<MemoryCronRows>,
}

impl MemoryCronStore {
    /// Empty adapter table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl CronScheduleStore for MemoryCronStore {
    fn upsert(&self, schedule: &CronSchedule) -> Result<(), CronError> {
        let mut inner = self.inner.lock().map_err(|_| CronError::StoreUnavailable {
            code: "memory_cron_lock_poisoned",
        })?;
        inner.insert(
            (
                Arc::clone(&schedule.tenant_scope),
                Arc::clone(&schedule.schedule_id),
            ),
            schedule.clone(),
        );
        Ok(())
    }

    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<CronSchedule>, CronError> {
        let inner = self.inner.lock().map_err(|_| CronError::StoreUnavailable {
            code: "memory_cron_lock_poisoned",
        })?;
        Ok(inner
            .values()
            .filter(|schedule| schedule.tenant_scope.as_ref() == tenant_scope)
            .cloned()
            .collect())
    }

    fn try_claim(
        &self,
        tenant_scope: &str,
        schedule_id: &str,
        expected_next_unix_ms: i64,
        now: Timestamp,
        claimed: &CronSchedule,
    ) -> Result<bool, CronError> {
        let mut inner = self.inner.lock().map_err(|_| CronError::StoreUnavailable {
            code: "memory_cron_lock_poisoned",
        })?;
        let Some(row) = inner.get_mut(&(Arc::from(tenant_scope), Arc::from(schedule_id))) else {
            return Ok(false);
        };
        if row.next_fire_at.as_unix_ms() != expected_next_unix_ms || row.next_fire_at > now {
            return Ok(false);
        }
        *row = claimed.clone();
        Ok(true)
    }

    fn load_due(&self, now: Timestamp, limit: usize) -> Result<Vec<CronSchedule>, CronError> {
        let inner = self.inner.lock().map_err(|_| CronError::StoreUnavailable {
            code: "memory_cron_lock_poisoned",
        })?;
        Ok(inner
            .values()
            .filter(|schedule| schedule.next_fire_at <= now)
            .take(limit)
            .cloned()
            .collect())
    }
}

const CRON_SCHEMA_VERSION: i64 = 1;

const CRON_DDL: &str = "
CREATE TABLE IF NOT EXISTS finstack_workflow_local_schema (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  version INTEGER NOT NULL
);
INSERT OR IGNORE INTO finstack_workflow_local_schema (singleton, version) VALUES (1, 1);
CREATE TABLE IF NOT EXISTS finstack_workflow_local_cron (
  tenant_scope TEXT NOT NULL,
  schedule_id TEXT NOT NULL,
  expression TEXT NOT NULL,
  origin_unix_ms INTEGER NOT NULL,
  next_fire_unix_ms INTEGER NOT NULL,
  last_fired_unix_ms INTEGER,
  fire_count INTEGER NOT NULL,
  PRIMARY KEY (tenant_scope, schedule_id)
);
";

/// Sqlite table in the same file as the journal store.
///
/// Rows are adapter state. They do not change `PRAGMA user_version` and are
/// not kernel records.
pub struct SqliteCronStore {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl SqliteCronStore {
    /// Open or create the adapter table in `path`.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::StoreUnavailable`] when the file cannot be opened
    /// or the table cannot be created.
    pub fn try_open(path: impl AsRef<Path>) -> Result<Self, CronError> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|_| CronError::StoreUnavailable {
            code: "sqlite_cron_open",
        })?;
        conn.busy_timeout(Duration::from_secs(1))
            .map_err(|_| CronError::StoreUnavailable {
                code: "sqlite_cron_busy_timeout",
            })?;
        if !is_memory_path(&path) {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_wal",
                })?;
        }
        let has_schema = table_exists(&conn, "finstack_workflow_local_schema")?;
        if !has_schema && table_exists(&conn, "finstack_workflow_local_cron")? {
            return Err(CronError::StoreIntegrity {
                code: "cron_schema_reset_required",
            });
        }
        conn.execute_batch(CRON_DDL)
            .map_err(|_| CronError::StoreUnavailable {
                code: "sqlite_cron_schema",
            })?;
        let version: i64 = conn
            .query_row(
                "SELECT version FROM finstack_workflow_local_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| CronError::StoreIntegrity {
                code: "cron_schema_version",
            })?;
        if version != CRON_SCHEMA_VERSION {
            return Err(CronError::StoreIntegrity {
                code: "cron_schema_version",
            });
        }
        Ok(Self {
            path,
            conn: Mutex::new(conn),
        })
    }

    /// Configured journal file, including `:memory:`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with_conn<T>(
        &self,
        body: impl FnOnce(&mut Connection) -> Result<T, CronError>,
    ) -> Result<T, CronError> {
        let mut conn = self.conn.lock().map_err(|_| CronError::StoreUnavailable {
            code: "sqlite_cron_lock_poisoned",
        })?;
        body(&mut conn)
    }
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, CronError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
    .map_err(|_| CronError::StoreUnavailable {
        code: "sqlite_cron_schema",
    })
}

impl CronScheduleStore for SqliteCronStore {
    fn upsert(&self, schedule: &CronSchedule) -> Result<(), CronError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO finstack_workflow_local_cron (
                    tenant_scope, schedule_id, expression, origin_unix_ms,
                    next_fire_unix_ms, last_fired_unix_ms, fire_count
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    schedule.tenant_scope.as_ref(),
                    schedule.schedule_id.as_ref(),
                    schedule.expression.as_str(),
                    schedule.origin.as_unix_ms(),
                    schedule.next_fire_at.as_unix_ms(),
                    schedule.last_fired_at.map(Timestamp::as_unix_ms),
                    i64_from_u64(schedule.fire_count)?,
                ],
            )
            .map_err(|_| CronError::StoreUnavailable {
                code: "sqlite_cron_upsert",
            })?;
            Ok(())
        })
    }

    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<CronSchedule>, CronError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT schedule_id, expression, origin_unix_ms, next_fire_unix_ms,
                            last_fired_unix_ms, fire_count
                     FROM finstack_workflow_local_cron
                     WHERE tenant_scope = ?1
                     ORDER BY schedule_id",
                )
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_prepare",
                })?;
            let tenant_scope: Arc<str> = Arc::from(tenant_scope);
            let rows = stmt
                .query_map(params![tenant_scope.as_ref()], |row| {
                    Ok((Arc::clone(&tenant_scope), RawSchedule::read(row, 0)?))
                })
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_query",
                })?;
            rows.map(decode_schedule).collect()
        })
    }

    fn try_claim(
        &self,
        tenant_scope: &str,
        schedule_id: &str,
        expected_next_unix_ms: i64,
        now: Timestamp,
        claimed: &CronSchedule,
    ) -> Result<bool, CronError> {
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_begin_immediate",
                })?;
            tx.execute(
                "UPDATE finstack_workflow_local_cron
                 SET last_fired_unix_ms = ?1, fire_count = ?2, next_fire_unix_ms = ?3
                 WHERE tenant_scope = ?4 AND schedule_id = ?5
                   AND next_fire_unix_ms = ?6 AND next_fire_unix_ms <= ?7",
                params![
                    claimed.last_fired_at.map(Timestamp::as_unix_ms),
                    i64_from_u64(claimed.fire_count)?,
                    claimed.next_fire_at.as_unix_ms(),
                    tenant_scope,
                    schedule_id,
                    expected_next_unix_ms,
                    now.as_unix_ms(),
                ],
            )
            .map_err(|_| CronError::StoreUnavailable {
                code: "sqlite_cron_claim",
            })?;
            let won = tx.changes() == 1;
            tx.commit().map_err(|_| CronError::StoreUnavailable {
                code: "sqlite_cron_commit",
            })?;
            Ok(won)
        })
    }

    fn load_due(&self, now: Timestamp, limit: usize) -> Result<Vec<CronSchedule>, CronError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT tenant_scope, schedule_id, expression, origin_unix_ms,
                            next_fire_unix_ms, last_fired_unix_ms, fire_count
                     FROM finstack_workflow_local_cron
                     WHERE next_fire_unix_ms <= ?1
                     ORDER BY tenant_scope, schedule_id
                     LIMIT ?2",
                )
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_prepare",
                })?;
            let rows = stmt
                .query_map(
                    params![now.as_unix_ms(), i64::try_from(limit).unwrap_or(i64::MAX)],
                    |row| {
                        let tenant_scope: String = row.get(0)?;
                        Ok((Arc::from(tenant_scope), RawSchedule::read(row, 1)?))
                    },
                )
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_query",
                })?;
            rows.map(decode_schedule).collect()
        })
    }
}

/// The schedule columns shared by [`CronScheduleStore::load_tenant`] and
/// [`CronScheduleStore::load_due`], as read from sqlite before decoding.
struct RawSchedule {
    schedule_id: String,
    expression: String,
    origin: i64,
    next_fire: i64,
    last_fired: Option<i64>,
    fire_count: i64,
}

impl RawSchedule {
    /// Read the six schedule columns starting at column `first`.
    fn read(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            schedule_id: row.get(first)?,
            expression: row.get(first + 1)?,
            origin: row.get(first + 2)?,
            next_fire: row.get(first + 3)?,
            last_fired: row.get(first + 4)?,
            fire_count: row.get(first + 5)?,
        })
    }
}

fn decode_schedule(
    row: rusqlite::Result<(Arc<str>, RawSchedule)>,
) -> Result<CronSchedule, CronError> {
    let (tenant_scope, raw) = row.map_err(|_| CronError::StoreIntegrity {
        code: "sqlite_cron_row",
    })?;
    Ok(CronSchedule {
        tenant_scope,
        schedule_id: Arc::from(raw.schedule_id),
        expression: IntervalSchedule::parse(&raw.expression)?,
        origin: Timestamp::from_unix_ms(raw.origin).map_err(|_| CronError::StoreIntegrity {
            code: "sqlite_cron_origin",
        })?,
        next_fire_at: Timestamp::from_unix_ms(raw.next_fire).map_err(|_| {
            CronError::StoreIntegrity {
                code: "sqlite_cron_next_fire",
            }
        })?,
        last_fired_at: raw
            .last_fired
            .map(Timestamp::from_unix_ms)
            .transpose()
            .map_err(|_| CronError::StoreIntegrity {
                code: "sqlite_cron_last_fired",
            })?,
        fire_count: u64_from_i64(raw.fire_count)?,
    })
}

fn is_memory_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text == ":memory:" || text.contains("mode=memory")
}

fn i64_from_u64(value: u64) -> Result<i64, CronError> {
    i64::try_from(value).map_err(|_| CronError::StoreIntegrity {
        code: "sqlite_cron_fire_count",
    })
}

fn u64_from_i64(value: i64) -> Result<u64, CronError> {
    u64::try_from(value).map_err(|_| CronError::StoreIntegrity {
        code: "sqlite_cron_fire_count",
    })
}

/// Probe used by tests to confirm a foreign-tenant row is stored but hidden.
#[cfg(test)]
pub(crate) fn load_one(
    store: &dyn CronScheduleStore,
    tenant_scope: &str,
    schedule_id: &str,
) -> Result<Option<CronSchedule>, CronError> {
    Ok(store
        .load_tenant(tenant_scope)?
        .into_iter()
        .find(|schedule| schedule.schedule_id.as_ref() == schedule_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_isolates_tenants() {
        let store = MemoryCronStore::new();
        let origin = Timestamp::from_unix_ms(2_000).expect("origin");
        store
            .upsert(&CronSchedule {
                tenant_scope: Arc::from("tenant-a"),
                schedule_id: Arc::from("tick"),
                expression: IntervalSchedule::parse("every 10ms").expect("expr"),
                origin,
                next_fire_at: Timestamp::from_unix_ms(2_010).expect("next"),
                last_fired_at: None,
                fire_count: 0,
            })
            .expect("upsert a");
        store
            .upsert(&CronSchedule {
                tenant_scope: Arc::from("tenant-b"),
                schedule_id: Arc::from("tick"),
                expression: IntervalSchedule::parse("every 10ms").expect("expr"),
                origin,
                next_fire_at: Timestamp::from_unix_ms(2_010).expect("next"),
                last_fired_at: None,
                fire_count: 0,
            })
            .expect("upsert b");
        assert_eq!(store.load_tenant("tenant-a").expect("a").len(), 1);
        assert_eq!(store.load_tenant("tenant-b").expect("b").len(), 1);
        assert!(
            load_one(&store, "tenant-a", "missing")
                .expect("miss")
                .is_none()
        );
    }

    #[test]
    fn load_due_crosses_tenants_and_respects_now() {
        let store = MemoryCronStore::new();
        let origin = Timestamp::from_unix_ms(2_000).expect("origin");
        for (tenant, next) in [("tenant-a", 2_010), ("tenant-b", 2_020)] {
            store
                .upsert(&CronSchedule {
                    tenant_scope: Arc::from(tenant),
                    schedule_id: Arc::from("tick"),
                    expression: IntervalSchedule::parse("every 10ms").expect("expr"),
                    origin,
                    next_fire_at: Timestamp::from_unix_ms(next).expect("next"),
                    last_fired_at: None,
                    fire_count: 0,
                })
                .expect("upsert");
        }
        let due = store
            .load_due(Timestamp::from_unix_ms(2_015).expect("now"), 10)
            .expect("due");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].tenant_scope.as_ref(), "tenant-a");
    }

    #[test]
    fn sqlite_load_due_crosses_tenants_and_respects_now() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("cron.sqlite");
        let store = SqliteCronStore::try_open(&path).expect("store");
        let origin = Timestamp::from_unix_ms(2_000).expect("origin");
        for (tenant, next) in [("tenant-a", 2_010), ("tenant-b", 2_020)] {
            store
                .upsert(&CronSchedule {
                    tenant_scope: Arc::from(tenant),
                    schedule_id: Arc::from("tick"),
                    expression: IntervalSchedule::parse("every 10ms").expect("expr"),
                    origin,
                    next_fire_at: Timestamp::from_unix_ms(next).expect("next"),
                    last_fired_at: None,
                    fire_count: 0,
                })
                .expect("upsert");
        }
        let due = store
            .load_due(Timestamp::from_unix_ms(2_015).expect("now"), 10)
            .expect("due");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].tenant_scope.as_ref(), "tenant-a");
    }

    #[test]
    fn sqlite_rejects_an_unversioned_cron_table() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("legacy.sqlite");
        let legacy = Connection::open(&path).expect("legacy");
        legacy
            .execute_batch(
                "CREATE TABLE finstack_workflow_local_cron (
                    tenant_scope TEXT NOT NULL,
                    schedule_id TEXT NOT NULL,
                    expression TEXT NOT NULL,
                    origin_unix_ms INTEGER NOT NULL,
                    next_fire_unix_ms INTEGER NOT NULL,
                    last_fired_unix_ms INTEGER,
                    fire_count INTEGER NOT NULL,
                    PRIMARY KEY (tenant_scope, schedule_id)
                );",
            )
            .expect("schema");
        drop(legacy);

        let Err(error) = SqliteCronStore::try_open(&path) else {
            panic!("unversioned schema must fail closed");
        };
        assert_eq!(error.code(), "cron_schema_reset_required");
    }

    #[test]
    fn two_sqlite_stores_claim_exactly_one_fire() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("cron.sqlite");
        let first = SqliteCronStore::try_open(&path).expect("first");
        let second = SqliteCronStore::try_open(&path).expect("second");
        let origin = Timestamp::from_unix_ms(2_000).expect("origin");
        let due = Timestamp::from_unix_ms(2_010).expect("due");
        let schedule = CronSchedule {
            tenant_scope: Arc::from("tenant-a"),
            schedule_id: Arc::from("tick"),
            expression: IntervalSchedule::parse("every 10ms").expect("expr"),
            origin,
            next_fire_at: due,
            last_fired_at: None,
            fire_count: 0,
        };
        first.upsert(&schedule).expect("upsert");
        let now = Timestamp::from_unix_ms(2_025).expect("now");
        let mut claimed = schedule.clone();
        claimed.last_fired_at = Some(now);
        claimed.fire_count = 1;
        claimed.next_fire_at = claimed.expression.next_after(origin, now).expect("next");
        let won_first = first
            .try_claim("tenant-a", "tick", due.as_unix_ms(), now, &claimed)
            .expect("first claim");
        let won_second = second
            .try_claim("tenant-a", "tick", due.as_unix_ms(), now, &claimed)
            .expect("second claim");
        assert_eq!(usize::from(won_first) + usize::from(won_second), 1);
        assert_eq!(
            first.load_tenant("tenant-a").expect("load")[0].fire_count,
            1
        );
    }
}
