//! Finite bundle/catalog resolution and credential-free exact locks.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_runtime::{
    AgentId, AgentInvoker, ArtifactStore, BudgetLedger, BundleId, CapabilityId, ComponentId,
    ComponentRef, Digest, PortObject, RawJson, SchemaRef, Version,
};
use serde::{Deserialize, Serialize, de};
use thiserror::Error;

use crate::{
    AgentComponentSelection, AgentConstructionContext, AgentSpec, CapabilityActivation,
    CapabilitySpec, ComponentKind, ComponentSelector, Registry, ResolveRequest, ResolvedAgent,
};

/// Current strict bundle and lock schema version.
pub const BUNDLE_SCHEMA_VERSION: u16 = 1;
/// Stable code for malformed bundle/lock input.
pub const BUNDLE_RESOLUTION_INVALID: &str = "bundle_resolution_invalid";
/// Stable code for a missing bundle, agent, capability, component, or service.
pub const BUNDLE_RESOLUTION_MISSING: &str = "bundle_resolution_missing";
/// Stable code for a finite requirement or declared conflict.
pub const BUNDLE_RESOLUTION_CONFLICT: &str = "bundle_resolution_conflict";
/// Stable code for exact lock reconstruction mismatch.
pub const BUNDLE_RESOLUTION_LOCK_MISMATCH: &str = "bundle_resolution_lock_mismatch";
const MAX_ITEMS: usize = 4_096;
const MAX_CONFIG_ENTRIES: usize = 256;

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
    fn matches(&self, version: Version) -> bool {
        match self {
            Self::Exact { version: expected } => version == *expected,
            Self::CompatibleMajor { major } => version.major == *major,
            Self::Range {
                min_inclusive,
                max_exclusive,
            } => {
                version_tuple(version) >= version_tuple(*min_inclusive)
                    && version_tuple(version) < version_tuple(*max_exclusive)
            }
        }
    }

    fn validate(&self) -> bool {
        match self {
            Self::Range {
                min_inclusive,
                max_exclusive,
            } => version_tuple(*min_inclusive) < version_tuple(*max_exclusive),
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
                code: BUNDLE_RESOLUTION_INVALID,
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
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from(format!("{field}_too_many_items")),
                });
            }
        }
        if self.defaults.extension_config.len() > MAX_CONFIG_ENTRIES {
            return Err(BundleResolutionError::Invalid {
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from("bundle_defaults_too_many_entries"),
            });
        }
        let mut agents = BTreeSet::new();
        for agent in self.agents.iter() {
            agent
                .validate()
                .map_err(|error| BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from(error.to_string()),
                })?;
            if !agents.insert(agent.id.clone()) {
                return Err(BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from("duplicate_agent_id"),
                });
            }
        }
        let mut capabilities = BTreeSet::new();
        for capability in self.capabilities.iter() {
            capability
                .validate()
                .map_err(|error| BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from(error.to_string()),
                })?;
            if !capabilities.insert(capability.id.clone()) {
                return Err(BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
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
                        code: BUNDLE_RESOLUTION_INVALID,
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
                        code: BUNDLE_RESOLUTION_INVALID,
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
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from("unsupported_lock_schema_version"),
            });
        }
        let mut components = BTreeSet::new();
        for component in self.components.iter() {
            if component.component.version().is_none()
                || !components.insert(component.component.id().clone())
            {
                return Err(BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
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
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from("duplicate_locked_capability"),
            });
        }
        if self
            .schema_digests
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(BundleResolutionError::Invalid {
                code: BUNDLE_RESOLUTION_INVALID,
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
            code: BUNDLE_RESOLUTION_INVALID,
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
                code: BUNDLE_RESOLUTION_INVALID,
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
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from(error.to_string()),
            }
        })
    }
}

/// Direct optional non-primary service handles validated at construction.
#[derive(Clone, Default)]
pub struct RuntimeServices {
    /// Child-agent invocation service.
    pub agent_invoker: Option<Arc<dyn AgentInvoker>>,
    /// Shared-budget ledger service.
    pub budget_ledger: Option<Arc<dyn BudgetLedger>>,
    /// Scoped artifact service.
    pub artifact_store: Option<Arc<dyn ArtifactStore>>,
}

