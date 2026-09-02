//! Finite resolver from exact bundle/catalog registrations to one immutable agent.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{AgentId, BundleId, CapabilityId, ComponentId, RawJson, Version};

use crate::{
    AgentConstructionContext, AgentSpec, CapabilityActivation, CapabilitySpec, Registry,
    ResolveRequest, ResolvedAgent,
};

use super::expand::{build_lock, effective_config, expand_spec, required_services, selection};
use super::secret::ensure_secret_free_config;
use super::{
    BundleCatalog, BundleConflict, BundleRequirement, BundleResolutionError, BundleSpec,
    CompositionRecipe, HostFeature, LockedBundle, RequiredServices, ResolvedAgentLock,
    RuntimeServices, VersionRequirement,
};

type ResolvedCapabilities = (
    BTreeMap<CapabilityId, (LockedBundle, Arc<CapabilitySpec>)>,
    RequiredServices,
);

/// Finite resolver from exact bundle/catalog registrations to one immutable agent.
pub struct BundleResolver<'a> {
    catalog: &'a BundleCatalog,
    engine_version: Version,
    host_features: BTreeSet<HostFeature>,
    services: RuntimeServices,
}

impl<'a> BundleResolver<'a> {
    /// Construct a resolver over one exact installed catalog.
    #[must_use]
    pub fn new(
        catalog: &'a BundleCatalog,
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
                    item: Arc::from(agent_id.as_str()),
                })?,
        );
        let (capabilities, required_services) =
            self.resolve_capabilities(registry, &bundle, &base_spec)?;
        let recipe = Arc::new(CompositionRecipe {
            bundle,
            base_spec,
            application_config,
            capabilities,
            active_application: BTreeSet::new(),
            active_model: BTreeSet::new(),
            services: self.services.clone(),
            required_services,
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
        self.activate(
            registry,
            current,
            capabilities,
            context,
            CapabilityActivation::Application,
            "capability_not_application_activated",
        )
        .await
    }

    /// Rebuild an immutable plan with bounded model-selected capability IDs active.
    ///
    /// The caller owns selection policy. This method only validates declared
    /// `Model` capabilities and reruns complete component and middleware
    /// resolution without mutating the current plan.
    ///
    /// # Errors
    ///
    /// Rejects unknown, disabled, application-only, or conflicting activation.
    pub async fn activate_model(
        &self,
        registry: &mut Registry,
        current: &ResolvedAgent,
        capabilities: impl IntoIterator<Item = CapabilityId>,
        context: AgentConstructionContext,
    ) -> Result<ResolvedAgent, BundleResolutionError> {
        self.activate(
            registry,
            current,
            capabilities,
            context,
            CapabilityActivation::Model,
            "capability_not_model_activated",
        )
        .await
    }

    async fn activate(
        &self,
        registry: &mut Registry,
        current: &ResolvedAgent,
        capabilities: impl IntoIterator<Item = CapabilityId>,
        context: AgentConstructionContext,
        expected: CapabilityActivation,
        invalid_kind: &'static str,
    ) -> Result<ResolvedAgent, BundleResolutionError> {
        let current_recipe =
            current
                .composition()
                .ok_or_else(|| BundleResolutionError::Invalid {
                    message: Arc::from("agent_was_not_bundle_resolved"),
                })?;
        let mut active_application = current_recipe.active_application.clone();
        let mut active_model = current_recipe.active_model.clone();
        let active = match expected {
            CapabilityActivation::Application => &mut active_application,
            CapabilityActivation::Model => &mut active_model,
            CapabilityActivation::Always | CapabilityActivation::Disabled => {
                return Err(BundleResolutionError::Invalid {
                    message: Arc::from(invalid_kind),
                });
            }
        };
        for capability_id in capabilities {
            let (_, capability) =
                current_recipe
                    .capabilities
                    .get(&capability_id)
                    .ok_or_else(|| BundleResolutionError::Missing {
                        item: Arc::from(capability_id.as_str()),
                    })?;
            if capability.activation != expected {
                return Err(BundleResolutionError::Invalid {
                    message: Arc::from(invalid_kind),
                });
            }
            active.insert(capability_id);
        }
        let recipe = Arc::new(CompositionRecipe {
            bundle: Arc::clone(&current_recipe.bundle),
            base_spec: Arc::clone(&current_recipe.base_spec),
            application_config: current_recipe.application_config.clone(),
            capabilities: current_recipe.capabilities.clone(),
            active_application,
            active_model,
            services: current_recipe.services.clone(),
            required_services: current_recipe.required_services,
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
                message: Arc::from("engine_version_mismatch"),
            });
        }
        let locked_bundle =
            lock.bundle
                .as_ref()
                .ok_or_else(|| BundleResolutionError::LockMismatch {
                    message: Arc::from("lock_missing_bundle"),
                })?;
        let bundle = self.catalog.bundle(&locked_bundle.id).ok_or_else(|| {
            BundleResolutionError::Missing {
                item: Arc::from(locked_bundle.id.as_str()),
            }
        })?;
        if bundle.version != locked_bundle.version {
            return Err(BundleResolutionError::LockMismatch {
                message: Arc::from("bundle_version_mismatch"),
            });
        }
        let base_spec = bundle
            .agents
            .iter()
            .find(|agent| agent.fingerprint().ok() == Some(lock.agent_spec_digest))
            .ok_or_else(|| BundleResolutionError::LockMismatch {
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
        let active_ids = |activation: CapabilityActivation| {
            lock.capabilities
                .iter()
                .filter(|capability| capability.active && capability.activation == activation)
                .map(|capability| capability.id.clone())
                .collect::<Vec<_>>()
        };
        let active = active_ids(CapabilityActivation::Application);
        let resolved = if active.is_empty() {
            resolved
        } else {
            self.activate_application(registry, &resolved, active, context.clone())
                .await?
        };
        let active_model = active_ids(CapabilityActivation::Model);
        let resolved = if active_model.is_empty() {
            resolved
        } else {
            self.activate_model(registry, &resolved, active_model, context)
                .await?
        };
        let reconstructed = resolved
            .lock()
            .ok_or_else(|| BundleResolutionError::LockMismatch {
                message: Arc::from("reconstruction_missing_lock"),
            })?;
        if reconstructed.as_ref() != lock || reconstructed.fingerprint()? != lock.fingerprint()? {
            return Err(BundleResolutionError::LockMismatch {
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
            .is_some_and(|minimum| self.engine_version < minimum)
        {
            return Err(BundleResolutionError::Conflict {
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
                            item: Arc::from("one_of_components_not_exactly_one"),
                        });
                    }
                }
                BundleRequirement::RequiredHostFeature { feature } => {
                    self.require_feature(feature)?;
                }
                BundleRequirement::MinimumFrameworkContract { version }
                    if self.engine_version < *version =>
                {
                    return Err(BundleResolutionError::Conflict {
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
                item: Arc::from(host_feature_name(feature)),
            })
        }
    }

    fn resolve_capabilities(
        &self,
        registry: &Registry,
        bundle: &Arc<BundleSpec>,
        spec: &AgentSpec,
    ) -> Result<ResolvedCapabilities, BundleResolutionError> {
        let mut resolved = BTreeMap::new();
        let mut required = required_services(bundle);
        let mut validated_sources = BTreeSet::from([bundle.id.clone()]);
        for reference in spec.capabilities.iter() {
            let source_bundle = match reference.bundle.as_ref() {
                None => Arc::clone(bundle),
                Some(id) if id == &bundle.id => Arc::clone(bundle),
                Some(id) => {
                    self.catalog
                        .bundle(id)
                        .ok_or_else(|| BundleResolutionError::Missing {
                            item: Arc::from(id.as_str()),
                        })?
                }
            };
            source_bundle.validate()?;
            if validated_sources.insert(source_bundle.id.clone()) {
                self.validate_environment(registry, &source_bundle)?;
            }
            let source_required = required_services(&source_bundle);
            required.agent_invoker |= source_required.agent_invoker;
            required.budget_ledger |= source_required.budget_ledger;
            required.artifact_store |= source_required.artifact_store;
            let capability = Arc::new(
                source_bundle
                    .capabilities
                    .iter()
                    .find(|candidate| candidate.id == reference.id)
                    .cloned()
                    .ok_or_else(|| BundleResolutionError::Missing {
                        item: Arc::from(reference.id.as_str()),
                    })?,
            );
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
                    message: Arc::from("duplicate_capability_resolution"),
                });
            }
        }
        Ok((resolved, required))
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
                message: Arc::from(error.to_string()),
            }
        })?;
        let mut request = ResolveRequest::new(source, selection(&effective_spec)?);
        for (component, config) in &effective_config {
            request = request
                .with_configuration(component.clone(), config.clone())
                .map_err(|error| BundleResolutionError::Invalid {
                    message: Arc::from(error.to_string()),
                })?;
        }
        let resolved = registry.resolve(request, context).await.map_err(|error| {
            BundleResolutionError::Invalid {
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
        Ok(resolved.attach_composition(effective_spec, lock, recipe))
    }
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
            item: Arc::from(component.as_str()),
        })
    }
}

fn host_feature_name(feature: &HostFeature) -> &str {
    match feature {
        HostFeature::AgentInvoker => "agent_invoker",
        HostFeature::BudgetLedger => "budget_ledger",
        HostFeature::ArtifactStore => "artifact_store",
        HostFeature::Custom { id } => id.as_str(),
    }
}
