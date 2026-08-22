//! Bundle specification types and validate-on-deserialize.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{BundleId, CapabilityId, ComponentId, RawJson, SchemaRef, Version};
use serde::{Deserialize, Serialize, de};

use crate::{AgentSpec, CapabilitySpec};

use super::{BUNDLE_SCHEMA_VERSION, BundleResolutionError};

pub(super) const MAX_ITEMS: usize = 4_096;
pub(super) const MAX_CONFIG_ENTRIES: usize = 256;

/// Finite component-version requirement; this is not a package solver range language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum VersionRequirement {
    /// Require one exact semantic version.
    Exact {
        /// Required version.
        version: Version,
    },
    /// Require one registered version with the given major.
    CompatibleMajor {
        /// Required major version.
        major: u16,
    },
    /// Require `min_inclusive <= version < max_exclusive`.
    Range {
        /// Inclusive lower bound.
        min_inclusive: Version,
        /// Exclusive upper bound.
        max_exclusive: Version,
    },
}

impl VersionRequirement {
    pub(super) fn matches(&self, version: Version) -> bool {
        match self {
            Self::Exact { version: expected } => version == *expected,
            Self::CompatibleMajor { major } => version.major == *major,
            Self::Range {
                min_inclusive,
                max_exclusive,
            } => version >= *min_inclusive && version < *max_exclusive,
        }
    }

    fn validate(&self) -> bool {
        match self {
            Self::Range {
                min_inclusive,
                max_exclusive,
            } => min_inclusive < max_exclusive,
            Self::Exact { .. } | Self::CompatibleMajor { .. } => true,
        }
    }
}

/// Host capability selected during bundle resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum HostFeature {
    /// Child-agent invocation service.
    AgentInvoker,
    /// Shared-budget ledger service.
    BudgetLedger,
    /// Scoped artifact storage service.
    ArtifactStore,
    /// Application-defined non-secret host feature.
    Custom {
        /// Namespaced feature identity.
        id: ComponentId,
    },
}

/// Finite bundle dependency requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum BundleRequirement {
    /// Required component and compatible version.
    RequiredComponent {
        /// Component identity.
        component: ComponentId,
        /// Finite version requirement.
        version: VersionRequirement,
    },
    /// Optional component; if installed, its version must match.
    OptionalComponent {
        /// Component identity.
        component: ComponentId,
        /// Finite version requirement.
        version: VersionRequirement,
    },
    /// Required capability definition in this exact bundle.
    RequiredCapability {
        /// Capability identity.
        capability: CapabilityId,
    },
    /// Exactly one registered component from a finite alternatives list.
    OneOfComponents {
        /// Finite component alternatives.
        alternatives: Arc<[ComponentId]>,
    },
    /// Required host feature/service.
    RequiredHostFeature {
        /// Required feature.
        feature: HostFeature,
    },
    /// Minimum framework contract version.
    MinimumFrameworkContract {
        /// Minimum inclusive engine version.
        version: Version,
    },
}

/// Finite incompatible installed identity or host feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum BundleConflict {
    /// Installed component must be absent.
    Component {
        /// Conflicting component.
        component: ComponentId,
    },
    /// Capability definition must be absent from this bundle.
    Capability {
        /// Conflicting capability.
        capability: CapabilityId,
    },
    /// Host feature must be absent.
    HostFeature {
        /// Conflicting feature.
        feature: HostFeature,
    },
}

/// Bundle-owned default extension configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleDefaults {
    /// Default configuration by component identity.
    #[serde(default)]
    pub extension_config: BTreeMap<ComponentId, RawJson>,
}

/// Bundle-level compatibility requirements.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityRequirements {
    /// Required host features in addition to explicit requirements.
    #[serde(default)]
    pub required_host_features: BTreeSet<HostFeature>,
    /// Minimum engine version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_engine_version: Option<Version>,
}

