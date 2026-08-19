//! Immutable operation identity snapshot.

use std::sync::Arc;

use finstack_ai_kernel::OperationLocator;
use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Immutable operation identity snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Locator", frozen)]
pub(crate) struct PyLocator {
    pub(crate) locator: Arc<OperationLocator>,
}

#[pymethods]
impl PyLocator {
    #[getter]
    fn tenant_scope(&self) -> &str {
        &self.locator.tenant_scope
    }

    #[getter]
    fn session_id(&self) -> String {
        self.locator.session_id.to_string()
    }

    #[getter]
    fn lane_id(&self) -> String {
        self.locator.lane_id.to_string()
    }

    #[getter]
    fn run_id(&self) -> String {
        self.locator.run_id.to_string()
    }

    /// Serialize the immutable snapshot on explicit request.
    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        locator_dict(py, &self.locator)
    }
}

pub(crate) fn locator_dict(py: Python<'_>, locator: &OperationLocator) -> PyResult<Py<PyDict>> {
    let context = PyDict::new(py);
    context.set_item("tenant_scope", locator.tenant_scope.as_ref())?;
    context.set_item("session_id", locator.session_id.to_string())?;
    context.set_item("lane_id", locator.lane_id.to_string())?;
    context.set_item("run_id", locator.run_id.to_string())?;
    Ok(context.unbind())
}
