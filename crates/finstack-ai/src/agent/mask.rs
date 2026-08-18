//! Lock-time capability contribution index used as a dispatch-time mask.

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{ActiveCapability, CapabilityId, ComponentId};

use crate::{CapabilityActivation, CapabilitySpec};

/// Maps contributed components to the capability that owns them.
#[derive(Clone, Debug, Default)]
pub(super) struct CapabilityContributionIndex {
    owners: BTreeMap<ComponentId, CapabilityId>,
}

impl CapabilityContributionIndex {
    pub(super) fn from_specs(capabilities: &[CapabilitySpec]) -> Self {
        let mut owners = BTreeMap::new();
        for capability in capabilities {
            if capability.activation == CapabilityActivation::Disabled {
                continue;
            }
            for component in capability
                .toolsets
                .iter()
                .chain(capability.context_providers.iter())
                .chain(capability.middleware.iter())
            {
                owners.insert(component.id().clone(), capability.id.clone());
            }
        }
        Self { owners }
    }

    pub(super) fn owners(&self) -> &BTreeMap<ComponentId, CapabilityId> {
        &self.owners
    }

    pub(super) fn allows(&self, component: &ComponentId, active: &[ActiveCapability]) -> bool {
        match self.owners.get(component) {
            None => true,
            Some(owner) => active.iter().any(|item| &item.capability_id == owner),
        }
    }

    pub(super) fn as_arc_owners(&self) -> Arc<BTreeMap<ComponentId, CapabilityId>> {
        Arc::new(self.owners.clone())
    }
}
