//! Python constructor for the composable E2B sandbox toolset.

use std::sync::Arc;

use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_sandbox_e2b::{E2bSandboxConfig, E2bSandboxToolset};
use pyo3::prelude::*;

const COMPONENT: &str = "finstack.tools.e2b";

/// T4 E2B toolset. Attach this to an agent with a real model provider.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "E2bSandboxToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyE2bSandboxToolset {
    component: ComponentRef,
    inner: Arc<E2bSandboxToolset>,
}

#[pymethods]
impl PyE2bSandboxToolset {
    #[new]
    #[pyo3(signature = (api_key, *, endpoint = None, template = None))]
    fn new(api_key: String, endpoint: Option<String>, template: Option<String>) -> PyResult<Self> {
        let inner = E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key,
            endpoint: endpoint.unwrap_or_default(),
            template,
        })
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(COMPONENT)
            .map(|id| {
                ComponentRef::new(
                    id,
                    Some(Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    }),
                )
            })
            .map_err(|_| pyo3::exceptions::PyValueError::new_err("e2b component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(inner),
        })
    }

    /// Stable component identifier for the E2B sandbox toolset.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }

    /// Number of E2B sandbox tools exposed to the model.
    #[getter]
    fn tool_count(&self) -> usize {
        self.inner.tools().len()
    }
}

impl PyE2bSandboxToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.component.clone(), self.inner.clone())
    }
}