impl RuntimeServices {
    fn validate(&self, required: RequiredServices) -> Result<(), BundleResolutionError> {
        for (is_required, is_present, name) in [
            (
                required.agent_invoker,
                self.agent_invoker.is_some(),
                "agent_invoker",
            ),
            (
                required.budget_ledger,
                self.budget_ledger.is_some(),
                "budget_ledger",
            ),
            (
                required.artifact_store,
                self.artifact_store.is_some(),
                "artifact_store",
            ),
        ] {
            if is_required && !is_present {
                return Err(BundleResolutionError::Missing {
                    code: BUNDLE_RESOLUTION_MISSING,
                    item: Arc::from(name),
                });
            }
        }
        Ok(())
    }
}

/// Read-only exact installed bundle catalog.
pub trait AgentCatalog: PortObject {
    /// Look up one exact installed bundle identity.
    fn bundle(&self, id: &BundleId) -> Option<Arc<BundleSpec>>;
}

/// In-memory exact-installation catalog; one bundle version per ID.
#[derive(Default)]
pub struct BundleCatalog {
    bundles: BTreeMap<BundleId, Arc<BundleSpec>>,
}

impl BundleCatalog {
    /// Install one validated exact bundle.
    ///
    /// # Errors
    ///
    /// Rejects malformed bundles and duplicate bundle IDs rather than solving versions.
    pub fn install(&mut self, bundle: BundleSpec) -> Result<(), BundleResolutionError> {
        bundle.validate()?;
        if self.bundles.contains_key(&bundle.id) {
            return Err(BundleResolutionError::Conflict {
                code: BUNDLE_RESOLUTION_CONFLICT,
                item: Arc::from(bundle.id.as_str()),
            });
        }
        self.bundles.insert(bundle.id.clone(), Arc::new(bundle));
        Ok(())
    }
}

impl AgentCatalog for BundleCatalog {
    fn bundle(&self, id: &BundleId) -> Option<Arc<BundleSpec>> {
        self.bundles.get(id).cloned()
    }
}

/// Immutable recipe retained solely for explicit application activation rebuilds.
pub(crate) struct CompositionRecipe {
    pub bundle: Arc<BundleSpec>,
    pub base_spec: Arc<AgentSpec>,
    pub application_config: BTreeMap<ComponentId, RawJson>,
    pub capabilities: BTreeMap<CapabilityId, (LockedBundle, Arc<CapabilitySpec>)>,
    pub active_application: BTreeSet<CapabilityId>,
    pub services: RuntimeServices,
}

/// Finite resolver from exact bundle/catalog registrations to one immutable agent.
pub struct BundleResolver<'a> {
    catalog: &'a dyn AgentCatalog,
    engine_version: Version,
    host_features: BTreeSet<HostFeature>,
    services: RuntimeServices,
}

impl<'a> BundleResolver<'a> {
    /// Construct a resolver over one exact installed catalog.
    #[must_use]
    pub fn new(
        catalog: &'a dyn AgentCatalog,
        engine_version: Version,
        host_features: BTreeSet<HostFeature>,
        services: RuntimeServices,
    ) -> Self {
        Self {
            catalog,
            engine_version,
            host_features,
            services,
        }
    }

    /// Resolve one bundle-owned agent with no deferred application capability active.
    ///
    /// # Errors
    ///
    /// Fails before run acceptance on every missing, conflicting, incompatible,
    /// secret-bearing, or malformed selection.
    pub async fn resolve_agent(
        &self,
        registry: &mut Registry,
        bundle_id: &BundleId,
        agent_id: &AgentId,
        application_config: BTreeMap<ComponentId, RawJson>,
        context: AgentConstructionContext,
    ) -> Result<ResolvedAgent, BundleResolutionError> {
        let bundle =
            self.catalog
                .bundle(bundle_id)
                .ok_or_else(|| BundleResolutionError::Missing {
                    code: BUNDLE_RESOLUTION_MISSING,
                    item: Arc::from(bundle_id.as_str()),
                })?;
        bundle.validate()?;
        self.validate_environment(registry, &bundle)?;
        let base_spec = Arc::new(
            bundle
                .agents
                .iter()
                .find(|agent| &agent.id == agent_id)
                .cloned()
                .ok_or_else(|| BundleResolutionError::Missing {
                    code: BUNDLE_RESOLUTION_MISSING,
                    item: Arc::from(agent_id.as_str()),
                })?,
        );
        let capabilities = self.resolve_capabilities(&bundle, &base_spec)?;
        let recipe = Arc::new(CompositionRecipe {
            bundle,
            base_spec,
            application_config,
            capabilities,
            active_application: BTreeSet::new(),
            services: self.services.clone(),
        });
        self.resolve_recipe(registry, recipe, context).await
    }

