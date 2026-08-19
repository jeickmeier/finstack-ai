use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{
    AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleDefaults,
    BundleResolver, BundleSpec, CapabilityActivation, CapabilityRef, CapabilitySpec,
    CompatibilityRequirements, Extension, ExtensionDescriptor, InstructionSpec, ReadyComponent,
    Registrar, RegistrationError, RegistrationMetadata, Registry, RunPolicy, RuntimeServices,
};
use finstack_ai_kernel::{AgentId, BundleId, CapabilityId, ComponentRef, MiddlewareRef};
use finstack_ai_runtime::{ContextProvider, Middleware, Model, Observer, Toolset};

use super::PREVIEW_ENGINE_VERSION;
use super::handle::{Agent, ModelCapabilityVariant};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, CapabilityCatalogEntry,
    MAX_COMPACT_CATALOG_BYTES,
};

/// Ergonomic native composition builder over direct ready handles.
///
/// It creates the same strict `AgentSpec`, bundle lock, and no-lookup run plan
/// as the lower-level catalog/registry APIs. No credentials are serialized.
#[derive(Clone)]
pub struct NativeAgentBuilder {
    agent_id: AgentId,
    bundle_id: BundleId,
    model: (ComponentRef, Arc<dyn Model>),
    store: (ComponentRef, Arc<dyn finstack_ai_runtime::JournalStore>),
    toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn Observer>)>,
    installed_toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    installed_context_providers: Vec<(ComponentRef, Arc<dyn ContextProvider>)>,
    installed_middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    instructions: Vec<InstructionSpec>,
    capabilities: Vec<CapabilitySpec>,
    active_application: BTreeSet<CapabilityId>,
    policy: RunPolicy,
    activation_host: Option<Arc<super::activation::NativeCapabilityHost>>,
}

impl NativeAgentBuilder {
    pub(super) fn new(
        agent_id: AgentId,
        bundle_id: BundleId,
        model: (ComponentRef, Arc<dyn Model>),
        store: (ComponentRef, Arc<dyn finstack_ai_runtime::JournalStore>),
    ) -> Self {
        Self {
            agent_id,
            bundle_id,
            model,
            store,
            toolsets: Vec::new(),
            context_providers: Vec::new(),
            middleware: Vec::new(),
            observers: Vec::new(),
            installed_toolsets: Vec::new(),
            installed_context_providers: Vec::new(),
            installed_middleware: Vec::new(),
            instructions: Vec::new(),
            capabilities: Vec::new(),
            active_application: BTreeSet::new(),
            policy: RunPolicy::default(),
            activation_host: None,
        }
    }

    /// Add one ordered model instruction.
    ///
    /// # Arguments
    ///
    /// * `text` - Non-empty instruction prefix appended in registration order.
    ///
    /// # Errors
    ///
    /// Rejects empty, NUL-bearing, or oversized instruction text.
    pub fn try_instruction(mut self, text: impl Into<Arc<str>>) -> Result<Self, AgentRunError> {
        self.instructions
            .push(InstructionSpec::try_new(text).map_err(invalid_config)?);
        Ok(self)
    }

    /// Add one ordered direct Toolset handle.
    ///
    /// # Arguments
    ///
    /// * `component` - Exact-version component identity for the lock.
    /// * `toolset` - Ready in-process Toolset implementation.
    #[must_use]
    pub fn toolset(mut self, component: ComponentRef, toolset: Arc<dyn Toolset>) -> Self {
        self.toolsets.push((component, toolset));
        self
    }

    /// Add one ordered direct [`ContextProvider`] handle.
    ///
    /// # Arguments
    ///
    /// * `component` - Exact-version component identity for the lock.
    /// * `provider` - Ready in-process context provider.
    #[must_use]
    pub fn context_provider(
        mut self,
        component: ComponentRef,
        provider: Arc<dyn ContextProvider>,
    ) -> Self {
        self.context_providers.push((component, provider));
        self
    }

    /// Register a Toolset handle without adding it to the base spec.
    ///
    /// Use this for capability-contributed toolsets so they join the lock-time
    /// union without becoming base-agent members.
    #[must_use]
    pub fn capability_toolset(
        mut self,
        component: ComponentRef,
        toolset: Arc<dyn Toolset>,
    ) -> Self {
        self.installed_toolsets.push((component, toolset));
        self
    }

    /// Register a context provider handle without adding it to the base spec.
    #[must_use]
    pub fn capability_context_provider(
        mut self,
        component: ComponentRef,
        provider: Arc<dyn ContextProvider>,
    ) -> Self {
        self.installed_context_providers.push((component, provider));
        self
    }

