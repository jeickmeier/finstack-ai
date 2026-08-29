//! Python constructors for native Rust observer extensions.

use std::io::Write;
use std::sync::{Arc, Mutex};

use finstack_ai::runtime::ports::observer::Observer;
use finstack_ai_kernel::ComponentRef;
use finstack_ai_observer_log::LogObserver;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::callbacks::parse_payload_mode;

/// Structured NDJSON run-event observer backed by the Rust implementation.
///
/// Writes one JSON line per observed run event to a file (append mode) or
/// to the process's stderr. Payload visibility is bounded by
/// `payload_mode`: `"metadata_only"` (default) never includes event
/// bodies, `"redacted"` includes only public-sensitivity bodies, and
/// `"full"` includes everything except credential-sensitivity bodies.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "LogObserver",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyLogObserver {
    component: ComponentRef,
    inner: Arc<LogObserver>,
}

impl PyLogObserver {
    fn from_writer(payload_mode: &str, writer: Arc<Mutex<dyn Write + Send>>) -> PyResult<Self> {
        let mode = parse_payload_mode(payload_mode)?;
        let observer = LogObserver::try_new(mode, writer)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = observer.descriptor().component;
        Ok(Self {
            component,
            inner: Arc::new(observer),
        })
    }

    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.component.clone(), self.inner.clone())
    }
}

#[pymethods]
impl PyLogObserver {
    /// Append NDJSON event lines to the file at `path`.
    #[new]
    #[pyo3(signature = (path, payload_mode = "metadata_only"))]
    fn new(path: &str, payload_mode: &str) -> PyResult<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| PyValueError::new_err(format!("log path unwritable: {error}")))?;
        Self::from_writer(payload_mode, Arc::new(Mutex::new(file)))
    }

    /// Write NDJSON event lines to the process's stderr.
    #[staticmethod]
    #[pyo3(signature = (payload_mode = "metadata_only"))]
    fn stderr(payload_mode: &str) -> PyResult<Self> {
        Self::from_writer(payload_mode, Arc::new(Mutex::new(std::io::stderr())))
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}
