//! Python constructor for the Rust repository-instructions context provider.

use std::sync::Arc;

use finstack_ai::runtime::ports::context::ContextProvider;
use finstack_ai_context_repository::RepositoryContextProvider;
use finstack_ai_kernel::{ComponentRef, Version};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::component_ref;

const REPOSITORY_COMPONENT: &str = "finstack.context.repository";

/// `RepositoryContextProvider`'s declared invocation version (see the
/// crate's checked-in `ContextProviderDescriptor`); the registered
/// `ComponentRef` must match it exactly.
const REPOSITORY_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 4,
};

/// Repository-instructions context provider backed by the Rust implementation.
///
/// Reads an allowlisted set of instruction files under one explicit root
/// directory (default: `AGENTS.md`, `README.md`,
/// `.finstack/instructions.md`) and contributes them as context items.
/// Reads are confined to the root via capability-safe directory handles;
/// symlinked escapes and traversing names are rejected at construction.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "RepositoryContextProvider",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyRepositoryContextProvider {
    component: ComponentRef,
    inner: Arc<RepositoryContextProvider>,
}

#[pymethods]
impl PyRepositoryContextProvider {
    /// Open `root` with the default or an explicit filename allowlist.
    #[new]
    #[pyo3(signature = (root, allowlist = None))]
    fn new(root: &str, allowlist: Option<Vec<String>>) -> PyResult<Self> {
        let provider = match allowlist {
            Some(names) => RepositoryContextProvider::try_with_allowlist(root, names),
            None => RepositoryContextProvider::try_new(root),
        }
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            component: component_ref(REPOSITORY_COMPONENT, REPOSITORY_VERSION)?,
            inner: Arc::new(provider),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PyRepositoryContextProvider {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn ContextProvider>) {
        (self.component.clone(), self.inner.clone())
    }
}