    /// Rebuild an immutable plan with explicit application capability IDs active.
    ///
    /// The existing plan is never mutated or spliced. Middleware validation runs
    /// again and the lock fingerprint changes whenever active membership changes.
    ///
    /// # Errors
    ///
    /// Rejects unknown, disabled, model-reserved, or conflicting activation.
    pub async fn activate_application(
        &self,
        registry: &mut Registry,
        current: &ResolvedAgent,
        capabilities: impl IntoIterator<Item = CapabilityId>,
        context: AgentConstructionContext,
    ) -> Result<ResolvedAgent, BundleResolutionError> {
        let current_recipe =
            current
                .composition()
                .ok_or_else(|| BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from("agent_was_not_bundle_resolved"),
                })?;
        let mut active = current_recipe.active_application.clone();
        for capability_id in capabilities {
            let (_, capability) =
                current_recipe
                    .capabilities
                    .get(&capability_id)
                    .ok_or_else(|| BundleResolutionError::Missing {
                        code: BUNDLE_RESOLUTION_MISSING,
                        item: Arc::from(capability_id.as_str()),
                    })?;
            if capability.activation != CapabilityActivation::Application {
                return Err(BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from("capability_not_application_activated"),
                });
            }
            active.insert(capability_id);
        }
        let recipe = Arc::new(CompositionRecipe {
            bundle: Arc::clone(&current_recipe.bundle),
            base_spec: Arc::clone(&current_recipe.base_spec),
            application_config: current_recipe.application_config.clone(),
            capabilities: current_recipe.capabilities.clone(),
            active_application: active,
            services: current_recipe.services.clone(),
        });
        self.resolve_recipe(registry, recipe, context).await
    }

    /// Reconstruct one imported lock and require exact equality/fingerprint.
    ///
    /// # Errors
    ///
    /// Fails closed when any exact component/schema/config/capability selection differs.
    pub async fn resolve_lock(
        &self,
        registry: &mut Registry,
        lock: &ResolvedAgentLock,
        application_config: BTreeMap<ComponentId, RawJson>,
        context: AgentConstructionContext,
    ) -> Result<ResolvedAgent, BundleResolutionError> {
        lock.validate()?;
        if lock.engine_version != self.engine_version {
            return Err(BundleResolutionError::LockMismatch {
                code: BUNDLE_RESOLUTION_LOCK_MISMATCH,
                message: Arc::from("engine_version_mismatch"),
            });
        }
        let locked_bundle =
            lock.bundle
                .as_ref()
                .ok_or_else(|| BundleResolutionError::LockMismatch {
                    code: BUNDLE_RESOLUTION_LOCK_MISMATCH,
                    message: Arc::from("lock_missing_bundle"),
                })?;
        let bundle = self.catalog.bundle(&locked_bundle.id).ok_or_else(|| {
            BundleResolutionError::Missing {
                code: BUNDLE_RESOLUTION_MISSING,
                item: Arc::from(locked_bundle.id.as_str()),
            }
        })?;
        if bundle.version != locked_bundle.version {
            return Err(BundleResolutionError::LockMismatch {
                code: BUNDLE_RESOLUTION_LOCK_MISMATCH,
                message: Arc::from("bundle_version_mismatch"),
            });
        }
        let base_spec = bundle
            .agents
            .iter()
            .find(|agent| agent.fingerprint().ok() == Some(lock.agent_spec_digest))
            .ok_or_else(|| BundleResolutionError::LockMismatch {
                code: BUNDLE_RESOLUTION_LOCK_MISMATCH,
                message: Arc::from("agent_spec_digest_missing"),
            })?;
        let resolved = self
            .resolve_agent(
                registry,
                &locked_bundle.id,
                &base_spec.id,
                application_config,
                context.clone(),
            )
            .await?;
        let active = lock
            .capabilities
            .iter()
            .filter(|capability| {
                capability.active && capability.activation == CapabilityActivation::Application
            })
            .map(|capability| capability.id.clone())
            .collect::<Vec<_>>();
        let resolved = if active.is_empty() {
            resolved
        } else {
            self.activate_application(registry, &resolved, active, context)
                .await?
        };
        let reconstructed = resolved
            .lock()
            .ok_or_else(|| BundleResolutionError::LockMismatch {
                code: BUNDLE_RESOLUTION_LOCK_MISMATCH,
                message: Arc::from("reconstruction_missing_lock"),
            })?;
        if reconstructed.as_ref() != lock || reconstructed.fingerprint()? != lock.fingerprint()? {
            return Err(BundleResolutionError::LockMismatch {
                code: BUNDLE_RESOLUTION_LOCK_MISMATCH,
                message: Arc::from("exact_lock_reconstruction_mismatch"),
            });
        }
        Ok(resolved)
    }

    fn validate_environment(
        &self,
        registry: &Registry,
        bundle: &BundleSpec,
    ) -> Result<(), BundleResolutionError> {
        if bundle
            .compatibility
            .minimum_engine_version
            .is_some_and(|minimum| version_tuple(self.engine_version) < version_tuple(minimum))
        {
            return Err(BundleResolutionError::Conflict {
                code: BUNDLE_RESOLUTION_CONFLICT,
                item: Arc::from("minimum_engine_version"),
            });
        }
        for feature in &bundle.compatibility.required_host_features {
            self.require_feature(feature)?;
        }
        for requirement in bundle.requirements.iter() {
            match requirement {
                BundleRequirement::RequiredComponent { component, version } => {
                    let descriptor = registry.registered_component(component).ok_or_else(|| {
                        BundleResolutionError::Missing {
                            code: BUNDLE_RESOLUTION_MISSING,
                            item: Arc::from(component.as_str()),
                        }
                    })?;
                    require_component_version(descriptor.component.version(), version, component)?;
                }
                BundleRequirement::OptionalComponent { component, version } => {
                    if let Some(descriptor) = registry.registered_component(component) {
                        require_component_version(
                            descriptor.component.version(),
                            version,
                            component,
                        )?;
                    }
                }
                BundleRequirement::RequiredCapability { capability } => {
                    if !bundle
                        .capabilities
                        .iter()
                        .any(|candidate| &candidate.id == capability)
                    {
                        return Err(BundleResolutionError::Missing {
                            code: BUNDLE_RESOLUTION_MISSING,
                            item: Arc::from(capability.as_str()),
                        });
                    }
                }
                BundleRequirement::OneOfComponents { alternatives } => {
                    let matches = alternatives
                        .iter()
                        .filter(|component| registry.registered_component(component).is_some())
                        .count();
                    if matches != 1 {
                        return Err(BundleResolutionError::Conflict {
                            code: BUNDLE_RESOLUTION_CONFLICT,
                            item: Arc::from("one_of_components_not_exactly_one"),
                        });
                    }
                }
                BundleRequirement::RequiredHostFeature { feature } => {
                    self.require_feature(feature)?;
                }
                BundleRequirement::MinimumFrameworkContract { version }
                    if version_tuple(self.engine_version) < version_tuple(*version) =>
                {
                    return Err(BundleResolutionError::Conflict {
                        code: BUNDLE_RESOLUTION_CONFLICT,
                        item: Arc::from("minimum_framework_contract"),
                    });
                }
                BundleRequirement::MinimumFrameworkContract { .. } => {}
            }
        }
        for conflict in bundle.conflicts.iter() {
            let present = match conflict {
                BundleConflict::Component { component } => {
                    registry.registered_component(component).is_some()
                }
                BundleConflict::Capability { capability } => bundle
                    .capabilities
                    .iter()
                    .any(|candidate| &candidate.id == capability),
                BundleConflict::HostFeature { feature } => self.host_features.contains(feature),
            };
            if present {
                return Err(BundleResolutionError::Conflict {
                    code: BUNDLE_RESOLUTION_CONFLICT,
                    item: Arc::from("declared_bundle_conflict"),
                });
            }
        }
        let required = required_services(bundle);
        self.services.validate(required)
    }

    fn require_feature(&self, feature: &HostFeature) -> Result<(), BundleResolutionError> {
        if self.host_features.contains(feature) {
            Ok(())
        } else {
            Err(BundleResolutionError::Missing {
                code: BUNDLE_RESOLUTION_MISSING,
                item: Arc::from(host_feature_name(feature)),
            })
        }
    }

    fn resolve_capabilities(
        &self,
        bundle: &Arc<BundleSpec>,
        spec: &AgentSpec,
    ) -> Result<BTreeMap<CapabilityId, (LockedBundle, Arc<CapabilitySpec>)>, BundleResolutionError>
    {
        let mut resolved = BTreeMap::new();
        for reference in spec.capabilities.iter() {
            let source_bundle = match reference.bundle.as_ref() {
                None => Arc::clone(bundle),
                Some(id) if id == &bundle.id => Arc::clone(bundle),
                Some(id) => {
                    self.catalog
                        .bundle(id)
                        .ok_or_else(|| BundleResolutionError::Missing {
                            code: BUNDLE_RESOLUTION_MISSING,
                            item: Arc::from(id.as_str()),
                        })?
                }
            };
            source_bundle.validate()?;
            let capability = Arc::new(
                source_bundle
                    .capabilities
                    .iter()
                    .find(|candidate| candidate.id == reference.id)
                    .cloned()
                    .ok_or_else(|| BundleResolutionError::Missing {
                        code: BUNDLE_RESOLUTION_MISSING,
                        item: Arc::from(reference.id.as_str()),
                    })?,
            );
            if capability.activation == CapabilityActivation::Model {
                return Err(BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from("model_capability_activation_is_reserved"),
                });
            }
            if resolved
                .insert(
                    reference.id.clone(),
                    (
                        LockedBundle {
                            id: source_bundle.id.clone(),
                            version: source_bundle.version,
                        },
                        capability,
                    ),
                )
                .is_some()
            {
                return Err(BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from("duplicate_capability_resolution"),
                });
            }
        }
        Ok(resolved)
    }

    async fn resolve_recipe(
        &self,
        registry: &mut Registry,
        recipe: Arc<CompositionRecipe>,
        context: AgentConstructionContext,
    ) -> Result<ResolvedAgent, BundleResolutionError> {
        self.validate_environment(registry, &recipe.bundle)?;
        let effective_spec = Arc::new(expand_spec(&recipe)?);
        let effective_config = effective_config(&recipe)?;
        ensure_secret_free_config(&effective_config)?;
        let source = ComponentId::parse(recipe.base_spec.id.as_str()).map_err(|error| {
            BundleResolutionError::Invalid {
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from(error.to_string()),
            }
        })?;
        let mut request = ResolveRequest::new(source, selection(&effective_spec)?);
        for (component, config) in &effective_config {
            request = request
                .with_configuration(component.clone(), config.clone())
                .map_err(|error| BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from(error.to_string()),
                })?;
        }
        let resolved = registry.resolve(request, context).await.map_err(|error| {
            BundleResolutionError::Invalid {
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from(error.to_string()),
            }
        })?;
        let lock = Arc::new(build_lock(
            self.engine_version,
            &recipe,
            &resolved,
            &effective_config,
        )?);
        lock.validate()?;
        Ok(resolved.attach_composition(Arc::clone(&recipe.base_spec), lock, recipe))
    }
}

