//! Lock-time capability contribution index used as a dispatch-time mask.

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{ActiveCapability, CapabilityId, ComponentId};

use crate::{CapabilityActivation, CapabilitySpec};

/// Maps contributed components to the capability that owns them.
#[derive(Clone, Debug, Default)]
pub(super) struct CapabilityContributionIndex {
    owners: BTreeMap<ComponentId, Arc<[CapabilityId]>>,
}

impl CapabilityContributionIndex {
    pub(super) fn from_specs(capabilities: &[CapabilitySpec]) -> Self {
        let mut owners: BTreeMap<ComponentId, Vec<CapabilityId>> = BTreeMap::new();
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
                owners
                    .entry(component.id().clone())
                    .or_default()
                    .push(capability.id.clone());
            }
        }
        Self {
            owners: owners
                .into_iter()
                .map(|(component, mut ids)| {
                    ids.sort();
                    ids.dedup();
                    (component, ids.into())
                })
                .collect(),
        }
    }

    pub(super) fn owners(&self) -> &BTreeMap<ComponentId, Arc<[CapabilityId]>> {
        &self.owners
    }

    pub(super) fn allows(&self, component: &ComponentId, active: &[ActiveCapability]) -> bool {
        match self.owners.get(component) {
            None => true,
            Some(owners) => active
                .iter()
                .any(|item| owners.iter().any(|owner| &item.capability_id == owner)),
        }
    }

    pub(super) fn as_arc_owners(&self) -> Arc<BTreeMap<ComponentId, Arc<[CapabilityId]>>> {
        Arc::new(self.owners.clone())
    }
}
