//! Live session, lane, and in-process external-identity handles.

use finstack_ai::{
    ExternalIdentityKey, ExternalIdentityMap, Lane, MemoryExternalIdentityMap, Session,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::ConfigurationError;
use crate::agent::{DEFAULT_MAX_CYCLES, PyAgent};
use crate::errors::{agent_error, session_py_error};
use crate::run::PyRun;

/// Live session handle over one journaled session.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Session", frozen)]
pub(crate) struct PySession {
    pub(crate) inner: Session,
}

#[pymethods]
impl PySession {
    #[getter]
    fn tenant_scope(&self) -> &str {
        self.inner.tenant_scope()
    }

    #[getter]
    fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Create a named lane, optionally forking from an existing entry.
    #[pyo3(signature = (name, fork = None))]
    fn create_lane<'py>(
        &self,
        py: Python<'py>,
        name: String,
        fork: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let fork = fork
                .map(|value| finstack_ai_kernel::EntryId::parse(&value))
                .transpose()
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match session.create_lane(name, fork).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PyLane { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// List restored lanes.
    fn list_lanes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match session.list_lanes().await {
                Ok(lanes) => Python::attach(|py| {
                    lanes
                        .into_iter()
                        .map(|inner| Py::new(py, PyLane { inner }))
                        .collect::<PyResult<Vec<_>>>()
                }),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Look up one lane by application name.
    fn lane<'py>(&self, py: Python<'py>, name: String) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match session.lane(&name).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PyLane { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Look up one lane by durable identity.
    fn lane_by_id<'py>(&self, py: Python<'py>, lane_id: String) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let lane_id = finstack_ai_kernel::LaneId::parse(&lane_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match session.lane_by_id(lane_id).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PyLane { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Bind a host-owned external identity to one lane.
    fn bind_external_identity(
        &self,
        map: &Bound<'_, PyMemoryExternalIdentityMap>,
        channel: String,
        account: String,
        thread: String,
        lane_id: &str,
    ) -> PyResult<()> {
        let key = ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        let lane_id = finstack_ai_kernel::LaneId::parse(lane_id)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        self.inner
            .bind_external_identity(&map.borrow().inner, key, lane_id)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))
    }

    /// Resolve a host-owned external identity key.
    #[staticmethod]
    fn resolve_external_identity(
        map: &Bound<'_, PyMemoryExternalIdentityMap>,
        channel: String,
        account: String,
        thread: String,
    ) -> PyResult<Option<(String, String)>> {
        let key = ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        Ok(
            Session::resolve_external_identity(&map.borrow().inner, &key)
                .map(|(session_id, lane_id)| (session_id.to_string(), lane_id.to_string())),
        )
    }
}

/// Live lane handle.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Lane", frozen)]
pub(crate) struct PyLane {
    inner: Lane,
}

#[pymethods]
impl PyLane {
    #[getter]
    fn lane_id(&self) -> String {
        self.inner.lane_id().to_string()
    }

    #[getter]
    fn session(&self) -> PySession {
        PySession {
            inner: self.inner.session().clone(),
        }
    }

    /// Point this idle lane at an existing entry without copying.
    fn navigate<'py>(&self, py: Python<'py>, entry_id: String) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let entry_id = finstack_ai_kernel::EntryId::parse(&entry_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match lane.navigate(entry_id).await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Inspect name, leaf, active run, and history length.
    fn inspect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match lane.inspect().await {
                Ok(inspect) => Python::attach(|py| {
                    let value = PyDict::new(py);
                    value.set_item("lane_id", inspect.lane_id.to_string())?;
                    value.set_item("name", inspect.name.as_ref())?;
                    value.set_item("leaf_id", inspect.leaf_id.map(|id| id.to_string()))?;
                    value.set_item(
                        "active_run_id",
                        inspect.active_run_id.map(|id| id.to_string()),
                    )?;
                    value.set_item("history_len", inspect.history.len())?;
                    Ok(value.unbind())
                }),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Cancel the active run on this lane and fan out through child mappings.
    fn cancel<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match lane.cancel().await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Append one user text message on this idle lane. Does not start a run.
    fn append_text<'py>(&self, py: Python<'py>, text: String) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match lane.append_text(&text).await {
                Ok(entry_id) => Ok(entry_id.to_string()),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Start a new root run on this idle lane.
    #[pyo3(signature = (agent, input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "lane run forwards the same bounded run inputs as Agent.start"
    )]
    fn run(
        &self,
        py: Python<'_>,
        agent: &Bound<'_, PyAgent>,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
    ) -> PyResult<PyRun> {
        agent.borrow().start_on_lane(
            py,
            &self.inner,
            input,
            timeout_seconds,
            max_cycles,
            max_output_retries,
            capability,
        )
    }

    /// Park the in-process driver without dropping the journal.
    fn suspend<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match lane.suspend().await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Recover the parked run and respawn the in-process owner.
    fn resume<'py>(
        &self,
        py: Python<'py>,
        agent: &Bound<'_, PyAgent>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        let agent = agent.borrow().clone_inner();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match lane.resume(&agent).await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, None))),
            }
        })
    }
}

/// In-process external identity map.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "MemoryExternalIdentityMap",
    frozen
)]
pub(crate) struct PyMemoryExternalIdentityMap {
    inner: MemoryExternalIdentityMap,
}

#[pymethods]
impl PyMemoryExternalIdentityMap {
    #[new]
    fn new() -> Self {
        Self {
            inner: MemoryExternalIdentityMap::new(),
        }
    }

    /// Resolve one previously bound key.
    fn resolve(
        &self,
        channel: String,
        account: String,
        thread: String,
    ) -> PyResult<Option<(String, String)>> {
        let key = ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        Ok(self
            .inner
            .resolve(&key)
            .map(|(session_id, lane_id)| (session_id.to_string(), lane_id.to_string())))
    }
}
