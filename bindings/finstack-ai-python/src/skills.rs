//! Python constructor for the native skills toolset (capability activation).
//!
//! The skills toolset gives the model `capability_list` and
//! `capability_activate` tools over the agent's declared model-activated
//! capability catalog. The activation bridge — a `NativeCapabilityHost`
//! whose compact catalog is assembled from the declared capabilities at
//! agent build, wired to the toolset via the crate's `SkillsHost`
//! callbacks — lives here so no Python host rewrites it (it was first
//! proven in `apps/finstack-knowledge/src/compose.rs`).

use std::sync::Arc;

use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai::{AgentRunError, CapabilityActivation, CapabilitySpec, NativeCapabilityHost};
use finstack_ai_kernel::{ComponentId, ComponentRef, Version};
use finstack_ai_tools_skills::{SkillsHost, SkillsHostError, SkillsToolset};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::errors::configuration_error;

/// Caller-named toolset registrations use the binding's stable version.
const SKILLS_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Deferred native skills toolset for model-driven capability activation.
///
/// Unlike the other toolset wrappers this handle is a marker: the real
/// `SkillsToolset` and its `NativeCapabilityHost` are built at agent
/// assembly, when the declared capabilities (and therefore the compact
/// catalog) are known. Only `Agent.from_python` supports it today — the
/// linked provider factories build through a path without builder access
/// and reject it with a clear error.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "SkillsToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PySkillsToolset {
    component: ComponentRef,
}

#[pymethods]
impl PySkillsToolset {
    /// Declare the skills toolset under a caller-chosen component name.
    #[new]
    #[pyo3(signature = (component = "python.tools.skills"))]
    fn new(component: &str) -> PyResult<Self> {
        let component = ComponentId::parse(component)
            .map(|id| ComponentRef::new(id, Some(SKILLS_VERSION)))
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self { component })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PySkillsToolset {
    pub(crate) fn component_ref(&self) -> ComponentRef {
        self.component.clone()
    }
}

/// Compact `id: description` catalog over the model-activated capabilities,
/// matching `Agent::compact_capability_catalog`'s line format.
fn model_capability_catalog(capabilities: &[CapabilitySpec]) -> String {
    capabilities
        .iter()
        .filter(|capability| capability.activation == CapabilityActivation::Model)
        .map(|capability| format!("{}: {}", capability.id, capability.description))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Skills toolset registration plus the activation host to attach via
/// `capability_activation_host`.
pub(crate) type SkillsPorts = ((ComponentRef, Arc<dyn Toolset>), Arc<NativeCapabilityHost>);

/// Build the skills toolset registration and activation host.
pub(crate) fn build_skills_ports(
    component: ComponentRef,
    capabilities: &[CapabilitySpec],
) -> Result<SkillsPorts, AgentRunError> {
    let host = Arc::new(NativeCapabilityHost::new(model_capability_catalog(
        capabilities,
    )));
    let toolset = SkillsToolset::try_new(skills_host(&host))
        .map_err(|error| configuration_error(error.to_string()))?;
    Ok(((component, Arc::new(toolset) as Arc<dyn Toolset>), host))
}

fn skills_host(host: &Arc<NativeCapabilityHost>) -> SkillsHost {
    let catalog = host.compact_catalog().to_owned();
    let active = Arc::clone(host);
    let activate = Arc::clone(host);
    SkillsHost {
        catalog,
        active: Arc::new(move |run| {
            active.active(run).map_err(|error| SkillsHostError::Failed {
                reason: error.to_string().into(),
            })
        }),
        activate: Arc::new(move |run, complete| {
            activate
                .queue_activation(run, complete)
                .map_err(|error| SkillsHostError::Failed {
                    reason: error.to_string().into(),
                })
        }),
    }
}
