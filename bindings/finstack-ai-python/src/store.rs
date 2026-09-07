//! Sqlite durability labels and journal-store construction for the binding.

use std::path::PathBuf;
use std::sync::Arc;

use finstack_ai::AgentRunError;
use finstack_ai::runtime::ports::journal::{JournalStore, StoreLimits};
use finstack_ai_store_memory::MemoryJournalStore;
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

const STORE_LIMITS: StoreLimits = StoreLimits {
    sessions: 64,
    batches_per_session: 256,
    records_per_session: 4_096,
    snapshot_bytes: 64 * 1_024,
};

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
        let config = PostgresStoreConfig::new(dsn, STORE_LIMITS);
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
            MemoryJournalStore::try_new(STORE_LIMITS)
                .map(|store| Arc::new(store) as Arc<dyn JournalStore>)
                .map_err(|error| configuration_error(error.to_string()))
        }
        Some(path) => {
            let durability = sqlite_durability.unwrap_or(PySqliteDurability::Durable);
            SqliteJournalStore::try_open(SqliteStoreConfig {
                path: PathBuf::from(path),
                durability: durability.to_rust(),
                limits: STORE_LIMITS,
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

/// Open a journal together with the exact registration identity used by all factories.
pub(crate) async fn open_journal_registration(
    (sqlite_path, sqlite_durability, postgres_dsn): (
        Option<String>,
        Option<PySqliteDurability>,
        Option<String>,
    ),
) -> Result<(finstack_ai_kernel::ComponentRef, Arc<dyn JournalStore>), AgentRunError> {
    let id = if postgres_dsn.is_some() {
        "python.store.postgres"
    } else if sqlite_path.is_some() {
        "python.store.sqlite"
    } else {
        "python.store.memory"
    };
    let store = open_journal_store(sqlite_path, sqlite_durability, postgres_dsn).await?;
    Ok((
        crate::agent::component(id, crate::agent::PREVIEW_VERSION)?,
        store,
    ))
}

/// Shared artifact-store ownership for composing agents and native indexing tools.
#[pyclass(module = "finstack_ai._finstack_ai", name = "ArtifactStore", frozen)]
pub(crate) struct PyArtifactStore {
    pub(crate) inner: Arc<dyn finstack_ai::runtime::artifact::ArtifactStore>,
}
/// Explicit configured S3 backend or an existing agent's exact shared store.
#[derive(FromPyObject)]
pub(crate) enum PyArtifactStoreArg {
    S3(Py<PyS3ArtifactStore>),
    Shared(Py<PyArtifactStore>),
}
impl PyArtifactStoreArg {
    pub(crate) fn inner(
        &self,
        py: Python<'_>,
    ) -> Arc<dyn finstack_ai::runtime::artifact::ArtifactStore> {
        match self {
            Self::S3(store) => store.bind(py).borrow().inner.clone(),
            Self::Shared(store) => store.bind(py).borrow().inner.clone(),
        }
    }
}

/// Enumerate IDs from an application-owned `SQLite` journal, outside the journal
/// port. Uses the same native `SQLite` library as the writer. The caller owns the
/// file selection; search still independently validates committed authority.
#[pyfunction]
#[pyo3(signature=(path, *, limit=256))]
pub(crate) fn sqlite_session_ids(
    py: Python<'_>,
    path: PathBuf,
    limit: u32,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let failure = |reason| crate::errors::agent_error(&configuration_error(reason), None);
        if limit == 0 || limit > 256 || !path.is_file() {
            return Err(failure("sqlite_session_catalog_request"));
        }
        let store = SqliteJournalStore::try_open(SqliteStoreConfig::new(
            path,
            SqliteDurability::Durable,
            STORE_LIMITS,
        ))
        .map_err(|_| failure("sqlite_session_catalog_unavailable"))?;
        let rows = store
            .list_sessions(limit + 1)
            .await
            .map_err(|_| failure("sqlite_session_catalog_unavailable"))?;
        if rows.len() > limit as usize {
            return Err(failure("sqlite_session_selection_required"));
        }
        let mut ids: Vec<_> = rows
            .into_iter()
            .map(|row| row.session_id.to_string())
            .collect();
        ids.sort();
        Ok(ids)
    })
}