fn expand_spec(recipe: &CompositionRecipe) -> Result<AgentSpec, BundleResolutionError> {
    let mut spec = (*recipe.base_spec).clone();
    let mut instructions = spec.instructions.to_vec();
    let mut toolsets = spec.toolsets.to_vec();
    let mut context = spec.context_providers.to_vec();
    let mut middleware = spec.middleware.to_vec();
    for (id, (_, capability)) in &recipe.capabilities {
        let active = capability.activation == CapabilityActivation::Always
            || recipe.active_application.contains(id);
        if active {
            instructions.extend(capability.instructions.iter().cloned());
            toolsets.extend(capability.toolsets.iter().cloned());
            context.extend(capability.context_providers.iter().cloned());
            for component in capability.middleware.iter().cloned() {
                middleware.push(
                    finstack_ai_runtime::MiddlewareRef::try_new(component, None::<&str>).map_err(
                        |error| BundleResolutionError::Invalid {
                            code: BUNDLE_RESOLUTION_INVALID,
                            message: Arc::from(error.to_string()),
                        },
                    )?,
                );
            }
        }
    }
    spec.instructions = instructions.into();
    spec.toolsets = toolsets.into();
    spec.context_providers = context.into();
    spec.middleware = middleware.into();
    spec.validate()
        .map_err(|error| BundleResolutionError::Invalid {
            code: BUNDLE_RESOLUTION_INVALID,
            message: Arc::from(error.to_string()),
        })?;
    Ok(spec)
}

