//! Python constructors for native Rust toolset extensions.

use std::sync::Arc;

use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::{ComponentRef, Version};
use finstack_ai_tools_calculator::CalculatorToolset;
use finstack_ai_tools_filesystem::FileSystemToolset;
use finstack_ai_tools_mcp::{McpConfig, McpToolset, McpToolsetFactory, StdioConfig};
use finstack_ai_tools_shell::{ShellPolicy, ShellToolset};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::component_ref;

/// Caller-named toolset registrations use the binding's stable version.
const TOOLSET_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

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
            component: component_ref(component, TOOLSET_VERSION)?,
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
            component: component_ref(component, TOOLSET_VERSION)?,
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
            component: component_ref(component, TOOLSET_VERSION)?,
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

/// Model Context Protocol client toolset backed by the Rust implementation.
///
/// Connects to one allowlisted stdio MCP server at construction,
/// enumerates `tools/list`, and freezes the catalog. Classification is
/// fail-closed: tools are non-idempotent writes requiring approval unless
/// the host names them in `read_only_tools` / `idempotent_tools` (server
/// annotations never grant this).
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "McpToolset",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyMcpToolset {
    component: ComponentRef,
    inner: Arc<McpToolset>,
}

#[pymethods]
impl PyMcpToolset {
    /// Connect to one stdio MCP server and freeze its tool catalog.
    #[staticmethod]
    #[pyo3(signature = (program, args = None, *, component = "python.tools.mcp", read_only_tools = None, idempotent_tools = None))]
    fn stdio<'py>(
        py: Python<'py>,
        program: String,
        args: Option<Vec<String>>,
        component: &str,
        read_only_tools: Option<Vec<String>>,
        idempotent_tools: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let component = component_ref(component, TOOLSET_VERSION)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut config = McpConfig::default().allow_command(&program);
            if let Some(names) = read_only_tools {
                config = config.with_read_only_tools(names);
            }
            if let Some(names) = idempotent_tools {
                config = config.with_idempotent_tools(names);
            }
            let config = config
                .stdio(StdioConfig::new(program, args.unwrap_or_default()))
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
            let toolset = McpToolsetFactory::new(config)
                .construct()
                .await
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
            Python::attach(|py| {
                Py::new(
                    py,
                    Self {
                        component,
                        inner: Arc::new(toolset),
                    },
                )
            })
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PyMcpToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.component.clone(), self.inner.clone())
    }
}
