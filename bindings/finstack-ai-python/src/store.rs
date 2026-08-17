//! Sqlite durability labels and journal-store construction for the binding.

use std::path::PathBuf;
use std::sync::Arc;

use finstack_ai::AgentRunError;
use finstack_ai::runtime::JournalStore;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_store_sqlite::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteJournalStore, SqliteStoreConfig,
};
use pyo3::prelude::*;

use crate::errors::configuration_error;

/// Durability policy applied when the Python binding opens a sqlite journal.
///
/// `Durable` is WAL plus `synchronous=FULL` and is the only mode that may
/// advertise durable health. `Relaxed` is a named non-durable mode and must
/// never advertise NFR-REL-001. `:memory:` is allowed only with `Relaxed`.
#[pyclass(
    eq,
    eq_int,
    frozen,
    from_py_object,
    name = "SqliteDurability",
    module = "finstack_ai._finstack_ai"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PySqliteDurability {
    /// WAL plus `synchronous=FULL` (and Darwin full-fsync controls).
    Durable = 0,
    /// Named non-durable mode. Must never advertise NFR-REL-001.
    Relaxed = 1,
}

#[pymethods]
impl PySqliteDurability {
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "PyO3 __repr__ must borrow the shared Python object"
    )]
    fn __repr__(&self) -> &'static str {
        match self {
            Self::Durable => "Durable",
            Self::Relaxed => "Relaxed",
        }
    }
}

impl PySqliteDurability {
    pub(crate) fn to_rust(self) -> SqliteDurability {
        match self {
            Self::Durable => SqliteDurability::Durable,
            Self::Relaxed => SqliteDurability::Relaxed {
                synchronous: finstack_ai_store_sqlite::SqliteSynchronous::Normal,
            },
        }
    }
}

pub(crate) fn default_store_limits() -> MemoryStoreLimits {
    MemoryStoreLimits {
        sessions: 64,
        batches_per_session: 256,
        records_per_session: 4_096,
        snapshot_bytes: 64 * 1_024,
    }
}

pub(crate) fn open_journal_store(
    sqlite_path: Option<String>,
    sqlite_durability: Option<PySqliteDurability>,
) -> Result<Arc<dyn JournalStore>, AgentRunError> {
    match sqlite_path {
        None => {
            if sqlite_durability.is_some() {
                return Err(configuration_error(
                    "sqlite_durability requires sqlite_path",
                ));
            }
            MemoryJournalStore::try_new(default_store_limits())
                .map(|store| Arc::new(store) as Arc<dyn JournalStore>)
                .map_err(|error| configuration_error(error.to_string()))
        }
        Some(path) => {
            let durability = sqlite_durability.unwrap_or(PySqliteDurability::Durable);
            SqliteJournalStore::try_open(SqliteStoreConfig {
                path: PathBuf::from(path),
                durability: durability.to_rust(),
                limits: default_store_limits(),
                busy_timeout: DEFAULT_BUSY_TIMEOUT,
            })
            .map(|store| Arc::new(store) as Arc<dyn JournalStore>)
            .map_err(|error| configuration_error(format!("{}: {error}", error.code())))
        }
    }
}
