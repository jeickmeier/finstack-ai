use std::sync::Arc;
use std::sync::atomic::Ordering;

use finstack_ai::runtime::CancellationSignal;
use futures_util::future::{Either, select};
use futures_util::{FutureExt, pin_mut};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use super::engine::CallbackState;

const PYTHON_CALLBACK_CONTEXT_SETTLED: &str = "python_callback_context_settled";

/// One invocation-scoped immutable context passed to trusted Python callbacks.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "CallbackContext",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyCallbackContext {
    pub(super) state: CallbackState,
    pub(super) cancellation: CancellationSignal,
    kind: Arc<str>,
    tenant_scope: Arc<str>,
    session_id: Arc<str>,
    lane_id: Arc<str>,
    run_id: Arc<str>,
    effect_id: Arc<str>,
}

#[pymethods]
impl PyCallbackContext {
    /// Stable coarse callback kind.
    #[getter]
    fn kind(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.kind)
    }

    /// Authenticated tenant scope for this committed effect.
    #[getter]
    fn tenant_scope(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.tenant_scope)
    }

    /// Session identity for this committed effect.
    #[getter]
    fn session_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.session_id)
    }

    /// Lane identity for this committed effect.
    #[getter]
    fn lane_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.lane_id)
    }

    /// Run identity for this committed effect.
    #[getter]
    fn run_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.run_id)
    }

    /// Committed effect identity for this callback.
    #[getter]
    fn effect_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.effect_id)
    }

    /// Whether cooperative cancellation has reached this callback.
    #[getter]
    fn cancelled(&self, py: Python<'_>) -> PyResult<bool> {
        self.ensure_active(py)?;
        Ok(self.cancellation.is_cancelled())
    }

    /// Wait until cancellation reaches this still-active callback.
    fn wait_cancelled<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.ensure_active(py)?;
        let cancellation = self.cancellation.clone();
        let state = self.state.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let cancelled = cancellation.cancelled().fuse();
            let settled = state.settled.notified().fuse();
            pin_mut!(cancelled, settled);
            match select(cancelled, settled).await {
                Either::Left(((), _)) if state.active.load(Ordering::Acquire) => Ok(()),
                _ => Python::attach(|py| Err(context_settled_error(py))),
            }
        })
    }

    /// Snapshot this active context into ordinary Python strings.
    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        self.ensure_active(py)?;
        let value = PyDict::new(py);
        value.set_item("kind", self.kind.as_ref())?;
        value.set_item("tenant_scope", self.tenant_scope.as_ref())?;
        value.set_item("session_id", self.session_id.as_ref())?;
        value.set_item("lane_id", self.lane_id.as_ref())?;
        value.set_item("run_id", self.run_id.as_ref())?;
        value.set_item("effect_id", self.effect_id.as_ref())?;
        value.set_item("cancelled", self.cancellation.is_cancelled())?;
        Ok(value.unbind())
    }
}

impl PyCallbackContext {
    pub(super) fn new(kind: &'static str, run: &finstack_ai::runtime::RunCallContext) -> Self {
        Self {
            state: CallbackState::new(),
            cancellation: run.cancellation.clone(),
            kind: Arc::from(kind),
            tenant_scope: Arc::clone(&run.locator.tenant_scope),
            session_id: Arc::from(run.locator.session_id.to_string()),
            lane_id: Arc::from(run.locator.lane_id.to_string()),
            run_id: Arc::from(run.locator.run_id.to_string()),
            effect_id: Arc::from(run.effect_id.to_string()),
        }
    }

    fn ensure_active(&self, py: Python<'_>) -> PyResult<()> {
        if self.state.active.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(context_settled_error(py))
        }
    }
}

fn context_settled_error(py: Python<'_>) -> PyErr {
    let exception = py.get_type::<crate::RuntimeError>();
    match exception.call1(("callback context is no longer active",)) {
        Ok(value) => {
            let _ = value.setattr("code", PYTHON_CALLBACK_CONTEXT_SETTLED);
            let _ = value.setattr("retryable", false);
            let _ = value.setattr("context", py.None());
            PyErr::from_value(value)
        }
        Err(error) => error,
    }
}
