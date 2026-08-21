use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{
    AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleDefaults,
    BundleResolver, BundleSpec, CapabilityActivation, CapabilityRef, CapabilitySpec,
    CompatibilityRequirements, Extension, ExtensionDescriptor, InstructionSpec, ReadyComponent,
    Registrar, RegistrationError, RegistrationMetadata, Registry, RunPolicy, RuntimeServices,
};
use finstack_ai_kernel::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentRef, MiddlewareRef,
};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::{AgentInvoker, ChildRunStarter};
use finstack_ai_runtime::{
    ArtifactStore, ChildRunPolicy, ContextProvider, Middleware, Model, Observer, Toolset,
};

use super::PREVIEW_ENGINE_VERSION;
use super::handle::{Agent, ModelCapabilityVariant};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, CapabilityCatalogEntry,
    MAX_COMPACT_CATALOG_BYTES,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum PortScope {
    Base,
    CapabilityOnly,
}

struct PortHandle<T: ?Sized> {
    component: ComponentRef,
    handle: Arc<T>,
    scope: PortScope,
}

impl<T: ?Sized> Clone for PortHandle<T> {
    fn clone(&self) -> Self {
        Self {
            component: self.component.clone(),
            handle: Arc::clone(&self.handle),
            scope: self.scope,
        }
    }
}

impl<T: ?Sized> PortHandle<T> {
    fn base(component: ComponentRef, handle: Arc<T>) -> Self {
        Self {
            component,
            handle,
            scope: PortScope::Base,
        }
    }

    fn capability_only(component: ComponentRef, handle: Arc<T>) -> Self {
        Self {
            component,
            handle,
            scope: PortScope::CapabilityOnly,
        }
    }
}

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
    toolsets: Vec<PortHandle<dyn Toolset>>,
    context_providers: Vec<PortHandle<dyn ContextProvider>>,
    middleware: Vec<PortHandle<dyn Middleware>>,
    observers: Vec<(ComponentRef, Arc<dyn Observer>)>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    instructions: Vec<InstructionSpec>,
    capabilities: Vec<CapabilitySpec>,
    active_application: BTreeSet<CapabilityId>,
    policy: RunPolicy,
    child_policy_binding: Option<ChildRunPolicy>,
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
            artifact_store: None,
            instructions: Vec::new(),
            capabilities: Vec::new(),
            active_application: BTreeSet::new(),
            policy: RunPolicy::default(),
            child_policy_binding: None,
            activation_host: None,
        }
    }

    /// Bind the artifact store used for staging and committed-reference ownership.
    #[must_use]
    pub fn artifact_store(mut self, store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
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
        self.toolsets.push(PortHandle::base(component, toolset));
        self
    }

    /// Atomically bind a child toolset to this builder's journal and policy.
    ///
    /// The resulting build fails closed if [`Self::policy`] is subsequently
    /// changed to a different child-run policy.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the toolset factory rejects its
    /// host-bound starter.
    #[cfg(feature = "native-tokio")]
    pub fn try_child_toolset<F, E>(
        mut self,
        component: ComponentRef,
        invoker: Arc<dyn AgentInvoker>,
        factory: F,
    ) -> Result<Self, AgentRunError>
    where
        F: FnOnce(Arc<ChildRunStarter>) -> Result<Arc<dyn Toolset>, E>,
        E: std::fmt::Display,
    {
        let child_policy = self.policy.child_runs;
        if self
            .child_policy_binding
            .is_some_and(|bound| bound != child_policy)
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "child toolsets must share one frozen ChildRunPolicy",
            ));
        }
        let starter = Arc::new(ChildRunStarter::new(
            Arc::clone(&self.store.1),
            child_policy,
            invoker,
        ));
        let toolset = factory(starter).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        self.child_policy_binding = Some(child_policy);
        self.toolsets.push(PortHandle::base(component, toolset));
        Ok(self)
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
        self.context_providers
            .push(PortHandle::base(component, provider));
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
        self.toolsets
            .push(PortHandle::capability_only(component, toolset));
        self
    }

    /// Register a context provider handle without adding it to the base spec.
    #[must_use]
    pub fn capability_context_provider(
        mut self,
        component: ComponentRef,
        provider: Arc<dyn ContextProvider>,
    ) -> Self {
        self.context_providers
            .push(PortHandle::capability_only(component, provider));
        self
    }

    /// Register a Middleware handle without adding it to the base spec.
    #[must_use]
    pub fn capability_middleware(
        mut self,
        component: ComponentRef,
        middleware: Arc<dyn Middleware>,
    ) -> Self {
        self.middleware
            .push(PortHandle::capability_only(component, middleware));
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
        self.middleware
            .push(PortHandle::base(component, middleware));
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
    ///   [`crate::AgentRunRequest::capability`], not by user-input overlap.
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
    /// Child runs default to [`crate::ChildRunPolicy::Deny`]. Paid-tool
    /// approvals default to [`crate::ApprovalGrantMode::PerCall`].
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
        validate_builder_components(&self)?;
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&self)
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
        let resolved_agent = bundle_resolver
            .resolve_agent(
                &mut registry,
                &self.bundle_id,
                &self.agent_id,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .map_err(invalid_config)?;
        let resolved_agent = if self.active_application.is_empty() {
            resolved_agent
        } else {
            bundle_resolver
                .activate_application(
                    &mut registry,
                    &resolved_agent,
                    self.active_application.clone(),
                    AgentConstructionContext::new(),
                )
                .await
                .map_err(invalid_config)?
        };
        let mut agent = Agent::try_from_resolved(Arc::new(resolved_agent))?;
        let contributions =
            super::mask::CapabilityContributionIndex::from_specs(&self.capabilities);
        agent.attach_capability_surface(
            Arc::from(self.capabilities.clone()),
            contributions,
            self.activation_host.clone(),
        )?;
        agent.model_capabilities =
            resolve_model_variants(&self.capabilities, &bundle_resolver, &mut registry, &agent)
                .await?
                .into();
        agent.artifact_store = self.artifact_store.clone();
        agent.rebuild = Some(Arc::new(self));
        Ok(agent)
    }

    pub(super) async fn rebuild_from_live_catalogs(mut self) -> Result<Agent, AgentRunError> {
        self.toolsets = reconstruct_toolsets(self.toolsets).await?;
        self.context_providers = reconstruct_providers(self.context_providers).await?;
        self.build().await
    }
}