    /// Register a Middleware handle without adding it to the base spec.
    #[must_use]
    pub fn capability_middleware(
        mut self,
        component: ComponentRef,
        middleware: Arc<dyn Middleware>,
    ) -> Self {
        self.installed_middleware.push((component, middleware));
        self
    }

    /// Attach the host used by `capability_activate` to submit complete sets.
    #[must_use]
    pub fn capability_activation_host(
        mut self,
        host: Arc<super::activation::NativeCapabilityHost>,
    ) -> Self {
        self.activation_host = Some(host);
        self
    }

    /// Add one ordered direct Middleware handle.
    ///
    /// # Arguments
    ///
    /// * `component` - Exact-version component identity for the lock.
    /// * `middleware` - Ready in-process middleware component.
    #[must_use]
    pub fn middleware(mut self, component: ComponentRef, middleware: Arc<dyn Middleware>) -> Self {
        self.middleware.push((component, middleware));
        self
    }

    /// Add one ordered direct [`Observer`] handle.
    ///
    /// # Arguments
    ///
    /// * `component` - Exact-version component identity for the lock.
    /// * `observer` - Ready read-only observer. Failures are isolated from run semantics.
    #[must_use]
    pub fn observer(mut self, component: ComponentRef, observer: Arc<dyn Observer>) -> Self {
        self.observers.push((component, observer));
        self
    }

    /// Add one validated declarative capability to the finite catalog.
    ///
    /// # Arguments
    ///
    /// * `capability` - Catalog entry. `model` activation is selected later via
    ///   [`AgentRunRequest::capability`], not by user-input overlap.
    #[must_use]
    pub fn capability(mut self, capability: CapabilitySpec) -> Self {
        self.capabilities.push(capability);
        self
    }

    /// Select one application capability for the initial immutable plan.
    ///
    /// # Arguments
    ///
    /// * `capability` - Catalog id whose activation is `application`.
    #[must_use]
    pub fn activate_application(mut self, capability: CapabilityId) -> Self {
        self.active_application.insert(capability);
        self
    }

