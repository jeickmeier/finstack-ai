use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ResolvedAgent;
use finstack_ai_kernel::ToolFailurePolicy;
use finstack_ai_kernel::{
    AgentId, BundleId, ComponentInvocation, ComponentRef, Digest, InvocationRecovery,
    JsonSchemaDraft, RawJson, SchemaRef,
};
use finstack_ai_runtime::artifact::ArtifactStore;
use finstack_ai_runtime::ports::model::Model;
use finstack_ai_runtime::ports::tool::{
    JsonSchemaToolValidatorCompiler, ResolvedToolCatalog, ToolExecutionPolicy, ToolPolicyDecision,
    ToolValidator, ToolValidatorCompiler, ToolsetRegistration,
};

use super::activation::NativeCapabilityHost;
use super::builder::NativeAgentBuilder;
use super::history::{HistoryCache, HistoryCachePolicy};
use super::mask::CapabilityContributionIndex;
use super::run::{AgentRun, AgentRunInner, CancellationState, EventStreamState, publish_result};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunOutput, AgentRunRequest,
    CapabilityCatalogEntry,
};
use crate::{CapabilityActivation, CapabilitySpec};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver as driver;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::native_driver as driver;

#[derive(Clone)]
pub(super) struct ModelCapabilityVariant {
    pub(super) entry: CapabilityCatalogEntry,
    // Cache both the immutable catalog and any deterministic preparation failure.
    pub(super) prepared: Result<Arc<Agent>, AgentRunError>,
}

/// Immutable native facade over one fully resolved agent.
///
/// Construct with [`Agent::builder`] then [`NativeAgentBuilder::build`].
/// `start` / `run` execute one bounded turn. Dropping an [`AgentRun`]
/// detaches observation; it does not cancel the durable run.
#[derive(Clone)]
pub struct Agent {
    pub(super) resolved: Arc<ResolvedAgent>,
    pub(super) tools: Arc<ResolvedToolCatalog>,
    pub(super) structured_output: Option<StructuredOutputConfig>,
    pub(super) model_capabilities: Arc<[ModelCapabilityVariant]>,
    pub(super) capability_specs: Arc<[CapabilitySpec]>,
    pub(super) capability_index: CapabilityContributionIndex,
    pub(super) activation_host: Option<Arc<NativeCapabilityHost>>,
    pub(super) artifact_store: Option<Arc<dyn ArtifactStore>>,
    pub(super) rebuild: Option<Arc<NativeAgentBuilder>>,
    pub(super) history_cache_policy: HistoryCachePolicy,
    pub(super) history_cache: Arc<Mutex<HistoryCache>>,
}

#[derive(Clone)]
pub(super) struct StructuredOutputConfig {
    pub(super) schema_ref: SchemaRef,
    pub(super) validator: Arc<dyn ToolValidator>,
}

fn prepare_tool_catalog(resolved: &ResolvedAgent) -> Result<ResolvedToolCatalog, AgentRunError> {
    let plan = resolved.run_plan();
    let lock = resolved.lock().ok_or_else(|| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "native Agent requires an exact resolved lock",
        )
    })?;
    let registrations = plan
        .toolsets()
        .iter()
        .map(|component| {
            let toolset = Arc::clone(component.handle());
            let descriptor = component.descriptor();
            let locked = lock
                .components
                .iter()
                .filter(|entry| {
                    entry.component.id() == descriptor.component.id()
                        && entry.kind == crate::LockedComponentKind::Toolset
                })
                .collect::<Vec<_>>();
            if locked.len() != 1 || locked[0].component.version() != descriptor.component.version()
            {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "toolset invocation is missing an exact lock component",
                ));
            }
            let invocation = ComponentInvocation {
                component: descriptor.component.id().clone(),
                version: descriptor.component.version().ok_or_else(|| {
                    AgentRunError::configuration(
                        AGENT_RUN_INVALID_CONFIGURATION,
                        "toolset invocation component version is missing",
                    )
                })?,
                configuration_digest: locked[0]
                    .config_digest
                    .unwrap_or_else(|| Digest::raw_json(b"{}")),
                recovery: InvocationRecovery::RecomputeSafe,
            };
            let policies = toolset
                .tools()
                .iter()
                .map(|tool| {
                    let approval = match tool.approval.requirement {
                        finstack_ai_runtime::ports::model::ApprovalRequirement::NotRequired => {
                            ToolPolicyDecision::Allow
                        }
                        finstack_ai_runtime::ports::model::ApprovalRequirement::Required
                        | finstack_ai_runtime::ports::model::ApprovalRequirement::Policy => {
                            ToolPolicyDecision::RequireApproval
                        }
                    };
                    (
                        tool.id.clone(),
                        ToolExecutionPolicy {
                            failure_policy: ToolFailurePolicy::ReturnToModel,
                            approval,
                            max_concurrency: 4,
                        },
                    )
                })
                .collect();
            let components = toolset
                .tools()
                .iter()
                .map(|tool| (tool.id.clone(), invocation.clone()))
                .collect();
            Ok(ToolsetRegistration {
                toolset,
                policies,
                components,
            })
        })
        .collect::<Result<Vec<_>, AgentRunError>>()?;
    ResolvedToolCatalog::try_new(
        registrations,
        &BTreeMap::new(),
        &JsonSchemaToolValidatorCompiler,
    )
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })
}

