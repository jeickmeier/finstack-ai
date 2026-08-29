//! Sqlite durability labels and journal-store construction for the binding.

use std::path::PathBuf;
use std::sync::Arc;

use finstack_ai::AgentRunError;
use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};
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

/// Open the configured journal: postgres when `postgres_dsn` is set,
/// sqlite when `sqlite_path` is set, in-memory otherwise. The DSN arrives
/// as an explicit Python value; the binding never reads environment
/// variables.
pub(crate) async fn open_journal_store(
    sqlite_path: Option<String>,
    sqlite_durability: Option<PySqliteDurability>,
    postgres_dsn: Option<String>,
) -> Result<Arc<dyn JournalStore>, AgentRunError> {
    if let Some(dsn) = postgres_dsn {
        if sqlite_path.is_some() || sqlite_durability.is_some() {
            return Err(configuration_error(
                "postgres_dsn is mutually exclusive with sqlite_path/sqlite_durability",
            ));
        }
        let limits = default_store_limits();
        let config = PostgresStoreConfig::new(
            dsn,
            finstack_ai::runtime::ports::journal::StoreLimits {
                sessions: limits.sessions,
                batches_per_session: limits.batches_per_session,
                records_per_session: limits.records_per_session,
                snapshot_bytes: limits.snapshot_bytes,
            },
        );
        return PostgresJournalStore::try_open(config)
            .await
            .map(|store| Arc::new(store) as Arc<dyn JournalStore>)
            .map_err(|error| configuration_error(format!("{}: {error}", error.code())));
    }
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

/// Durable S3-compatible artifact store handle.
///
/// Wraps the artifact crate's strict S3 transport (`SigV4` when credentials
/// are given, keyless otherwise). Pass instances via any agent factory's
/// `artifact_store=` keyword; attachment staging, the document toolset,
/// and the ingest middleware then share the bucket. Credentials arrive as
/// explicit Python values — the binding never reads environment variables.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "S3ArtifactStore",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyS3ArtifactStore {
    pub(crate) inner: Arc<dyn finstack_ai::runtime::artifact::ArtifactStore>,
}

#[pymethods]
impl PyS3ArtifactStore {
    /// Open a store over one bucket.
    #[new]
    #[pyo3(signature = (endpoint, bucket, region, *, key_prefix = None, access_key_id = None, secret_access_key = None))]
    fn new(
        endpoint: &str,
        bucket: &str,
        region: &str,
        key_prefix: Option<&str>,
        access_key_id: Option<&str>,
        secret_access_key: Option<&str>,
    ) -> PyResult<Self> {
        use pyo3::exceptions::PyValueError;
        let mut config =
            finstack_ai_store_artifact::S3ObjectStoreConfig::try_new(endpoint, bucket, region)
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
        if let Some(prefix) = key_prefix {
            config = config
                .try_with_key_prefix(prefix)
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
        }
        match (access_key_id, secret_access_key) {
            (Some(id), Some(secret)) => {
                let secret = finstack_ai::runtime::ports::model::SecretString::try_new(secret)
                    .map_err(|error| PyValueError::new_err(format!("{error:?}")))?;
                config = config
                    .with_credentials(id, secret)
                    .map_err(|error| PyValueError::new_err(error.to_string()))?;
            }
            (None, None) => {}
            _ => {
                return Err(PyValueError::new_err(
                    "access_key_id and secret_access_key must be provided together",
                ));
            }
        }
        let store = finstack_ai_store_artifact::S3ArtifactStore::try_new(config)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(store),
        })
    }
}
