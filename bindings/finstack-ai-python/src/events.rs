//! Immutable runtime event snapshots and the batch-first iterator.

use std::sync::{Arc, OnceLock};

use finstack_ai_kernel::RunEvent;
use pyo3::exceptions::{PyException, PyStopAsyncIteration};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::errors::agent_error;

/// Batch-first asynchronous event iterator.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "EventBatchIterator",
    frozen
)]
pub(crate) struct PyEventIterator {
    pub(crate) run: finstack_ai::AgentRun,
}

#[pymethods]
impl PyEventIterator {
    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.run.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.next_event_batch().await {
                Ok(Some(batch)) => Python::attach(|py| {
                    Py::new(
                        py,
                        PyEventBatch {
                            inner: batch,
                            serialized: OnceLock::new(),
                        },
                    )
                }),
                Ok(None) => Err(PyStopAsyncIteration::new_err(())),
                Err(error) => Err(Python::attach(|py| agent_error(py, &error, Some(&locator)))),
            }
        })
    }
}

/// Immutable runtime event snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Event", frozen)]
pub(crate) struct PyEvent {
    inner: RunEvent,
}

#[pymethods]
impl PyEvent {
    #[getter]
    fn kind(&self) -> &'static str {
        self.inner.kind().kind_name()
    }

    #[getter]
    fn event_class(&self) -> &'static str {
        self.inner.class().class_name()
    }

    #[getter]
    fn transient_sequence(&self) -> u64 {
        self.inner.transient_sequence()
    }

    #[getter]
    fn durable_sequence(&self) -> Option<u64> {
        self.inner.durable_sequence()
    }

    /// Serialize the complete immutable event on explicit request.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner)
            .map_err(|_| PyException::new_err("event serialization failed"))
    }
}

/// Immutable bounded transport batch.
#[pyclass(module = "finstack_ai._finstack_ai", name = "EventBatch", frozen)]
pub(crate) struct PyEventBatch {
    inner: finstack_ai::runtime::EventBatch,
    serialized: OnceLock<Result<Arc<[u8]>, Arc<str>>>,
}

#[pymethods]
impl PyEventBatch {
    #[getter]
    fn first_sequence(&self) -> u64 {
        self.inner.first_sequence()
    }

    #[getter]
    fn last_sequence(&self) -> u64 {
        self.inner.last_sequence()
    }

    #[getter]
    fn dropped_progress(&self) -> u64 {
        self.inner.dropped_progress()
    }

    fn __len__(&self) -> usize {
        self.inner.events().len()
    }

    /// Expand this batch into immutable event snapshots on explicit request.
    fn events(&self) -> Vec<PyEvent> {
        self.inner
            .events()
            .iter()
            .cloned()
            .map(|inner| PyEvent { inner })
            .collect()
    }

    /// Serialize the complete batch on explicit request.
    fn to_json(&self) -> PyResult<String> {
        let bytes = self.serialized_bytes()?;
        std::str::from_utf8(bytes)
            .map(ToOwned::to_owned)
            .map_err(|_| PyException::new_err("event batch serialization produced invalid UTF-8"))
    }

    /// Serialize the complete batch once and copy it directly into Python bytes.
    fn to_json_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        self.serialized_bytes()
            .map(|bytes| PyBytes::new(py, bytes.as_ref()))
    }
}

impl PyEventBatch {
    fn serialized_bytes(&self) -> PyResult<&Arc<[u8]>> {
        self.serialized
            .get_or_init(|| {
                serde_json::to_vec(self.inner.events())
                    .map(Arc::from)
                    .map_err(|_| Arc::from("event batch serialization failed"))
            })
            .as_ref()
            .map_err(|message| PyException::new_err(message.to_string()))
    }
}
