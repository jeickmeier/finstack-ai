//! Python constructors for native Rust middleware extensions.

use std::sync::Arc;

use finstack_ai::runtime::ports::middleware::Middleware;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_middleware_compaction::{CompactionConfig, CompactionMiddleware};
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

const COMPACTION_COMPONENT: &str = "finstack.middleware.compaction";

/// `CompactionMiddleware`'s declared invocation version (see the crate's
/// checked-in `MiddlewareDescriptor`); the registered `ComponentRef` must
/// match it exactly.
const COMPACTION_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 4,
};

/// Deterministic context-compaction middleware backed by the Rust
/// implementation.
///
/// One instance owns one strategy; construct via the strategy factories.
/// The `summarize` strategy (model-assisted) is not exposed from Python
/// yet — it needs an authorized model reference and budget scope.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "CompactionMiddleware",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyCompactionMiddleware {
    component: ComponentRef,
    inner: Arc<CompactionMiddleware>,
}

impl PyCompactionMiddleware {
    fn from_config(config: CompactionConfig) -> PyResult<Self> {
        let middleware = CompactionMiddleware::try_new(config)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(COMPACTION_COMPONENT)
            .map(|id| ComponentRef::new(id, Some(COMPACTION_VERSION)))
            .map_err(|_| PyValueError::new_err("compaction component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(middleware),
        })
    }

    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Middleware>) {
        (self.component.clone(), self.inner.clone())
    }
}

#[pymethods]
impl PyCompactionMiddleware {
    /// Drop the oldest unprotected context above the token threshold.
    #[staticmethod]
    #[pyo3(signature = (threshold_tokens, hysteresis_tokens))]
    fn sliding_window(threshold_tokens: u64, hysteresis_tokens: u64) -> PyResult<Self> {
        Self::from_config(CompactionConfig::sliding_window(
            threshold_tokens,
            hysteresis_tokens,
        ))
    }

    /// Truncate large tool-result bodies while keeping call/result pairs.
    #[staticmethod]
    #[pyo3(signature = (threshold_tokens, hysteresis_tokens, max_body_bytes))]
    fn large_tool_output(
        threshold_tokens: u64,
        hysteresis_tokens: u64,
        max_body_bytes: usize,
    ) -> PyResult<Self> {
        Self::from_config(CompactionConfig::large_tool_output(
            threshold_tokens,
            hysteresis_tokens,
            max_body_bytes,
        ))
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}
