use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::observer::{
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode,
};
use finstack_ai_kernel::{ComponentRef, Metadata, RunEvent, Sensitivity};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;

use super::engine::PythonCallback;
use super::shared::exact_component;

struct PythonObserverAdapter {
    callback: Arc<PythonCallback>,
    descriptor: ObserverDescriptor,
}

impl Observer for PythonObserverAdapter {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let callback = Arc::clone(&self.callback);
        let mode = self.descriptor.payload_mode;
        Box::pin(async move {
            let projected = batch
                .iter()
                .map(|event| observer_event(event, mode))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|()| ObserverError::Unavailable)?;
            callback
                .invoke_batch(&projected)
                .await
                .map_err(|_| ObserverError::Unavailable)
        })
    }
}

/// Trusted batched Python implementation of the Observer port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonObserver",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonObserver {
    inner: Arc<PythonObserverAdapter>,
}

#[pymethods]
impl PyPythonObserver {
    /// Register one trusted batched Python observer callback.
    #[new]
    #[pyo3(signature = (callback, *, component, payload_mode = "metadata_only", callback_timeout_seconds = 30.0))]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        payload_mode: &str,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        Ok(Self {
            inner: Arc::new(PythonObserverAdapter {
                callback: Arc::new(PythonCallback::try_new(
                    py,
                    callback,
                    callback_timeout_seconds,
                )?),
                descriptor: ObserverDescriptor {
                    component,
                    payload_mode: parse_payload_mode(payload_mode)?,
                    metadata: Metadata::empty(),
                },
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor.component.id().to_string()
    }
}

impl PyPythonObserver {
    /// Ready handle pair for [`finstack_ai::NativeAgentBuilder`].
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (
            self.inner.descriptor.component.clone(),
            Arc::clone(&self.inner) as Arc<dyn Observer>,
        )
    }
}

pub(crate) fn parse_payload_mode(value: &str) -> PyResult<ObserverPayloadMode> {
    match value {
        "metadata_only" => Ok(ObserverPayloadMode::MetadataOnly),
        "redacted" => Ok(ObserverPayloadMode::Redacted),
        "full" => Ok(ObserverPayloadMode::Full),
        _ => Err(PyTypeError::new_err(format!(
            "unsupported observer payload_mode: {value}"
        ))),
    }
}

fn observer_event(event: &RunEvent, mode: ObserverPayloadMode) -> Result<serde_json::Value, ()> {
    let mut value = serde_json::to_value(event).map_err(|_| ())?;
    let include_body = match mode {
        ObserverPayloadMode::MetadataOnly => false,
        ObserverPayloadMode::Redacted => event.sensitivity() == Sensitivity::Public,
        ObserverPayloadMode::Full => event.sensitivity() != Sensitivity::Credential,
    };
    if !include_body {
        value.as_object_mut().ok_or(())?.remove("body");
    }
    Ok(value)
}
