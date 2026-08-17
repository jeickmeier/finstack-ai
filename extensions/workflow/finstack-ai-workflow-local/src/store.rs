use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_runtime::Timestamp;
use rusqlite::{Connection, params};

use crate::cron::{CronError, CronExpression, CronSchedule};

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
}

const CRON_DDL: &str = "
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
}

impl SqliteCronStore {
    /// Open or create the adapter table in `path`.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::StoreUnavailable`] when the file cannot be opened
    /// or the table cannot be created.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CronError> {
        let path = path.as_ref().to_path_buf();
        let store = Self { path };
        store.with_conn(|_| Ok(()))?;
        Ok(store)
    }

    /// Configured journal file, including `:memory:`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with_conn<T>(
        &self,
        body: impl FnOnce(&Connection) -> Result<T, CronError>,
    ) -> Result<T, CronError> {
        let conn = Connection::open(&self.path).map_err(|_| CronError::StoreUnavailable {
            code: "sqlite_cron_open",
        })?;
        conn.busy_timeout(Duration::from_secs(1))
            .map_err(|_| CronError::StoreUnavailable {
                code: "sqlite_cron_busy_timeout",
            })?;
        if !is_memory_path(&self.path) {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_wal",
                })?;
        }
        conn.execute_batch(CRON_DDL)
            .map_err(|_| CronError::StoreUnavailable {
                code: "sqlite_cron_schema",
            })?;
        body(&conn)
    }
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
            let rows = stmt
                .query_map(params![tenant_scope], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .map_err(|_| CronError::StoreUnavailable {
                    code: "sqlite_cron_query",
                })?;
            let mut schedules = Vec::new();
            for row in rows {
                let (schedule_id, expression, origin, next_fire, last_fired, fire_count) = row
                    .map_err(|_| CronError::StoreIntegrity {
                        code: "sqlite_cron_row",
                    })?;
                schedules.push(CronSchedule {
                    tenant_scope: Arc::from(tenant_scope),
                    schedule_id: Arc::from(schedule_id),
                    expression: CronExpression::parse(&expression)?,
                    origin: Timestamp::from_unix_ms(origin).map_err(|_| {
                        CronError::StoreIntegrity {
                            code: "sqlite_cron_origin",
                        }
                    })?,
                    next_fire_at: Timestamp::from_unix_ms(next_fire).map_err(|_| {
                        CronError::StoreIntegrity {
                            code: "sqlite_cron_next_fire",
                        }
                    })?,
                    last_fired_at: last_fired
                        .map(Timestamp::from_unix_ms)
                        .transpose()
                        .map_err(|_| CronError::StoreIntegrity {
                            code: "sqlite_cron_last_fired",
                        })?,
                    fire_count: u64_from_i64(fire_count)?,
                });
            }
            Ok(schedules)
        })
    }
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
                expression: CronExpression::parse("every 10ms").expect("expr"),
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
                expression: CronExpression::parse("every 10ms").expect("expr"),
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
}