async fn reconstruct_toolsets(
    entries: Vec<PortHandle<dyn Toolset>>,
) -> Result<Vec<PortHandle<dyn Toolset>>, AgentRunError> {
    let mut rebuilt = Vec::with_capacity(entries.len());
    for entry in entries {
        let next = entry.handle.reconstruct().await.map_err(invalid_config)?;
        rebuilt.push(PortHandle {
            component: entry.component,
            handle: next.unwrap_or(entry.handle),
            scope: entry.scope,
        });
    }
    Ok(rebuilt)
}

async fn reconstruct_providers(
    entries: Vec<PortHandle<dyn ContextProvider>>,
) -> Result<Vec<PortHandle<dyn ContextProvider>>, AgentRunError> {
    let mut rebuilt = Vec::with_capacity(entries.len());
    for entry in entries {
        let next = entry.handle.reconstruct().await.map_err(invalid_config)?;
        rebuilt.push(PortHandle {
            component: entry.component,
            handle: next.unwrap_or(entry.handle),
            scope: entry.scope,
        });
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
        .filter(|port| port.scope == PortScope::Base)
        .map(|port| {
            MiddlewareRef::try_new(port.component.clone(), None::<&str>).map_err(invalid_config)
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
            .filter(|port| port.scope == PortScope::Base)
            .map(|port| port.component.clone())
            .collect::<Vec<_>>(),
    )
    .context_providers(
        builder
            .context_providers
            .iter()
            .filter(|port| port.scope == PortScope::Base)
            .map(|port| port.component.clone())
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
    if builder
        .child_policy_binding
        .is_some_and(|bound| bound != builder.policy.child_runs)
    {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "child toolset policy drifted after binding",
        ));
    }
    validate_exact_component(&builder.model.0)?;
    validate_exact_component(&builder.store.0)?;
    for port in &builder.toolsets {
        validate_exact_component(&port.component)?;
    }
    for port in &builder.context_providers {
        validate_exact_component(&port.component)?;
    }
    for port in &builder.middleware {
        validate_exact_component(&port.component)?;
    }
    for (component, _) in &builder.observers {
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
        let prepared = Agent::try_from_resolved(Arc::new(resolved)).and_then(|mut variant| {
            variant.attach_capability_surface(
                Arc::clone(&agent.capability_specs),
                agent.capability_index.clone(),
                agent.activation_host.clone(),
            )?;
            Ok(Arc::new(variant))
        });
        variants.push(ModelCapabilityVariant {
            entry: CapabilityCatalogEntry {
                id: capability.id.clone(),
                description: Arc::clone(&capability.description),
            },
            prepared,
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

impl Extension for NativeAgentBuilder {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor::trusted_in_process(native_builder_source(), PREVIEW_ENGINE_VERSION)
    }

    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
        registrar.model(
            registration_metadata(&self.model.0),
            ReadyComponent::new(Arc::clone(&self.model.1)),
        )?;
        for port in &self.toolsets {
            registrar.toolset(
                registration_metadata(&port.component),
                ReadyComponent::new(Arc::clone(&port.handle)),
            )?;
        }
        for port in &self.context_providers {
            registrar.context_provider(
                registration_metadata(&port.component),
                ReadyComponent::new(Arc::clone(&port.handle)),
            )?;
        }
        for port in &self.middleware {
            registrar.middleware(
                registration_metadata(&port.component),
                ReadyComponent::new(Arc::clone(&port.handle)),
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

fn native_builder_source() -> ComponentId {
    finstack_ai_kernel::static_key!(ComponentId, "finstack.sdk.native-builder")
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
