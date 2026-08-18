use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{
    AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleDefaults,
    BundleResolver, BundleSpec, CapabilityActivation, CapabilityRef, CapabilitySpec,
    CompatibilityRequirements, Extension, ExtensionDescriptor, InstructionSpec, ReadyComponent,
    Registrar, RegistrationError, RegistrationMetadata, Registry, RunPolicy, RuntimeServices,
};
use finstack_ai_kernel::{AgentId, BundleId, CapabilityId, ComponentRef, MiddlewareRef, Version};
use finstack_ai_runtime::{ContextProvider, Middleware, Model, Observer, Toolset};

use super::handle::{Agent, ModelCapabilityVariant};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, CapabilityCatalogEntry,
    MAX_COMPACT_CATALOG_BYTES,
};

/// Ergonomic native composition builder over direct ready handles.
///
/// It creates the same strict `AgentSpec`, bundle lock, and no-lookup run plan
/// as the lower-level catalog/registry APIs. No credentials are serialized.
pub struct NativeAgentBuilder {
    agent_id: AgentId,
    bundle_id: BundleId,
    model: (ComponentRef, Arc<dyn Model>),
    store: (ComponentRef, Arc<dyn finstack_ai_runtime::JournalStore>),
    toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn Observer>)>,
    instructions: Vec<InstructionSpec>,
    capabilities: Vec<CapabilitySpec>,
    active_application: BTreeSet<CapabilityId>,
    policy: RunPolicy,
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
            instructions: Vec::new(),
            capabilities: Vec::new(),
            active_application: BTreeSet::new(),
            policy: RunPolicy::default(),
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
            .push(InstructionSpec::try_new(text).map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?);
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
        validate_builder_components(&self)?;
        let extension = NativeBuilderExtension {
            source: finstack_ai_kernel::ComponentId::parse("finstack.sdk.native-builder").map_err(
                |error| {
                    AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
                },
            )?,
            model: self.model.clone(),
            store: self.store.clone(),
            toolsets: self.toolsets.clone(),
            context_providers: self.context_providers.clone(),
            middleware: self.middleware.clone(),
            observers: self.observers.clone(),
        };
        let mut registrar = Registrar::new();
        registrar.register_extension(&extension).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
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
            .map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?;
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
            .map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?;
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
                .map_err(|error| {
                    AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
                })?
        };
        let mut agent = Agent::try_from_resolved(Arc::new(composed_agent))?;
        let variants =
            resolve_model_variants(&self.capabilities, &bundle_resolver, &mut registry, &agent)
                .await?;
        agent.model_capabilities = variants.into();
        Ok(agent)
    }
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
            MiddlewareRef::try_new(component.clone(), None::<&str>).map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })
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
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })
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
            .map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?;
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
        capability.validate().map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
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

const PREVIEW_ENGINE_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

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
