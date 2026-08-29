//! Python constructors for native Rust toolset extensions.

use std::sync::Arc;

use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_tools_calculator::CalculatorToolset;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// Caller-named toolset registrations use the binding's stable version.
const TOOLSET_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

fn caller_component(id: &str) -> PyResult<ComponentRef> {
    ComponentId::parse(id)
        .map(|id| ComponentRef::new(id, Some(TOOLSET_VERSION)))
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

/// Deterministic arithmetic toolset backed by the Rust implementation.
///
/// Exposes one `calculator` tool taking `{"operation": "add" | "subtract"
/// | "multiply" | "divide", "operands": [number, ...]}`.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "CalculatorToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyCalculatorToolset {
    component: ComponentRef,
    inner: Arc<CalculatorToolset>,
}

#[pymethods]
impl PyCalculatorToolset {
    /// Construct the calculator toolset.
    #[new]
    #[pyo3(signature = (component = "python.tools.calculator"))]
    fn new(component: &str) -> PyResult<Self> {
        let toolset = CalculatorToolset::try_new()
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            component: caller_component(component)?,
            inner: Arc::new(toolset),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PyCalculatorToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.component.clone(), self.inner.clone())
    }
}