    /// Replace run and child-invocation policy.
    ///
    /// Child runs default to [`crate::ChildRunPolicy::Deny`].
    ///
    /// # Arguments
    ///
    /// * `policy` - Composition policy frozen into the agent specification.
    #[must_use]
    pub fn policy(mut self, policy: RunPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Resolve, warm, lock, and construct one native [`Agent`].
    ///
    /// # Errors
    ///
    /// Fails closed on non-exact or duplicate components and all ordinary
    /// registry, bundle, warmup, and tool-catalog failures.
    pub async fn build(self) -> Result<Agent, AgentRunError> {
        let recompose = Arc::new(self.clone());
        validate_builder_components(&self)?;
        let mut registered_toolsets = self.toolsets.clone();
        registered_toolsets.extend(self.installed_toolsets.iter().cloned());
        let mut registered_providers = self.context_providers.clone();
        registered_providers.extend(self.installed_context_providers.iter().cloned());
        let mut registered_middleware = self.middleware.clone();
        registered_middleware.extend(self.installed_middleware.iter().cloned());
        let extension = NativeBuilderExtension {
            source: finstack_ai_kernel::ComponentId::parse("finstack.sdk.native-builder")
                .map_err(invalid_config)?,
            model: self.model.clone(),
            store: self.store.clone(),
            toolsets: registered_toolsets,
            context_providers: registered_providers,
            middleware: registered_middleware,
            observers: self.observers.clone(),
        };
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&extension)
            .map_err(invalid_config)?;
        let mut registry = registrar.into_registry();
        let spec = builder_spec(&self)?;
        let mut catalog = BundleCatalog::default();
        catalog
            .install(BundleSpec {
                schema_version: BUNDLE_SCHEMA_VERSION,
                id: self.bundle_id.clone(),
                version: PREVIEW_ENGINE_VERSION,
                agents: Arc::from([spec]),
                capabilities: self.capabilities.clone().into(),
                requirements: Arc::from([]),
                conflicts: Arc::from([]),
                defaults: BundleDefaults::default(),
                config_schema: None,
                compatibility: CompatibilityRequirements::default(),
            })
            .map_err(invalid_config)?;
        let bundle_resolver = BundleResolver::new(
            &catalog,
            PREVIEW_ENGINE_VERSION,
            std::collections::BTreeSet::new(),
            RuntimeServices::default(),
        );
        let composed_agent = bundle_resolver
            .resolve_agent(
                &mut registry,
                &self.bundle_id,
                &self.agent_id,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .map_err(invalid_config)?;
        let composed_agent = if self.active_application.is_empty() {
            composed_agent
        } else {
            bundle_resolver
                .activate_application(
                    &mut registry,
                    &composed_agent,
                    self.active_application,
                    AgentConstructionContext::new(),
                )
                .await
                .map_err(invalid_config)?
        };
        let mut agent = Agent::try_from_resolved(Arc::new(composed_agent))?;
        agent.recompose = Some(recompose);
        let contributions =
            super::mask::CapabilityContributionIndex::from_specs(&self.capabilities);
        agent.attach_capability_surface(
            Arc::from(self.capabilities.clone()),
            contributions.clone(),
            self.activation_host.clone(),
        )?;
        let variants =
            resolve_model_variants(&self.capabilities, &bundle_resolver, &mut registry, &agent)
                .await?;
        agent.model_capabilities = variants.into();
        for variant in Arc::make_mut(&mut agent.model_capabilities) {
            variant.agent.attach_capability_surface(
                Arc::from(self.capabilities.clone()),
                contributions.clone(),
                self.activation_host.clone(),
            )?;
        }
        Ok(agent)
    }

    pub(super) async fn rebuild_from_live_catalogs(mut self) -> Result<Agent, AgentRunError> {
        self.toolsets = reconstruct_toolsets(self.toolsets).await?;
        self.installed_toolsets = reconstruct_toolsets(self.installed_toolsets).await?;
        self.context_providers = reconstruct_providers(self.context_providers).await?;
        self.installed_context_providers =
            reconstruct_providers(self.installed_context_providers).await?;
        self.build().await
    }
}

async fn reconstruct_toolsets(
    entries: Vec<(ComponentRef, Arc<dyn Toolset>)>,
) -> Result<Vec<(ComponentRef, Arc<dyn Toolset>)>, AgentRunError> {
    let mut rebuilt = Vec::with_capacity(entries.len());
    for (component, toolset) in entries {
        let next = toolset.reconstruct().await.map_err(invalid_config)?;
        rebuilt.push((component, next.unwrap_or(toolset)));
    }
    Ok(rebuilt)
}

async fn reconstruct_providers(
    entries: Vec<(ComponentRef, Arc<dyn ContextProvider>)>,
) -> Result<Vec<(ComponentRef, Arc<dyn ContextProvider>)>, AgentRunError> {
    let mut rebuilt = Vec::with_capacity(entries.len());
    for (component, provider) in entries {
        let next = provider.reconstruct().await.map_err(invalid_config)?;
        rebuilt.push((component, next.unwrap_or(provider)));
    }
    Ok(rebuilt)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as Result::map_err so the error must be taken by value"
)]
fn invalid_config(error: impl ToString) -> AgentRunError {
    AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
}

fn builder_spec(builder: &NativeAgentBuilder) -> Result<crate::AgentSpec, AgentRunError> {
    validate_compact_catalog(&builder.capabilities)?;
    let capability_refs = builder
        .capabilities
        .iter()
        .map(|capability| CapabilityRef {
            id: capability.id.clone(),
            bundle: None,
        })
        .collect::<Vec<_>>();
    let middleware = builder
        .middleware
        .iter()
        .map(|(component, _)| {
            MiddlewareRef::try_new(component.clone(), None::<&str>).map_err(invalid_config)
        })
        .collect::<Result<Vec<_>, _>>()?;
    AgentBuilder::new(
        builder.agent_id.clone(),
        builder.model.0.clone(),
        builder.store.0.clone(),
    )
    .instructions(builder.instructions.clone())
    .toolsets(
        builder
            .toolsets
            .iter()
            .map(|(component, _)| component.clone())
            .collect::<Vec<_>>(),
    )
    .context_providers(
        builder
            .context_providers
            .iter()
            .map(|(component, _)| component.clone())
            .collect::<Vec<_>>(),
    )
    .middleware(middleware)
    .observers(
        builder
            .observers
            .iter()
            .map(|(component, _)| component.clone())
            .collect::<Vec<_>>(),
    )
    .capabilities(capability_refs)
    .policy(builder.policy.clone())
    .build()
    .map_err(invalid_config)
}

