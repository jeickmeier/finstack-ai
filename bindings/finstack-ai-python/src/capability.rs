//! Data-only declarative capability accepted by both Python agent factories.

use std::sync::Arc;

use finstack_ai::{CapabilityActivation, CapabilitySpec, InstructionSpec};
use finstack_ai_kernel::CapabilityId;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;

/// Instruction-only declarative capability accepted by both Python agent factories.
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
    #[pyo3(signature = (id, description, instructions, *, activation = "application"))]
    fn new(
        id: String,
        description: String,
        instructions: Vec<String>,
        activation: &str,
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
            toolsets: Arc::from([]),
            context_providers: Arc::from([]),
            middleware: Arc::from([]),
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
