//! Credential-free exact reconstruction locks.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{BundleId, CapabilityId, ComponentRef, Digest, SchemaRef, Version};
use serde::{Deserialize, Serialize};

use crate::{CapabilityActivation, ComponentKind};

use super::types::MAX_ITEMS;
use super::{BUNDLE_SCHEMA_VERSION, BundleResolutionError};

/// Exact bundle identity captured in a resolution lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedBundle {
    /// Bundle identity.
    pub id: BundleId,
    /// Exact bundle version.
    pub version: Version,
}

/// Primary component role captured without serializing executable handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockedComponentKind {
    /// Model.
    Model,
    /// Toolset.
    Toolset,
    /// Context provider.
    ContextProvider,
    /// Middleware.
    Middleware,
    /// Journal store.
    Store,
    /// Observer.
    Observer,
}

impl From<ComponentKind> for LockedComponentKind {
    fn from(value: ComponentKind) -> Self {
        match value {
            ComponentKind::Model => Self::Model,
            ComponentKind::Toolset => Self::Toolset,
            ComponentKind::ContextProvider => Self::ContextProvider,
            ComponentKind::Middleware => Self::Middleware,
            ComponentKind::Store => Self::Store,
            ComponentKind::Observer => Self::Observer,
        }
    }
}

/// Exact selected component and schema/config evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedComponent {
    /// Exact component reference; version is required in a valid lock.
    pub component: ComponentRef,
    /// Selected primary role.
    pub kind: LockedComponentKind,
    /// Effective non-secret configuration digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_digest: Option<Digest>,
    /// Exact configuration schema reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_schema: Option<SchemaRef>,
}

/// Exact declarative capability selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedCapability {
    /// Capability identity.
    pub id: CapabilityId,
    /// Exact source bundle.
    pub bundle: LockedBundle,
    /// Declared activation mode.
    pub activation: CapabilityActivation,
    /// Whether this capability contributes to the current immutable plan.
    pub active: bool,
    /// Canonical capability-definition digest.
    pub definition_digest: Digest,
}

/// Non-primary service requirements selected by composition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RequiredServices {
    /// Child invocation service required.
    #[serde(default)]
    pub agent_invoker: bool,
    /// Shared-budget ledger required.
    #[serde(default)]
    pub budget_ledger: bool,
    /// Scoped artifact store required.
    #[serde(default)]
    pub artifact_store: bool,
}

/// Canonical credential-free reconstruction lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedAgentLock {
    /// Lock schema version.
    pub schema_version: u16,
    /// Exact framework engine version.
    pub engine_version: Version,
    /// Canonical base `AgentSpec` digest.
    pub agent_spec_digest: Digest,
    /// Exact source bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<LockedBundle>,
    /// Exact selected components.
    pub components: Arc<[LockedComponent]>,
    /// Exact capability definitions and active plan membership.
    pub capabilities: Arc<[LockedCapability]>,
    /// Effective non-secret configuration digest.
    pub effective_config_digest: Digest,
    /// Runtime-resolved middleware descriptor-chain digest.
    pub middleware_chain_digest: Digest,
    /// Exact referenced schema digests.
    pub schema_digests: Arc<[Digest]>,
    /// Required non-primary service presence.
    pub required_services: RequiredServices,
}

impl ResolvedAgentLock {
    /// Validate schema version, exact component versions, and duplicate identities.
    ///
    /// # Errors
    ///
    /// Returns a strict lock error before a run starts.
    pub fn validate(&self) -> Result<(), BundleResolutionError> {
        if self.schema_version != BUNDLE_SCHEMA_VERSION {
            return Err(BundleResolutionError::Invalid {
                message: Arc::from("unsupported_lock_schema_version"),
            });
        }
        for (field, len) in [
            ("components", self.components.len()),
            ("capabilities", self.capabilities.len()),
            ("schema_digests", self.schema_digests.len()),
        ] {
            if len > MAX_ITEMS {
                return Err(BundleResolutionError::Invalid {
                    message: Arc::from(format!("lock_{field}_too_many_items")),
                });
            }
        }
        let mut components = BTreeSet::new();
        for component in self.components.iter() {
            if component.component.version().is_none()
                || !components.insert(component.component.id().clone())
            {
                return Err(BundleResolutionError::Invalid {
                    message: Arc::from("lock_component_not_exact_or_duplicate"),
                });
            }
        }
        let mut capabilities = BTreeSet::new();
        if self
            .capabilities
            .iter()
            .any(|capability| !capabilities.insert(capability.id.clone()))
        {
            return Err(BundleResolutionError::Invalid {
                message: Arc::from("duplicate_locked_capability"),
            });
        }
        if self
            .schema_digests
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(BundleResolutionError::Invalid {
                message: Arc::from("schema_digests_not_sorted_unique"),
            });
        }
        Ok(())
    }

    /// Canonical JSON export.
    ///
    /// # Errors
    ///
    /// Returns a strict serialization failure.
    pub fn to_json(&self) -> Result<Vec<u8>, BundleResolutionError> {
        self.validate()?;
        serde_json_canonicalizer::to_vec(self).map_err(|error| BundleResolutionError::Invalid {
            message: Arc::from(error.to_string()),
        })
    }

    /// Strict JSON import.
    ///
    /// # Errors
    ///
    /// Rejects unknown fields, unsupported versions, and non-exact selections.
    pub fn from_json(bytes: &[u8]) -> Result<Self, BundleResolutionError> {
        let value: Self =
            serde_json::from_slice(bytes).map_err(|error| BundleResolutionError::Invalid {
                message: Arc::from(error.to_string()),
            })?;
        value.validate()?;
        Ok(value)
    }

    /// Canonical lock fingerprint used as the resolved-plan digest.
    ///
    /// # Errors
    ///
    /// Returns a strict serialization failure.
    pub fn fingerprint(&self) -> Result<Digest, BundleResolutionError> {
        let bytes = self.to_json()?;
        Digest::domain_separated("resolved-agent-lock", 1, &bytes).map_err(|error| {
            BundleResolutionError::Invalid {
                message: Arc::from(error.to_string()),
            }
        })
    }
}
