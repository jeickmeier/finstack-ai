//! Python constructors for native Rust middleware extensions.

use std::sync::Arc;

use finstack_ai::runtime::ports::middleware::Middleware;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_middleware_instructions::{
    InstructionsMiddleware, PolicyEntry, PolicyInstructionsConfig,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

const INSTRUCTIONS_COMPONENT: &str = "finstack.middleware.instructions";

/// `InstructionsMiddleware`'s declared invocation version
/// (`INSTRUCTIONS_VERSION` in `finstack-ai-middleware-instructions::lib`).
/// `validate_middleware_descriptor` requires the registered `ComponentRef`
/// to match the handle's own reported `(component id, version)` exactly.
const INSTRUCTIONS_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Frozen policy-instruction middleware backed by the Rust implementation.
///
/// Each `(label, text)` entry becomes one protected System context item
/// injected at `prepare_context`, with provenance source id
/// `policy:{label}`. Entries are validated and frozen at construction; at
/// most 16 entries, labels at most 249 bytes.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "InstructionsMiddleware",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyInstructionsMiddleware {
    component: ComponentRef,
    inner: Arc<InstructionsMiddleware>,
}

#[pymethods]
impl PyInstructionsMiddleware {
    /// Build the middleware from ordered `(label, text)` policy entries.
    #[new]
    #[pyo3(signature = (entries))]
    fn new(entries: Vec<(String, String)>) -> PyResult<Self> {
        let config = PolicyInstructionsConfig {
            entries: entries
                .into_iter()
                .map(|(label, text)| PolicyEntry { label, text })
                .collect(),
        };
        let middleware = InstructionsMiddleware::try_new(config)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(INSTRUCTIONS_COMPONENT)
            .map(|id| ComponentRef::new(id, Some(INSTRUCTIONS_VERSION)))
            .map_err(|_| PyValueError::new_err("instructions component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(middleware),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PyInstructionsMiddleware {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Middleware>) {
        (self.component.clone(), self.inner.clone())
    }
}
