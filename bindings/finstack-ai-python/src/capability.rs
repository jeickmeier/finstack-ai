//! Declarative capability accepted by every Python agent factory.

use std::sync::Arc;

use finstack_ai::{CapabilityActivation, CapabilitySpec, InstructionSpec};
use finstack_ai_kernel::{CapabilityId, ComponentId, ComponentRef};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;

/// Declarative capability: bounded instructions plus optional component
/// references gating registered toolsets/providers/middleware.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "Capability",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyCapability {
    pub(crate) inner: CapabilitySpec,
}

#[pymethods]
impl PyCapability {
    /// Construct one bounded declarative capability.
    #[new]
    #[pyo3(signature = (id, description, instructions, *, activation = "application", toolsets = None, context_providers = None, middleware = None))]
    fn new(
        id: String,
        description: String,
        instructions: Vec<String>,
        activation: &str,
        toolsets: Option<Vec<String>>,
        context_providers: Option<Vec<String>>,
        middleware: Option<Vec<String>>,
    ) -> PyResult<Self> {
        let activation = match activation {
            "always" => CapabilityActivation::Always,
            "application" => CapabilityActivation::Application,
            "model" => CapabilityActivation::Model,
            "disabled" => CapabilityActivation::Disabled,
            _ => {
                return Err(PyTypeError::new_err(
                    "activation must be always, application, model, or disabled",
                ));
            }
        };
        let instructions = instructions
            .into_iter()
            .map(InstructionSpec::try_new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| PyTypeError::new_err(error.to_string()))?;
        let inner = CapabilitySpec {
            id: CapabilityId::parse(id).map_err(|error| PyTypeError::new_err(error.to_string()))?,
            description: Arc::from(description),
            instructions: instructions.into(),
            toolsets: component_refs(toolsets)?,
            context_providers: component_refs(context_providers)?,
            middleware: component_refs(middleware)?,
            activation,
        };
        inner
            .validate()
            .map_err(|error| PyTypeError::new_err(error.to_string()))?;
        Ok(Self { inner })
    }

    /// Stable capability identifier used for activation.
    #[getter]
    fn id(&self) -> &str {
        self.inner.id.as_str()
    }

    /// Compact model-visible capability description.
    #[getter]
    fn description(&self) -> &str {
        &self.inner.description
    }

    /// Activation owner selected when the capability was declared.
    #[getter]
    fn activation(&self) -> &'static str {
        match self.inner.activation {
            CapabilityActivation::Always => "always",
            CapabilityActivation::Application => "application",
            CapabilityActivation::Model => "model",
            CapabilityActivation::Disabled => "disabled",
        }
    }
}

/// Convert component-name strings into unversioned capability references.
///
/// Capability gating matches registered components by id alone, so the
/// references carry no version; the named components must be registered on
/// the same agent (for toolsets, usually via ``capability_toolsets=``).
fn component_refs(names: Option<Vec<String>>) -> PyResult<Arc<[ComponentRef]>> {
    Ok(names
        .unwrap_or_default()
        .into_iter()
        .map(|name| {
            ComponentId::parse(&name)
                .map(|id| ComponentRef::new(id, None))
                .map_err(|error| PyTypeError::new_err(error.to_string()))
        })
        .collect::<PyResult<Vec<_>>>()?
        .into())
}
