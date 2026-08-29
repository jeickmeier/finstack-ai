//! Python constructors for native Rust toolset extensions.

use std::sync::Arc;

use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_tools_calculator::CalculatorToolset;
use finstack_ai_tools_filesystem::FileSystemToolset;
use finstack_ai_tools_shell::{ShellPolicy, ShellToolset};
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

/// Capability-confined filesystem toolset backed by the Rust implementation.
///
/// **Security (T2 — trusted, not sandboxed):** tools run in-process with
/// the host's privileges; confinement to `root` relies on capability-safe
/// directory handles, not an OS sandbox. Reads/writes/listings are bounded
/// by the crate's default limits; `protected_paths` patterns (default:
/// the crate's secret-bearing names) are never readable or writable.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "FileSystemToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyFileSystemToolset {
    component: ComponentRef,
    inner: Arc<FileSystemToolset>,
}

#[pymethods]
impl PyFileSystemToolset {
    /// Open an explicit root directory with default bounds.
    #[new]
    #[pyo3(signature = (root, *, component = "python.tools.filesystem", protected_paths = None))]
    fn new(root: &str, component: &str, protected_paths: Option<Vec<String>>) -> PyResult<Self> {
        let mut toolset = FileSystemToolset::try_new(root)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        if let Some(patterns) = protected_paths {
            toolset = toolset
                .try_with_protected_paths(patterns)
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
        }
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

impl PyFileSystemToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.component.clone(), self.inner.clone())
    }
}

/// Deny-by-default shell execution toolset backed by the Rust implementation.
///
/// **Security (T2 — trusted, not sandboxed by default):** `shell_exec`
/// runs allowlisted programs in-process-spawned children with the host's
/// privileges. Only the listed basenames or exact paths run; everything
/// else fails closed with `shell_policy_denied`. Output and runtime are
/// bounded by the crate's default limits.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "ShellToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyShellToolset {
    component: ComponentRef,
    inner: Arc<ShellToolset>,
}

#[pymethods]
impl PyShellToolset {
    /// Build the toolset over an executable allowlist.
    #[new]
    #[pyo3(signature = (allowed, *, root = None, component = "python.tools.shell"))]
    fn new(allowed: Vec<String>, root: Option<&str>, component: &str) -> PyResult<Self> {
        let policy = ShellPolicy::try_new(allowed)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let toolset = ShellToolset::try_new(policy, root.map(std::path::Path::new))
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

impl PyShellToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.component.clone(), self.inner.clone())
    }
}