/// Strict finite bundle specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSpec {
    /// JSON schema version.
    pub schema_version: u16,
    /// Bundle identity.
    pub id: BundleId,
    /// Exact installed bundle version.
    pub version: Version,
    /// Agent definitions.
    pub agents: Arc<[AgentSpec]>,
    /// Capability definitions.
    pub capabilities: Arc<[CapabilitySpec]>,
    /// Finite requirements.
    pub requirements: Arc<[BundleRequirement]>,
    /// Finite conflicts.
    pub conflicts: Arc<[BundleConflict]>,
    /// Bundle configuration defaults.
    pub defaults: BundleDefaults,
    /// Optional bundle configuration schema reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<SchemaRef>,
    /// Host/framework compatibility requirements.
    pub compatibility: CompatibilityRequirements,
}

impl BundleSpec {
    /// Validate schema version, bounds, finite alternatives, and duplicate IDs.
    ///
    /// # Errors
    ///
    /// Returns a strict pre-run resolution error.
    pub fn validate(&self) -> Result<(), BundleResolutionError> {
        if self.schema_version != BUNDLE_SCHEMA_VERSION {
            return Err(BundleResolutionError::Invalid {
                message: Arc::from("unsupported_bundle_schema_version"),
            });
        }
        for (field, len) in [
            ("agents", self.agents.len()),
            ("capabilities", self.capabilities.len()),
            ("requirements", self.requirements.len()),
            ("conflicts", self.conflicts.len()),
        ] {
            if len > MAX_ITEMS {
                return Err(BundleResolutionError::Invalid {
                    message: Arc::from(format!("{field}_too_many_items")),
                });
            }
        }
        if self.defaults.extension_config.len() > MAX_CONFIG_ENTRIES {
            return Err(BundleResolutionError::Invalid {
                message: Arc::from("bundle_defaults_too_many_entries"),
            });
        }
        let mut agents = BTreeSet::new();
        for agent in self.agents.iter() {
            agent
                .validate()
                .map_err(|error| BundleResolutionError::Invalid {
                    message: Arc::from(error.to_string()),
                })?;
            if !agents.insert(agent.id.clone()) {
                return Err(BundleResolutionError::Invalid {
                    message: Arc::from("duplicate_agent_id"),
                });
            }
        }
        let mut capabilities = BTreeSet::new();
        for capability in self.capabilities.iter() {
            capability
                .validate()
                .map_err(|error| BundleResolutionError::Invalid {
                    message: Arc::from(error.to_string()),
                })?;
            if !capabilities.insert(capability.id.clone()) {
                return Err(BundleResolutionError::Invalid {
                    message: Arc::from("duplicate_capability_id"),
                });
            }
        }
        for requirement in self.requirements.iter() {
            match requirement {
                BundleRequirement::RequiredComponent { version, .. }
                | BundleRequirement::OptionalComponent { version, .. }
                    if !version.validate() =>
                {
                    return Err(BundleResolutionError::Invalid {
                        message: Arc::from("invalid_version_range"),
                    });
                }
                BundleRequirement::OneOfComponents { alternatives }
                    if alternatives.is_empty()
                        || alternatives.len() > MAX_ITEMS
                        || alternatives.iter().collect::<BTreeSet<_>>().len()
                            != alternatives.len() =>
                {
                    return Err(BundleResolutionError::Invalid {
                        message: Arc::from("invalid_one_of_components"),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for BundleSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema_version: u16,
            id: BundleId,
            version: Version,
            agents: Arc<[AgentSpec]>,
            #[serde(default)]
            capabilities: Arc<[CapabilitySpec]>,
            #[serde(default)]
            requirements: Arc<[BundleRequirement]>,
            #[serde(default)]
            conflicts: Arc<[BundleConflict]>,
            #[serde(default)]
            defaults: BundleDefaults,
            #[serde(default)]
            config_schema: Option<SchemaRef>,
            #[serde(default)]
            compatibility: CompatibilityRequirements,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema_version: wire.schema_version,
            id: wire.id,
            version: wire.version,
            agents: wire.agents,
            capabilities: wire.capabilities,
            requirements: wire.requirements,
            conflicts: wire.conflicts,
            defaults: wire.defaults,
            config_schema: wire.config_schema,
            compatibility: wire.compatibility,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}