fn effective_config(
    recipe: &CompositionRecipe,
) -> Result<BTreeMap<ComponentId, RawJson>, BundleResolutionError> {
    let mut effective = recipe.bundle.defaults.extension_config.clone();
    for (component, value) in &recipe.application_config {
        effective.insert(component.clone(), value.clone());
    }
    for (component, value) in &recipe.base_spec.extension_config {
        effective.insert(component.clone(), value.clone());
    }
    if effective.len() > MAX_CONFIG_ENTRIES {
        return Err(BundleResolutionError::Invalid {
            code: BUNDLE_RESOLUTION_INVALID,
            message: Arc::from("effective_config_too_many_entries"),
        });
    }
    Ok(effective)
}

fn selection(spec: &AgentSpec) -> Result<AgentComponentSelection, BundleResolutionError> {
    let store = spec
        .store
        .clone()
        .ok_or_else(|| BundleResolutionError::Missing {
            code: BUNDLE_RESOLUTION_MISSING,
            item: Arc::from("agent journal store"),
        })?;
    Ok(AgentComponentSelection {
        model: ComponentSelector::Component(spec.model.clone()),
        toolsets: spec
            .toolsets
            .iter()
            .cloned()
            .map(ComponentSelector::Component)
            .collect::<Vec<_>>()
            .into(),
        context_providers: spec
            .context_providers
            .iter()
            .cloned()
            .map(ComponentSelector::Component)
            .collect::<Vec<_>>()
            .into(),
        middleware: spec
            .middleware
            .iter()
            .map(|reference| ComponentSelector::Component(reference.component().clone()))
            .collect::<Vec<_>>()
            .into(),
        store: ComponentSelector::Component(store),
        observers: spec
            .observers
            .iter()
            .cloned()
            .map(ComponentSelector::Component)
            .collect::<Vec<_>>()
            .into(),
    })
}

