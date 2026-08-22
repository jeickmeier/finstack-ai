use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::model::ToolSpec;
use finstack_ai::runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolStreamItem, Toolset,
    ToolsetDescriptor,
};
use finstack_ai_kernel::{ArtifactRef, ComponentRef, ErrorCategory, Metadata, RawJson};
use futures_util::stream;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use serde::Deserialize;

use super::context::PyCallbackContext;
use super::engine::{CallbackFailure, PythonCallback};
use super::shared::{configuration_error, exact_component, python_value};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PythonToolOutput {
    output: serde_json::Value,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    artifacts: Vec<ArtifactRef>,
}

struct PythonToolsetAdapter {
    callback: Arc<PythonCallback>,
    component: ComponentRef,
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
}

impl Toolset for PythonToolsetAdapter {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: finstack_ai_kernel::ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let callback = Arc::clone(&self.callback);
        Box::pin(async move {
            let context = PyCallbackContext::new("toolset", &ctx.run);
            let output: PythonToolOutput = callback
                .invoke(context, &call)
                .await
                .map_err(tool_failure)?;
            let bytes = serde_json::to_vec(&output.output)
                .map_err(|_| tool_failure(CallbackFailure::InvalidResult))?;
            let result = ToolResult {
                output: RawJson::parse(bytes)
                    .map_err(|_| tool_failure(CallbackFailure::InvalidResult))?,
                is_error: output.is_error,
            };
            let items = output
                .artifacts
                .into_iter()
                .map(ToolStreamItem::Artifact)
                .chain(core::iter::once(ToolStreamItem::Completed(result)))
                .map(Ok)
                .collect::<Vec<_>>();
            Ok(Box::pin(stream::iter(items)) as ToolEventStream)
        })
    }
}

fn tool_failure(failure: CallbackFailure) -> ToolError {
    ToolError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Tool),
        false,
        failure.message(),
        Metadata::empty(),
    )
    .unwrap_or_else(ToolError::from)
}

/// Trusted Python implementation of the Toolset port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonToolset {
    inner: Arc<PythonToolsetAdapter>,
}

#[pymethods]
impl PyPythonToolset {
    /// Register one trusted coarse Python tool callback and cached tool schemas.
    #[new]
    #[pyo3(signature = (callback, *, component, name, tools, callback_timeout_seconds = 30.0))]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        name: String,
        tools: Py<PyAny>,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        let callback = Arc::new(PythonCallback::try_new(
            py,
            callback,
            callback_timeout_seconds,
        )?);
        let tools = python_value::<Vec<ToolSpec>>(py, &callback, tools)?;
        if tools.is_empty() {
            return Err(PyTypeError::new_err("tools must not be empty"));
        }
        for tool in &tools {
            tool.validate().map_err(configuration_error)?;
        }
        Ok(Self {
            inner: Arc::new(PythonToolsetAdapter {
                callback,
                component,
                descriptor: ToolsetDescriptor {
                    name: Arc::from(name),
                    metadata: Metadata::empty(),
                },
                tools: tools.into(),
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.component.id().to_string()
    }

    /// Return the number of schemas cached at registration.
    #[getter]
    fn tool_count(&self) -> usize {
        self.inner.tools.len()
    }
}

impl PyPythonToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.inner.component.clone(), self.inner.clone())
    }
}
