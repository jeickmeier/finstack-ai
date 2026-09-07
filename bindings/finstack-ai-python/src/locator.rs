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
    /// Reconstruct a validated immutable locator from persisted identifiers.
    #[staticmethod]
    fn from_dict(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        let locator = serde_json::from_value(crate::json_bridge::py_to_json(value)?)
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            locator: Arc::new(locator),
        })
    }

    /// Tenant scope that owns the accepted operation.
    #[getter]
    fn tenant_scope(&self) -> &str {
        &self.locator.tenant_scope
    }

    /// Durable session identity.
    #[getter]
    fn session_id(&self) -> String {
        self.locator.session_id.to_string()
    }

    /// Durable lane identity.
    #[getter]
    fn lane_id(&self) -> String {
        self.locator.lane_id.to_string()
    }

    /// Durable run identity.
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