fn build_lock(
    engine_version: Version,
    recipe: &CompositionRecipe,
    resolved: &ResolvedAgent,
    effective_config: &BTreeMap<ComponentId, RawJson>,
) -> Result<ResolvedAgentLock, BundleResolutionError> {
    let run_plan = resolved.run_plan();
    let mut components = Vec::new();
    let mut push = |component: &crate::RegisteredComponentDescriptor| {
        components.push(LockedComponent {
            component: component.component.clone(),
            kind: component.kind.into(),
            config_digest: effective_config
                .get(component.component.id())
                .map(RawJson::digest),
            configuration_schema: component.configuration_schema.clone(),
        });
    };
    push(run_plan.model().descriptor());
    for component in run_plan.toolsets() {
        push(component.descriptor());
    }
    for component in run_plan.context_providers() {
        push(component.descriptor());
    }
    for component in run_plan.middleware() {
        push(component.descriptor());
    }
    push(run_plan.store().descriptor());
    for component in run_plan.observers() {
        push(component.descriptor());
    }
    let capabilities = recipe
        .capabilities
        .iter()
        .map(|(id, (bundle, capability))| {
            let bytes = serde_json_canonicalizer::to_vec(capability.as_ref()).map_err(|error| {
                BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from(error.to_string()),
                }
            })?;
            let definition_digest = Digest::domain_separated("capability-spec", 1, &bytes)
                .map_err(|error| BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from(error.to_string()),
                })?;
            Ok(LockedCapability {
                id: id.clone(),
                bundle: bundle.clone(),
                activation: capability.activation,
                active: capability.activation == CapabilityActivation::Always
                    || recipe.active_application.contains(id),
                definition_digest,
            })
        })
        .collect::<Result<Vec<_>, BundleResolutionError>>()?;
    let config_bytes = serde_json_canonicalizer::to_vec(effective_config).map_err(|error| {
        BundleResolutionError::Invalid {
            code: BUNDLE_RESOLUTION_INVALID,
            message: Arc::from(error.to_string()),
        }
    })?;
    let effective_config_digest = Digest::domain_separated("effective-config", 1, &config_bytes)
        .map_err(|error| BundleResolutionError::Invalid {
            code: BUNDLE_RESOLUTION_INVALID,
            message: Arc::from(error.to_string()),
        })?;
    let mut schema_digests = components
        .iter()
        .filter_map(|component| {
            component
                .configuration_schema
                .as_ref()
                .map(|schema| schema.schema_digest)
        })
        .chain(
            recipe
                .bundle
                .config_schema
                .as_ref()
                .map(|schema| schema.schema_digest),
        )
        .collect::<Vec<_>>();
    schema_digests.sort_unstable();
    schema_digests.dedup();
    Ok(ResolvedAgentLock {
        schema_version: BUNDLE_SCHEMA_VERSION,
        engine_version,
        agent_spec_digest: recipe.base_spec.fingerprint().map_err(|error| {
            BundleResolutionError::Invalid {
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from(error.to_string()),
            }
        })?,
        bundle: Some(LockedBundle {
            id: recipe.bundle.id.clone(),
            version: recipe.bundle.version,
        }),
        components: components.into(),
        capabilities: capabilities.into(),
        effective_config_digest,
        middleware_chain_digest: run_plan.middleware_chain().digest(),
        schema_digests: schema_digests.into(),
        required_services: required_services(&recipe.bundle),
    })
}