fn validate_builder_components(builder: &NativeAgentBuilder) -> Result<(), AgentRunError> {
    validate_exact_component(&builder.model.0)?;
    validate_exact_component(&builder.store.0)?;
    for (component, _) in &builder.toolsets {
        validate_exact_component(component)?;
    }
    for (component, _) in &builder.context_providers {
        validate_exact_component(component)?;
    }
    for (component, _) in &builder.middleware {
        validate_exact_component(component)?;
    }
    for (component, _) in &builder.observers {
        validate_exact_component(component)?;
    }
    for (component, _) in &builder.installed_toolsets {
        validate_exact_component(component)?;
    }
    for (component, _) in &builder.installed_context_providers {
        validate_exact_component(component)?;
    }
    for (component, _) in &builder.installed_middleware {
        validate_exact_component(component)?;
    }
    Ok(())
}

async fn resolve_model_variants(
    capabilities: &[CapabilitySpec],
    bundle_resolver: &BundleResolver<'_>,
    registry: &mut Registry,
    agent: &Agent,
) -> Result<Vec<ModelCapabilityVariant>, AgentRunError> {
    let mut variants = Vec::new();
    for capability in capabilities
        .iter()
        .filter(|capability| capability.activation == CapabilityActivation::Model)
    {
        let resolved = bundle_resolver
            .activate_model(
                registry,
                agent.resolved(),
                [capability.id.clone()],
                AgentConstructionContext::new(),
            )
            .await
            .map_err(invalid_config)?;
        variants.push(ModelCapabilityVariant {
            entry: CapabilityCatalogEntry {
                id: capability.id.clone(),
                description: Arc::clone(&capability.description),
            },
            agent: Agent::try_from_resolved(Arc::new(resolved))?,
        });
    }
    Ok(variants)
}

pub(super) fn validate_compact_catalog(
    capabilities: &[CapabilitySpec],
) -> Result<(), AgentRunError> {
    let mut ids = BTreeSet::new();
    let mut bytes = 0usize;
    for capability in capabilities {
        capability.validate().map_err(invalid_config)?;
        if !ids.insert(capability.id.clone()) {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                format!("duplicate capability {}", capability.id),
            ));
        }
        if capability.activation == CapabilityActivation::Model {
            bytes = bytes
                .checked_add(capability.id.as_str().len())
                .and_then(|value| value.checked_add(capability.description.len()))
                .and_then(|value| value.checked_add(3))
                .ok_or_else(|| {
                    AgentRunError::configuration(
                        AGENT_RUN_INVALID_CONFIGURATION,
                        "compact capability catalog size overflow",
                    )
                })?;
        }
    }
    if bytes > MAX_COMPACT_CATALOG_BYTES {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            format!("compact capability catalog exceeds {MAX_COMPACT_CATALOG_BYTES} bytes"),
        ));
    }
    Ok(())
}

struct NativeBuilderExtension {
    source: finstack_ai_kernel::ComponentId,
    model: (ComponentRef, Arc<dyn Model>),
    store: (ComponentRef, Arc<dyn finstack_ai_runtime::JournalStore>),
    toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn Observer>)>,
}

impl Extension for NativeBuilderExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor::trusted_in_process(self.source.clone(), PREVIEW_ENGINE_VERSION)
    }

    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
        registrar.model(
            registration_metadata(&self.model.0),
            ReadyComponent::new(Arc::clone(&self.model.1)),
        )?;
        for (component, toolset) in &self.toolsets {
            registrar.toolset(
                registration_metadata(component),
                ReadyComponent::new(Arc::clone(toolset)),
            )?;
        }
        for (component, provider) in &self.context_providers {
            registrar.context_provider(
                registration_metadata(component),
                ReadyComponent::new(Arc::clone(provider)),
            )?;
        }
        for (component, middleware) in &self.middleware {
            registrar.middleware(
                registration_metadata(component),
                ReadyComponent::new(Arc::clone(middleware)),
            )?;
        }
        for (component, observer) in &self.observers {
            registrar.observer(
                registration_metadata(component),
                ReadyComponent::new(Arc::clone(observer)),
            )?;
        }
        registrar.store(
            registration_metadata(&self.store.0),
            ReadyComponent::new(Arc::clone(&self.store.1)),
        )
    }
}

fn registration_metadata(component: &ComponentRef) -> RegistrationMetadata {
    RegistrationMetadata::new(
        component.id().clone(),
        component.version().unwrap_or(PREVIEW_ENGINE_VERSION),
    )
}

fn validate_exact_component(component: &ComponentRef) -> Result<(), AgentRunError> {
    if component.version().is_none() {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            format!("component {} requires an exact version", component.id()),
        ));
    }
    Ok(())
}