impl Agent {
    /// Start ergonomic native composition over direct model and store handles.
    ///
    /// This is the live-handle constructor ([`NativeAgentBuilder`]). Use
    /// [`crate::AgentSpec::builder`] when you only need declarative spec data.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - Stable agent identity recorded in the bundle spec and lock.
    /// * `bundle_id` - Bundle identity that owns this agent.
    /// * `model` - Exact-version component ref plus a ready [`Model`] handle.
    /// * `store` - Exact-version component ref plus a ready journal-store handle.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use finstack_ai::runtime::ports::journal::JournalStore;
    /// use finstack_ai::runtime::ports::model::Model;
    /// use finstack_ai::{Agent, AgentId, BundleId, ComponentId, ComponentRef, Version};
    ///
    /// # async fn compose(
    /// #     model: Arc<dyn Model>,
    /// #     store: Arc<dyn JournalStore>,
    /// # ) -> Result<(), Box<dyn std::error::Error>> {
    /// const VERSION: Version = Version { major: 0, minor: 1, patch: 0 };
    ///
    /// let agent = Agent::builder(
    ///     AgentId::parse("demo.agent")?,
    ///     BundleId::parse("demo.bundle")?,
    ///     (ComponentRef::new(ComponentId::parse("demo.model")?, Some(VERSION)), model),
    ///     (ComponentRef::new(ComponentId::parse("demo.store")?, Some(VERSION)), store),
    /// )
    /// .try_instruction("Answer directly.")?
    /// .build()
    /// .await?;
    /// # let _ = agent;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn builder(
        agent_id: AgentId,
        bundle_id: BundleId,
        model: (ComponentRef, Arc<dyn Model>),
        store: (
            ComponentRef,
            Arc<dyn finstack_ai_runtime::ports::journal::JournalStore>,
        ),
    ) -> NativeAgentBuilder {
        NativeAgentBuilder::new(agent_id, bundle_id, model, store)
    }

    /// Validate and retain one resolved, no-lookup execution plan.
    ///
    /// This is the bridge from the registry and bundle path to a runnable
    /// agent: [`Registry::resolve`](crate::registry::Registry::resolve) and
    /// [`BundleResolver::resolve_agent`](crate::BundleResolver::resolve_agent)
    /// both yield a [`ResolvedAgent`], which is inspection-only until it is
    /// turned into an [`Agent`] here.
    ///
    /// Supports direct model, Toolset, context-provider, middleware, and
    /// observer handles. Observer failures are isolated from run semantics.
    ///
    /// # Errors
    ///
    /// Fails closed when the resolved value carries no specification or lock.
    pub fn try_from_resolved(resolved: Arc<ResolvedAgent>) -> Result<Self, AgentRunError> {
        if resolved.spec().is_none() || resolved.lock().is_none() {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires a bundle-resolved specification and lock",
            ));
        }
        let tools = prepare_tool_catalog(&resolved)?;
        let specs: Arc<[CapabilitySpec]> = match resolved.composition() {
            Some(recipe) => recipe
                .capabilities
                .values()
                .map(|(_, capability)| capability.as_ref().clone())
                .collect::<Vec<_>>()
                .into(),
            None if resolved
                .lock()
                .is_some_and(|lock| lock.capabilities.is_empty()) =>
            {
                Arc::from([])
            }
            None => {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "capability definitions are missing from the resolved composition",
                ));
            }
        };
        let index = CapabilityContributionIndex::from_specs(&specs);
        let mut agent = Self {
            resolved,
            tools: Arc::new(tools),
            structured_output: None,
            model_capabilities: Arc::from([]),
            capability_specs: Arc::from([]),
            capability_index: CapabilityContributionIndex::default(),
            activation_host: None,
            artifact_store: None,
            rebuild: None,
            history_cache_policy: HistoryCachePolicy::default(),
            history_cache: HistoryCache::shared(HistoryCachePolicy::default()),
        };
        agent.attach_capability_surface(specs, index, None)?;
        Ok(agent)
    }

    /// Build a new agent from reconstructed catalogs.
    ///
    /// MCP `list_changed` never mutates this lock. The returned agent has a
    /// new lock. In-flight runs keep the previous composition.
    ///
    /// # Errors
    ///
    /// Returns [`AGENT_RUN_INVALID_CONFIGURATION`] when the agent was not
    /// built through [`NativeAgentBuilder`], or when a catalog
    /// reconstruction fails.
    pub async fn re_resolve(&self) -> Result<Self, AgentRunError> {
        let builder = self
            .rebuild
            .as_ref()
            .ok_or_else(|| {
                AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "re_resolve requires a builder-built agent",
                )
            })?
            .as_ref()
            .clone();
        let mut rebuilt = builder.rebuild_from_live_catalogs().await?;
        rebuilt.history_cache_policy = self.history_cache_policy;
        rebuilt.history_cache = Arc::clone(&self.history_cache);
        Ok(rebuilt)
    }

    /// Return an immutable composition with a fresh bounded history cache.
    #[must_use]
    pub fn with_history_cache_policy(mut self, policy: HistoryCachePolicy) -> Self {
        let cache = HistoryCache::shared(policy);
        self.history_cache_policy = policy;
        self.history_cache = Arc::clone(&cache);
        self.model_capabilities = self
            .model_capabilities
            .iter()
            .map(|variant| {
                let prepared = match &variant.prepared {
                    Ok(agent) => {
                        let mut agent = agent.as_ref().clone();
                        agent.history_cache_policy = policy;
                        agent.history_cache = Arc::clone(&cache);
                        Ok(Arc::new(agent))
                    }
                    Err(error) => Err(error.clone()),
                };
                ModelCapabilityVariant {
                    entry: variant.entry.clone(),
                    prepared,
                }
            })
            .collect::<Vec<_>>()
            .into();
        if let Some(rebuild) = &self.rebuild {
            self.rebuild = Some(Arc::new(
                rebuild.as_ref().clone().history_cache_policy(policy),
            ));
        }
        self
    }

    pub(super) fn attach_capability_surface(
        &mut self,
        specs: Arc<[CapabilitySpec]>,
        index: CapabilityContributionIndex,
        host: Option<Arc<NativeCapabilityHost>>,
    ) -> Result<(), AgentRunError> {
        for provider in self.resolved.run_plan().context_providers() {
            let component_id = provider.descriptor().component.id();
            let Some(owners) = index.owners().get(component_id) else {
                continue;
            };
            let untrusted = specs.iter().any(|spec| {
                owners.contains(&spec.id) && spec.activation == CapabilityActivation::Model
            });
            if untrusted
                && provider
                    .handle()
                    .descriptor()
                    .trusted_application_instructions
            {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "untrusted capability cannot set trusted_application_instructions",
                ));
            }
        }
        self.capability_specs = specs;
        self.capability_index = index;
        self.activation_host = host;
        Ok(())
    }

    pub(super) fn validate_restored_mask(
        &self,
        active: &[finstack_ai_kernel::ActiveCapability],
    ) -> Result<(), AgentRunError> {
        let Some(lock) = self.resolved.lock() else {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires an exact resolved lock",
            ));
        };
        for item in active {
            if !lock
                .capabilities
                .iter()
                .any(|capability| capability.id == item.capability_id)
            {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "capability_mask_not_in_lock",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn live_tool_specs(
        &self,
        active: &[finstack_ai_kernel::ActiveCapability],
    ) -> Vec<finstack_ai_runtime::ports::model::ToolSpec> {
        self.tools
            .tools()
            .filter(|tool| {
                match tool
                    .component
                    .as_ref()
                    .map(|invocation| &invocation.component)
                {
                    Some(component) => self.capability_index.allows(component, active),
                    None => true,
                }
            })
            .map(|tool| tool.spec.clone())
            .collect()
    }

    /// Return an agent configured for one compile-once Draft 2020-12 output schema.
    ///
    /// # Arguments
    ///
    /// * `schema` - Canonical JSON Schema document compiled by the offline Rust validator.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error when the schema cannot be compiled
    /// by the canonical offline Rust validator.
    pub fn try_with_output_schema(mut self, schema: &RawJson) -> Result<Self, AgentRunError> {
        let validator = JsonSchemaToolValidatorCompiler
            .compile(schema, &BTreeMap::new())
            .map_err(|error| {
                AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    format!("structured output schema is invalid: {}", error.message()),
                )
            })?;
        let schema_ref = SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: Digest::raw_json(schema.as_bytes()),
        };
        self.structured_output = Some(StructuredOutputConfig {
            schema_ref,
            validator,
        });
        Ok(self)
    }

    /// Borrow the exact resolved composition retained by this facade.
    #[must_use]
    pub fn resolved(&self) -> &Arc<ResolvedAgent> {
        &self.resolved
    }

    /// Return the bounded model-activated capability catalog in identity order.
    #[must_use]
    pub fn capability_catalog(&self) -> Vec<CapabilityCatalogEntry> {
        self.model_capabilities
            .iter()
            .map(|variant| variant.entry.clone())
            .collect()
    }

    /// Render the compact catalog supplied to model-facing integrations.
    #[must_use]
    pub fn compact_capability_catalog(&self) -> String {
        self.model_capabilities
            .iter()
            .map(|variant| variant.entry.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Start one bounded native run and return its detached control handle.
    ///
    /// The caller must already be inside the selected runtime driver. Dropping the returned
    /// handle detaches frontend observation; it does not cancel the durable run.
    /// `request.capability` selects a model-activated variant; `None` runs `self`.
    /// An unknown catalog id fails closed. User-input word overlap is not used.
    ///
    /// # Arguments
    ///
    /// * `request` - Bounded run input, security context, limits, and optional capability id.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration or runtime error before the background
    /// run task is accepted.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn demo(agent: &finstack_ai::Agent, request: finstack_ai::AgentRunRequest)
    /// #     -> Result<(), finstack_ai::AgentRunError> {
    /// let run = agent.start(request)?;
    /// let output = run.result().await?;
    /// let _ = output.text();
    /// # Ok(())
    /// # }
    /// ```
    pub fn start(&self, request: AgentRunRequest) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_request(&request)?;
        let prepared = selected.prepare(request)?;
        Self::spawn_prepared(selected, prepared)
    }

    /// Start a new root run on an existing idle lane.
    ///
    /// # Arguments
    ///
    /// * `lane` - Idle lane that already belongs to this agent's journal store.
    /// * `request` - Bounded run input; `capability` selects a model-activated variant.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane or configuration/runtime failure.
    ///
    /// Internal on purpose: it starts the run without the lane registration
    /// that [`Lane::run`](crate::Lane::run) performs, so a run started here
    /// would leave `Lane::suspend` silently parking nothing. Callers use
    /// `Lane::run`.
    pub(crate) fn start_on_lane(
        &self,
        lane: &crate::Lane,
        request: AgentRunRequest,
    ) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_request(&request)?;
        let prepared = selected.prepare_on(request, lane)?;
        Self::spawn_prepared(selected, prepared)
    }

    /// Start one run on a frozen locator and already-validated acceptance.
    ///
    /// Used by child accept after a durable `ChildRunPrepared` mapping exists.
    ///
    /// # Errors
    ///
    /// Returns a configuration or runtime failure when the child cannot start.
    pub(super) fn start_prepared(
        &self,
        request: AgentRunRequest,
        locator: finstack_ai_kernel::OperationLocator,
        accepted: finstack_ai_kernel::RunAccepted,
        session: crate::Session,
    ) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_request(&request)?;
        let prepared = selected.prepare_accepted(request, locator, accepted, session)?;
        Self::spawn_prepared(selected, prepared)
    }

    fn spawn_prepared(
        agent: Self,
        prepared: super::prepare::PreparedAgentRun,
    ) -> Result<AgentRun, AgentRunError> {
        let locator = prepared.locator.clone();
        let store = Arc::clone(&prepared.store);
        let cancellation_initiator = prepared.cancellation_initiator()?;
        let child_runs = agent
            .resolved
            .spec()
            .map(|spec| spec.policy.child_runs)
            .unwrap_or_default();
        let inner = Arc::new(AgentRunInner {
            lifecycle: Arc::new(super::lifecycle::ExecutionLifecycle::new(Some(
                prepared.accepted.resolved_agent_lock_digest(),
            ))),
            locator,
            store,
            child_runs,
            cancellation_initiator,
            handle: Mutex::new(None),
            handle_ready: driver::Signal::new(),
            result: Mutex::new(None),
            result_ready: driver::Signal::new(),
            events: Mutex::new(EventStreamState::Waiting),
            events_fault: OnceLock::new(),
            cancellation: Mutex::new(CancellationState::default()),
            cancellation_ready: driver::Signal::new(),
            children: Mutex::new(Vec::new()),
            remote_invoker: Mutex::new(None),
            remote_child: None,
        });
        let execution = Arc::downgrade(&inner);
        driver::spawn(Box::pin(async move {
            let result = Box::pin(agent.execute_started(prepared, &execution)).await;
            publish_result(&execution, result);
        }))
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        Ok(AgentRun { inner })
    }

    /// Borrow the resolved journal store.
    #[must_use]
    pub fn journal_store(&self) -> Arc<dyn finstack_ai_runtime::ports::journal::JournalStore> {
        Arc::clone(self.resolved.run_plan().store().handle())
    }

    /// Replay one stored session into a provisional Rust-owned inspect snapshot.
    ///
    /// # Errors
    ///
    /// Returns a recover failure when the journal cannot be loaded or replayed.
    pub async fn inspect_session(
        &self,
        session_id: finstack_ai_kernel::SessionId,
    ) -> Result<
        finstack_ai_runtime::session::SessionInspectSnapshot,
        finstack_ai_runtime::session::SessionError,
    > {
        finstack_ai_runtime::session::inspect_session(self.journal_store(), session_id).await
    }

    fn select_for_request(&self, request: &AgentRunRequest) -> Result<Self, AgentRunError> {
        let Some(capability) = request.capability.as_ref() else {
            return Ok(self.clone());
        };
        let variant = self
            .model_capabilities
            .iter()
            .find(|variant| variant.entry.id == *capability)
            .ok_or_else(|| {
                AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    format!("unknown model capability {capability}"),
                )
            })?;
        let prepared = variant.prepared.as_ref().map_err(AgentRunError::clone)?;
        let mut agent = prepared.as_ref().clone();
        agent.structured_output.clone_from(&self.structured_output);
        agent.history_cache_policy = self.history_cache_policy;
        agent.history_cache = Arc::clone(&self.history_cache);
        Ok(agent)
    }

    /// Execute one bounded native run through the commit-before-effect runtime.
    ///
    /// Start-and-wait convenience over [`Self::start`] plus [`AgentRun::result`].
    /// `request.capability` selects a model-activated variant; `None` runs `self`.
    ///
    /// # Arguments
    ///
    /// * `request` - Bounded run input, security context, limits, and optional capability id.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration, runtime, cancellation, or timeout error.
    pub async fn run(&self, request: AgentRunRequest) -> Result<AgentRunOutput, AgentRunError> {
        let run = self.start(request)?;
        // The convenience API returns only the terminal output, so no caller can
        // consume its interactive event stream. Close that subscription before
        // waiting to keep durable event delivery from backpressuring execution.
        run.close_events();
        run.result().await
    }
}