fn required_services(bundle: &BundleSpec) -> RequiredServices {
    let mut required = RequiredServices::default();
    let features = bundle
        .requirements
        .iter()
        .filter_map(|requirement| match requirement {
            BundleRequirement::RequiredHostFeature { feature } => Some(feature),
            _ => None,
        })
        .chain(bundle.compatibility.required_host_features.iter());
    for feature in features {
        match feature {
            HostFeature::AgentInvoker => required.agent_invoker = true,
            HostFeature::BudgetLedger => required.budget_ledger = true,
            HostFeature::ArtifactStore => required.artifact_store = true,
            HostFeature::Custom { .. } => {}
        }
    }
    required
}

fn require_component_version(
    registered: Option<Version>,
    requirement: &VersionRequirement,
    component: &ComponentId,
) -> Result<(), BundleResolutionError> {
    if registered.is_some_and(|version| requirement.matches(version)) {
        Ok(())
    } else {
        Err(BundleResolutionError::Conflict {
            code: BUNDLE_RESOLUTION_CONFLICT,
            item: Arc::from(component.as_str()),
        })
    }
}

fn ensure_secret_free_config(
    config: &BTreeMap<ComponentId, RawJson>,
) -> Result<(), BundleResolutionError> {
    for value in config.values() {
        let parsed: serde_json::Value =
            serde_json::from_slice(value.as_bytes()).map_err(|error| {
                BundleResolutionError::Invalid {
                    code: BUNDLE_RESOLUTION_INVALID,
                    message: Arc::from(error.to_string()),
                }
            })?;
        if contains_secret(&parsed) {
            return Err(BundleResolutionError::Invalid {
                code: BUNDLE_RESOLUTION_INVALID,
                message: Arc::from("secret_material_in_configuration"),
            });
        }
    }
    Ok(())
}

fn contains_secret(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            let suspicious = [
                "password",
                "token",
                "credential",
                "api_key",
                "access_key",
                "private_key",
                "bearer",
                "secret",
            ]
            .iter()
            .any(|needle| normalized.contains(needle));
            (suspicious && !normalized.ends_with("_ref")) || contains_secret(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_secret),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}

fn version_tuple(version: Version) -> (u16, u16, u16) {
    (version.major, version.minor, version.patch)
}

