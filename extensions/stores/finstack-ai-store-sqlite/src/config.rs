use std::path::{Path, PathBuf};
use std::time::Duration;

use finstack_ai_runtime::{StoreError, StoreLimits};

/// Default lock-wait used when a second connection contends for the writer.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// Required resource ceilings for [`crate::SqliteJournalStore`].
pub type SqliteStoreLimits = StoreLimits;

/// Sqlite `synchronous` setting for explicitly labeled relaxed mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteSynchronous {
    /// `PRAGMA synchronous=NORMAL`.
    Normal,
    /// `PRAGMA synchronous=OFF`.
    Off,
}

/// Durability policy applied at open and advertised by [`finstack_ai_runtime::JournalStore::health`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteDurability {
    /// WAL plus `synchronous=FULL` (and Darwin full-fsync controls).
    ///
    /// This is the only mode that may set `health().durable = true`. It still
    /// depends on the OS and filesystem honoring those flush settings.
    Durable,
    /// Named non-durable mode. Must never advertise NFR-REL-001.
    Relaxed {
        /// Relaxed `synchronous` pragma.
        synchronous: SqliteSynchronous,
    },
}

/// Open configuration for [`crate::SqliteJournalStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteStoreConfig {
    /// Database path. `:memory:` is allowed only with [`SqliteDurability::Relaxed`].
    pub path: PathBuf,
    /// Durability / health labeling.
    pub durability: SqliteDurability,
    /// Resource ceilings.
    pub limits: SqliteStoreLimits,
    /// `SQLITE_BUSY` wait before `sqlite_busy`.
    pub busy_timeout: Duration,
}

impl SqliteStoreConfig {
    /// Construct a config using [`DEFAULT_BUSY_TIMEOUT`].
    #[must_use]
    pub fn new(
        path: impl Into<PathBuf>,
        durability: SqliteDurability,
        limits: SqliteStoreLimits,
    ) -> Self {
        Self {
            path: path.into(),
            durability,
            limits,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }
}

pub(crate) fn is_memory_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text == ":memory:" || text.contains("mode=memory")
}

pub(crate) fn health_label(
    durability: SqliteDurability,
    memory: bool,
) -> Result<(bool, &'static str), StoreError> {
    match durability {
        SqliteDurability::Durable => {
            if memory {
                return Err(StoreError::InvalidRequest {
                    reason_code: "sqlite_durable_requires_file",
                });
            }
            Ok((true, "sqlite_durable_wal_full"))
        }
        SqliteDurability::Relaxed { synchronous } => {
            let detail = if memory {
                "sqlite_relaxed_in_memory"
            } else {
                match synchronous {
                    SqliteSynchronous::Normal => "sqlite_relaxed_synchronous_normal",
                    SqliteSynchronous::Off => "sqlite_relaxed_synchronous_off",
                }
            };
            Ok((false, detail))
        }
    }
}
