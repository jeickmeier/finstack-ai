//! Declarative capability activation records owned by the kernel.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::primitives::CapabilityId;
use crate::primitives::Digest;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};

/// Source that selected an active capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityActivationSource {
    /// Declared always active by the resolved agent specification.
    Always,
    /// Selected by the application before execution.
    Application,
    /// Selected by the bounded model-catalog policy.
    Model,
}

/// One active capability and its selecting source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveCapability {
    /// Capability identity.
    pub capability_id: CapabilityId,
    /// Selecting source.
    pub source: CapabilityActivationSource,
}

/// Replay-complete transition to a new immutable resolved capability plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilitiesActivated {
    /// Prior resolved-plan digest; absent only for the initial activation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_plan_digest: Option<Digest>,
    /// New resolved-plan digest.
    pub resolved_plan_digest: Digest,
    /// Complete sorted active capability set after the transition.
    pub active: Arc<[ActiveCapability]>,
}

impl CapabilitiesActivated {
    /// Validate ordering and uniqueness for one complete activation set.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.active.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err("too_many_items");
        }
        let mut prior = None;
        for item in self.active.iter() {
            if prior
                .as_ref()
                .is_some_and(|value: &&ActiveCapability| value.capability_id >= item.capability_id)
            {
                return Err("not_strictly_sorted");
            }
            prior = Some(item);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CapabilitiesActivated {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            prior_plan_digest: Option<Digest>,
            resolved_plan_digest: Digest,
            active: BoundedVec<ActiveCapability, SEMANTIC_ARRAY_MAX_ITEMS>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            prior_plan_digest: wire.prior_plan_digest,
            resolved_plan_digest: wire.resolved_plan_digest,
            active: Arc::from(wire.active.into_inner()),
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