fn host_feature_name(feature: &HostFeature) -> &str {
    match feature {
        HostFeature::AgentInvoker => "agent_invoker",
        HostFeature::BudgetLedger => "budget_ledger",
        HostFeature::ArtifactStore => "artifact_store",
        HostFeature::Custom { id } => id.as_str(),
    }
}

/// Finite bundle/catalog/lock resolution failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BundleResolutionError {
    /// Malformed or unsupported specification/lock.
    #[error("{code}: {message}")]
    Invalid {
        /// Stable machine-readable code.
        code: &'static str,
        /// Stable non-secret diagnostic.
        message: Arc<str>,
    },
    /// Required exact identity or service is absent.
    #[error("{code}: missing {item}")]
    Missing {
        /// Stable machine-readable code.
        code: &'static str,
        /// Missing non-secret identity.
        item: Arc<str>,
    },
    /// Finite requirement/conflict failed.
    #[error("{code}: conflicting {item}")]
    Conflict {
        /// Stable machine-readable code.
        code: &'static str,
        /// Conflicting non-secret identity.
        item: Arc<str>,
    },
    /// Imported lock could not be reconstructed exactly.
    #[error("{code}: {message}")]
    LockMismatch {
        /// Stable machine-readable code.
        code: &'static str,
        /// Stable non-secret diagnostic.
        message: Arc<str>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_bundle_and_lock_round_trip_reject_unknown_fields() {
        let lock = ResolvedAgentLock {
            schema_version: BUNDLE_SCHEMA_VERSION,
            engine_version: Version {
                major: 0,
                minor: 0,
                patch: 1,
            },
            agent_spec_digest: Digest::raw_json(b"agent"),
            bundle: Some(LockedBundle {
                id: BundleId::parse("finstack.bundle.test").expect("bundle"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
            }),
            components: Arc::from([LockedComponent {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.model.test").expect("component"),
                    Some(Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    }),
                ),
                kind: LockedComponentKind::Model,
                config_digest: None,
                configuration_schema: None,
            }]),
            capabilities: Arc::from([]),
            effective_config_digest: Digest::raw_json(b"config"),
            middleware_chain_digest: Digest::raw_json(b"middleware"),
            schema_digests: Arc::from([]),
            required_services: RequiredServices::default(),
        };
        let bytes = lock.to_json().expect("lock JSON");
        let decoded = ResolvedAgentLock::from_json(&bytes).expect("lock round trip");
        assert_eq!(decoded, lock);
        assert_eq!(decoded.fingerprint(), lock.fingerprint());

        let mut unknown: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        unknown["credential"] = serde_json::json!("secret-canary");
        assert!(
            ResolvedAgentLock::from_json(&serde_json::to_vec(&unknown).expect("JSON")).is_err()
        );
    }

    #[test]
    fn configuration_secret_canary_fails_closed_but_refs_are_allowed() {
        let component = ComponentId::parse("finstack.model.test").expect("component");
        let bad = BTreeMap::from([(
            component.clone(),
            RawJson::parse(br#"{"api_key":"secret-canary"}"#).expect("JSON"),
        )]);
        assert!(ensure_secret_free_config(&bad).is_err());
        let good = BTreeMap::from([(
            component,
            RawJson::parse(br#"{"api_key_ref":"vault://model"}"#).expect("JSON"),
        )]);
        ensure_secret_free_config(&good).expect("secret reference");
    }

    #[test]
    fn version_ranges_and_required_services_are_finite() {
        let requirement = VersionRequirement::Range {
            min_inclusive: Version {
                major: 1,
                minor: 2,
                patch: 0,
            },
            max_exclusive: Version {
                major: 2,
                minor: 0,
                patch: 0,
            },
        };
        assert!(requirement.matches(Version {
            major: 1,
            minor: 9,
            patch: 0,
        }));
        assert!(!requirement.matches(Version {
            major: 2,
            minor: 0,
            patch: 0,
        }));
        let services = RuntimeServices::default();
        assert!(
            services
                .validate(RequiredServices {
                    budget_ledger: true,
                    ..RequiredServices::default()
                })
                .is_err()
        );
        services
            .validate(RequiredServices::default())
            .expect("minimal agent needs no services");
    }
}
