//! Python constructor for the Rust elicitation toolset.

use std::sync::Arc;

use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_tools_elicitation::{ElicitationKind, ElicitationToolDef, ElicitationToolset};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use serde::Deserialize;

use crate::json_bridge::py_to_json;

const ELICITATION_COMPONENT: &str = "finstack.tools.elicitation";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PyElicitationToolDef {
    name: String,
    title: String,
    description: String,
    prompt: String,
    kind: PyElicitationKind,
    response_schema: serde_json::Value,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum PyElicitationKind {
    FreeText,
    Choice,
    Form,
}

impl From<PyElicitationKind> for ElicitationKind {
    fn from(kind: PyElicitationKind) -> Self {
        match kind {
            PyElicitationKind::FreeText => Self::FreeText,
            PyElicitationKind::Choice => Self::Choice,
            PyElicitationKind::Form => Self::Form,
        }
    }
}

/// Human-in-the-loop elicitation toolset backed by the Rust implementation.
///
/// `ask_user=True` exposes the free-form `ask_user` tool; `tools=[...]`
/// registers typed per-workflow elicitation tools whose response schemas are
/// fixed at registration. Calls park the run as a pending interaction; the
/// resolution response becomes the tool result.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "ElicitationToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyElicitationToolset {
    component: ComponentRef,
    inner: Arc<ElicitationToolset>,
}

#[pymethods]
impl PyElicitationToolset {
    /// Build the elicitation toolset from keyword configuration.
    #[new]
    #[pyo3(signature = (*, ask_user = false, tools = None))]
    fn new(ask_user: bool, tools: Option<Vec<Bound<'_, PyAny>>>) -> PyResult<Self> {
        let mut builder = ElicitationToolset::builder();
        if ask_user {
            builder = builder.with_ask_user();
        }
        for tool in tools.unwrap_or_default() {
            let value = py_to_json(&tool)?;
            let def: PyElicitationToolDef = serde_json::from_value(value).map_err(|error| {
                PyTypeError::new_err(format!("invalid elicitation tool definition: {error}"))
            })?;
            builder = builder.tool(ElicitationToolDef {
                name: def.name,
                title: def.title,
                description: def.description,
                prompt: def.prompt,
                kind: def.kind.into(),
                response_schema: def.response_schema,
            });
        }
        let toolset = builder
            .build()
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let component = ComponentId::parse(ELICITATION_COMPONENT)
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
            .map_err(|_| PyValueError::new_err("elicitation component id is invalid"))?;
        Ok(Self {
            component,
            inner: Arc::new(toolset),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }

    /// Number of elicitation tools exposed to the model.
    #[getter]
    fn tool_count(&self) -> usize {
        self.inner.tools().len()
    }
}

impl PyElicitationToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.component.clone(), self.inner.clone())
    }
}
