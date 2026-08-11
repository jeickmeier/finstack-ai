//! Native developer-preview execution facade.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, AgentId, AllocatedIds, AppendBatchTag, BudgetPropagation, BundleId,
    CancellationPropagation, ComponentRef, ContentBlock, DeadlinePropagation, Digest,
    EffectOutputContract, EffectOutputKind, EventTag, KernelInput, LaneId, LaneTag, Message,
    MessageId, MessageRole, MessageTag, Metadata, ModelRequestTag, OperationLocator,
    PrincipalPropagation, ProviderIds, RawJson, RecordTag, ReducerStageOutcome, RetrySafety,
    RunAccepted, RunPhase, RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag,
    SessionId, SessionTag, Stage, StageCursor, StageSettled, TerminalState, TextBlock, Timestamp,
    TransitionEnv, TurnTag, Version,
};
use finstack_ai_runtime::{
    CommitCoordinator, EventHubConfig, IdGenerationError, JsonSchemaToolValidatorCompiler,
    LockedModelContextProfile, Model, ModelCapabilities, ModelContextProfileOverride,
    ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelReconcileResult, ModelRequest,
    ModelRequestDraft, ModelRequestLimits, ModelSettings, ModelTaskConfig, ModelTokenEstimate,
    ModelWarmupContext, OsRandomSource, PendingModelEffect, PortFuture, ReconcileContext,
    ResolvedToolCatalog, RunHandle, RunHandleError, RunTaskConfig, RunTaskOwner, SideEffectClass,
    SystemClock, ToolExecutionPolicy, ToolFailurePolicy, ToolPolicyDecision, ToolStreamLimits,
    ToolTaskConfig, Toolset, ToolsetRegistration, UuidV7Generator, resolve_model_context_profile,
};
use thiserror::Error;

use crate::{
    AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleDefaults,
    BundleResolver, BundleSpec, CompatibilityRequirements, Extension, ExtensionDescriptor,
    InstructionSpec, ReadyComponent, Registrar, RegistrationError, RegistrationMetadata,
    ResolvedAgent, RuntimeServices,
};

/// Invalid public run configuration.
pub const AGENT_RUN_INVALID_CONFIGURATION: &str = "agent_run_invalid_configuration";
/// The resolved plan contains a stage not supported by the native preview driver.
pub const AGENT_RUN_UNSUPPORTED_PLAN: &str = "agent_run_unsupported_plan";
/// The commit-before-effect runtime failed.
pub const AGENT_RUN_RUNTIME_FAILURE: &str = "agent_run_runtime_failure";
/// The operational run deadline elapsed.
pub const AGENT_RUN_TIMEOUT: &str = "agent_run_timeout";

const DEFAULT_QUEUE_CAPACITY: usize = 32;
const DEFAULT_MAX_CYCLES: u64 = 16;
const MAX_CONFIGURED_CYCLES: u64 = 1_024;

/// Immutable native facade over one fully resolved agent.
pub struct Agent {
    resolved: Arc<ResolvedAgent>,
    tools: Arc<ResolvedToolCatalog>,
}

impl Agent {
    /// Start ergonomic native composition over direct model and store handles.
    #[must_use]
    pub fn builder(
        agent_id: AgentId,
        bundle_id: BundleId,
        model: (ComponentRef, Arc<dyn Model>),
        store: (ComponentRef, Arc<dyn finstack_ai_runtime::JournalStore>),
    ) -> NativeAgentBuilder {
        NativeAgentBuilder::new(agent_id, bundle_id, model, store)
    }

    /// Validate and retain one resolved, no-lookup execution plan.
    ///
    /// The developer preview supports direct model and Toolset handles. It
    /// rejects non-empty context-provider, middleware, and observer selections
    /// instead of silently bypassing their stage contracts.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error when the agent did not come from a
    /// bundle resolver, contains a deferred stage, or has an invalid tool catalog.
    pub fn try_from_resolved(resolved: Arc<ResolvedAgent>) -> Result<Self, AgentRunError> {
        if resolved.spec().is_none() || resolved.lock().is_none() {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires a bundle-resolved specification and lock",
            ));
        }
        let plan = resolved.run_plan();
        if !plan.context_providers().is_empty()
            || !plan.middleware().is_empty()
            || !plan.observers().is_empty()
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_UNSUPPORTED_PLAN,
                "native preview supports model and Toolset stages only",
            ));
        }

        let registrations = plan
            .toolsets()
            .iter()
            .map(|component| {
                let toolset = Arc::clone(component.handle());
                let policies = toolset
                    .tools()
                    .iter()
                    .map(|tool| {
                        let approval = match tool.side_effect {
                            SideEffectClass::ReadOnly => ToolPolicyDecision::Allow,
                            SideEffectClass::IdempotentWrite
                            | SideEffectClass::NonIdempotentWrite => {
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
                ToolsetRegistration {
                    toolset,
                    policies,
                    components: BTreeMap::new(),
                }
            })
            .collect::<Vec<_>>();
        let tools = ResolvedToolCatalog::try_new(
            registrations,
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        Ok(Self {
            resolved,
            tools: Arc::new(tools),
        })
    }

    /// Borrow the exact resolved composition retained by this facade.
    #[must_use]
    pub fn resolved(&self) -> &Arc<ResolvedAgent> {
        &self.resolved
    }

    /// Execute one bounded native run through the commit-before-effect runtime.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration, runtime, terminal, or timeout error.
    #[expect(
        clippy::too_many_lines,
        reason = "construction keeps one resolved-model and owned-task lifetime contiguous"
    )]
    pub async fn run(&self, request: AgentRunRequest) -> Result<AgentRunOutput, AgentRunError> {
        request.validate()?;
        let plan = self.resolved.run_plan();
        let model = Arc::clone(plan.model().handle());
        validate_model_name(model.as_ref(), &request.model)?;
        let capabilities = model.capabilities(&request.model);
        let profile = resolve_model_context_profile(
            capabilities.context_profile,
            None,
            None::<&ModelContextProfileOverride>,
            false,
        )
        .map_err(AgentRunError::model)?;
        let store = Arc::clone(plan.store().handle());
        let spec = self.resolved.spec().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires a bundle-resolved specification and lock",
            )
        })?;
        let lock = self.resolved.lock().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires a bundle-resolved specification and lock",
            )
        })?;
        let session_id = NativeIds::generate::<SessionTag>()?;
        let lane_id = NativeIds::generate::<LaneTag>()?;
        let run_id = NativeIds::generate::<RunTag>()?;
        let locator =
            OperationLocator::try_new(request.security.tenant_scope(), session_id, lane_id, run_id)
                .map_err(|error| {
                    AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
                })?;
        let accepted = RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?,
            request.security.clone(),
            None,
            spec.limits.clone(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            lock.fingerprint().map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?,
            None,
        )
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;

        let ready_model: Arc<dyn Model> = Arc::new(ReadyModel(model));
        let coordinator = CommitCoordinator::new(Arc::clone(&store));
        let mut owner = if self.tools.is_empty() {
            RunTaskOwner::spawn_with_model(
                coordinator,
                run_task_config(),
                model_task_config(),
                ready_model,
                profile.clone(),
                SystemClock,
                OsRandomSource,
            )
            .await
            .map_err(AgentRunError::runtime)?
        } else {
            RunTaskOwner::spawn_with_model_and_tools(
                coordinator,
                run_task_config(),
                model_task_config(),
                tool_task_config(),
                ready_model,
                profile.clone(),
                Arc::clone(&self.tools),
                SystemClock,
                OsRandomSource,
            )
            .await
            .map_err(AgentRunError::runtime)?
        };
        let handle = owner.handle();
        let timeout = request.timeout;
        let result = finstack_ai_runtime::native_driver::timeout(
            timeout,
            Box::pin(self.drive(
                &handle, store, session_id, lane_id, accepted, request, profile, locator,
            )),
        )
        .await;
        let _shutdown = owner.shutdown().await;
        match result {
            Ok(value) => value,
            Err(_) => Err(AgentRunError::Timeout {
                code: AGENT_RUN_TIMEOUT,
                timeout,
            }),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the preview driver keeps the complete aggregate-stage sequence auditable"
    )]
    async fn drive(
        &self,
        handle: &RunHandle,
        store: Arc<dyn finstack_ai_runtime::JournalStore>,
        session_id: SessionId,
        lane_id: LaneId,
        accepted: RunAccepted,
        request: AgentRunRequest,
        profile: LockedModelContextProfile,
        locator: OperationLocator,
    ) -> Result<AgentRunOutput, AgentRunError> {
        submit(
            handle,
            NativeIds::environment(1, 1, 0, 0, 0, 0)?,
            KernelInput::AcceptRun(AcceptRun {
                session_id,
                lane_id,
                accepted,
            }),
        )
        .await?;
        submit_stage(
            handle,
            0,
            Stage::BeforeRun,
            ReducerStageOutcome::Continue,
            StageIds::continued(),
        )
        .await?;

        loop {
            let state = recover_state(Arc::clone(&store), session_id).await?;
            if state.cycle >= request.max_cycles {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "run exceeded max_cycles before producing a final response",
                ));
            }
            let messages = self.context_messages(&request.input, &state.messages)?;
            submit_stage(
                handle,
                state.cycle,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared { messages },
                StageIds::context(),
            )
            .await?;
            let state = recover_state(Arc::clone(&store), session_id).await?;
            let draft = model_draft(
                request.model.clone(),
                state
                    .current_turn
                    .as_ref()
                    .map(|turn| Arc::clone(&turn.context.messages))
                    .ok_or_else(|| AgentRunError::runtime_message("prepared context is missing"))?,
                self.tools.tools().map(|tool| tool.spec.clone()).collect(),
                request.settings.clone(),
                &profile,
            )?;
            let request_json =
                RawJson::parse(draft.canonical_bytes().map_err(AgentRunError::model)?)
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
            submit_stage(
                handle,
                state.cycle,
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: request_json,
                    component: None,
                    output_contract: model_output_contract(),
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: None,
                },
                StageIds::model_request(),
            )
            .await?;

            let after_model = wait_for_phase(
                handle,
                Arc::clone(&store),
                session_id,
                &[
                    RunPhase::AfterModel,
                    RunPhase::BeforeFinalize,
                    RunPhase::Failed,
                    RunPhase::Cancelled,
                ],
            )
            .await?;
            ensure_nonterminal_failure(&after_model)?;
            let next = if after_model.phase == Some(RunPhase::BeforeFinalize) {
                after_model
            } else {
                submit_stage(
                    handle,
                    after_model.cycle,
                    Stage::AfterModel,
                    ReducerStageOutcome::Continue,
                    StageIds::continued(),
                )
                .await?;
                wait_for_phase(
                    handle,
                    Arc::clone(&store),
                    session_id,
                    &[
                        RunPhase::BeforeFinalize,
                        RunPhase::AfterToolBatch,
                        RunPhase::Failed,
                        RunPhase::Cancelled,
                    ],
                )
                .await?
            };
            ensure_nonterminal_failure(&next)?;
            if next.phase == Some(RunPhase::AfterToolBatch) {
                submit_stage(
                    handle,
                    next.cycle,
                    Stage::AfterToolBatch,
                    ReducerStageOutcome::Continue,
                    StageIds::continued(),
                )
                .await?;
                continue;
            }

            submit_stage(
                handle,
                next.cycle,
                Stage::BeforeFinalize,
                ReducerStageOutcome::FinalizeAccepted,
                StageIds::finalize(),
            )
            .await?;
            let terminal = recover_state(store, session_id).await?;
            let TerminalState::Completed(completed) = terminal
                .terminal
                .as_ref()
                .ok_or_else(|| AgentRunError::runtime_message("terminal state is missing"))?
            else {
                return Err(AgentRunError::runtime_message(
                    "run did not complete successfully",
                ));
            };
            let message = terminal
                .messages
                .iter()
                .find(|message| message.id() == &completed.result_message_id)
                .cloned()
                .ok_or_else(|| AgentRunError::runtime_message("result message is missing"))?;
            return Ok(AgentRunOutput { locator, message });
        }
    }

    fn context_messages(
        &self,
        input: &str,
        committed: &[Message],
    ) -> Result<Arc<[Message]>, AgentRunError> {
        let now = NativeIds::now()?;
        let spec = self.resolved.spec().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires a bundle-resolved specification",
            )
        })?;
        let mut messages = Vec::with_capacity(spec.instructions.len() + committed.len() + 1);
        for instruction in spec.instructions.iter() {
            messages.push(text_message(
                NativeIds::generate::<MessageTag>()?,
                MessageRole::System,
                instruction.text(),
                now,
            )?);
        }
        messages.push(text_message(
            NativeIds::generate::<MessageTag>()?,
            MessageRole::User,
            input,
            now,
        )?);
        messages.extend_from_slice(committed);
        Ok(messages.into())
    }
}

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
    instructions: Vec<InstructionSpec>,
}

impl NativeAgentBuilder {
    fn new(
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
            instructions: Vec::new(),
        }
    }

    /// Add one ordered model instruction.
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
    #[must_use]
    pub fn toolset(mut self, component: ComponentRef, toolset: Arc<dyn Toolset>) -> Self {
        self.toolsets.push((component, toolset));
        self
    }

    /// Resolve, warm, lock, and construct one native [`Agent`].
    ///
    /// # Errors
    ///
    /// Fails closed on non-exact or duplicate components and all ordinary
    /// registry, bundle, warmup, and tool-catalog failures.
    pub async fn build(self) -> Result<Agent, AgentRunError> {
        validate_exact_component(&self.model.0)?;
        validate_exact_component(&self.store.0)?;
        for (component, _) in &self.toolsets {
            validate_exact_component(component)?;
        }
        let extension = NativeBuilderExtension {
            source: finstack_ai_kernel::ComponentId::parse("finstack.sdk.native-builder").map_err(
                |error| {
                    AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
                },
            )?,
            model: self.model.clone(),
            store: self.store.clone(),
            toolsets: self.toolsets.clone(),
        };
        let mut registrar = Registrar::new();
        registrar.register_extension(&extension).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        let mut registry = registrar.into_registry();
        let spec = AgentBuilder::new(
            self.agent_id.clone(),
            self.model.0.clone(),
            self.store.0.clone(),
        )
        .instructions(self.instructions)
        .toolsets(
            self.toolsets
                .iter()
                .map(|(component, _)| component.clone())
                .collect::<Vec<_>>(),
        )
        .build()
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        let mut catalog = BundleCatalog::default();
        catalog
            .install(BundleSpec {
                schema_version: BUNDLE_SCHEMA_VERSION,
                id: self.bundle_id.clone(),
                version: PREVIEW_ENGINE_VERSION,
                agents: Arc::from([spec]),
                capabilities: Arc::from([]),
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
        Agent::try_from_resolved(Arc::new(composed_agent))
    }
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

/// One bounded native run request.
#[derive(Debug, Clone)]
pub struct AgentRunRequest {
    /// Provider model name selected from the resolved model descriptor.
    pub model: ModelName,
    /// Plain-text user input.
    pub input: Arc<str>,
    /// Explicit validated security context captured durably at acceptance.
    pub security: RunSecurityContext,
    /// Canonical provider-specific settings.
    pub settings: ModelSettings,
    /// Operational deadline for the complete run.
    pub timeout: Duration,
    /// Maximum number of model cycles.
    pub max_cycles: u64,
}

impl AgentRunRequest {
    /// Construct a request with conservative preview defaults.
    ///
    /// # Errors
    ///
    /// Returns a configuration error if the input is empty or invalid JSON
    /// defaults cannot be constructed.
    pub fn try_new(
        model: ModelName,
        input: impl Into<Arc<str>>,
        security: RunSecurityContext,
    ) -> Result<Self, AgentRunError> {
        let request = Self {
            model,
            input: input.into(),
            security,
            settings: ModelSettings {
                values: RawJson::parse(b"{}")
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
            },
            timeout: Duration::from_secs(30),
            max_cycles: DEFAULT_MAX_CYCLES,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), AgentRunError> {
        if self.input.is_empty()
            || self.input.as_bytes().contains(&0)
            || self.timeout.is_zero()
            || self.max_cycles == 0
            || self.max_cycles > MAX_CONFIGURED_CYCLES
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "input, timeout, or max_cycles is invalid",
            ));
        }
        Ok(())
    }
}

/// Successful native run output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunOutput {
    /// Complete durable locator for the run.
    pub locator: OperationLocator,
    /// Final committed assistant message.
    pub message: Message,
}

impl AgentRunOutput {
    /// Concatenate final plain-text blocks in source order.
    #[must_use]
    pub fn text(&self) -> String {
        self.message
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text()),
                _ => None,
            })
            .collect()
    }
}

/// Stable native facade failure.
#[derive(Debug, Error)]
pub enum AgentRunError {
    /// Invalid or unsupported resolved/run configuration.
    #[error("{code}: {message}")]
    Configuration {
        /// Stable error code.
        code: &'static str,
        /// Non-secret explanation.
        message: String,
    },
    /// Runtime or provider failure.
    #[error("{code}: {message}")]
    Runtime {
        /// Stable error code.
        code: &'static str,
        /// Non-secret explanation.
        message: String,
    },
    /// Operational deadline elapsed.
    #[error("{code}: run exceeded {timeout:?}")]
    Timeout {
        /// Stable error code.
        code: &'static str,
        /// Configured deadline.
        timeout: Duration,
    },
}

impl AgentRunError {
    fn configuration(code: &'static str, message: impl Into<String>) -> Self {
        Self::Configuration {
            code,
            message: message.into(),
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "used directly as a Result::map_err adapter"
    )]
    fn runtime(error: RunHandleError) -> Self {
        Self::runtime_message(error.to_string())
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "used directly as a Result::map_err adapter"
    )]
    fn model(error: ModelError) -> Self {
        Self::runtime_message(error.to_string())
    }

    fn runtime_message(message: impl Into<String>) -> Self {
        Self::Runtime {
            code: AGENT_RUN_RUNTIME_FAILURE,
            message: message.into(),
        }
    }
}

impl From<IdGenerationError> for AgentRunError {
    fn from(error: IdGenerationError) -> Self {
        Self::runtime_message(error.to_string())
    }
}

#[derive(Debug)]
struct NativeIds;

impl NativeIds {
    fn now() -> Result<Timestamp, AgentRunError> {
        finstack_ai_runtime::Clock::now(&SystemClock).map_err(AgentRunError::from)
    }

    fn generate<T: finstack_ai_kernel::IdTag>() -> Result<finstack_ai_kernel::Id<T>, AgentRunError>
    {
        UuidV7Generator::new(SystemClock, OsRandomSource)
            .generate()
            .map_err(AgentRunError::from)
    }

    fn environment(
        records: usize,
        events: usize,
        effects: usize,
        turns: usize,
        model_requests: usize,
        messages: usize,
    ) -> Result<TransitionEnv, AgentRunError> {
        Ok(TransitionEnv {
            now: Self::now()?,
            ids: AllocatedIds::try_new(
                generate_many::<RecordTag>(records)?,
                generate_many::<EventTag>(events)?,
                generate_many::<finstack_ai_kernel::EffectTag>(effects)?,
                Vec::new(),
                generate_many::<MessageTag>(messages)?,
                generate_many::<TurnTag>(turns)?,
                generate_many::<ModelRequestTag>(model_requests)?,
                Vec::new(),
                Vec::new(),
                generate_many::<AppendBatchTag>(1)?,
                Vec::new(),
            )
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
        })
    }
}

fn generate_many<T: finstack_ai_kernel::IdTag>(
    count: usize,
) -> Result<Vec<finstack_ai_kernel::Id<T>>, AgentRunError> {
    (0..count).map(|_| NativeIds::generate()).collect()
}

#[derive(Debug, Clone, Copy)]
struct StageIds {
    records: usize,
    events: usize,
    effects: usize,
    turns: usize,
    model_requests: usize,
    messages: usize,
}

impl StageIds {
    const fn continued() -> Self {
        Self::new(1, 0, 0, 0, 0, 0)
    }

    const fn context() -> Self {
        Self::new(2, 0, 0, 1, 0, 0)
    }

    const fn model_request() -> Self {
        Self::new(2, 1, 1, 0, 1, 0)
    }

    const fn finalize() -> Self {
        Self::new(2, 1, 0, 0, 0, 0)
    }

    const fn new(
        records: usize,
        events: usize,
        effects: usize,
        turns: usize,
        model_requests: usize,
        messages: usize,
    ) -> Self {
        Self {
            records,
            events,
            effects,
            turns,
            model_requests,
            messages,
        }
    }
}

async fn submit_stage(
    handle: &RunHandle,
    cycle: u64,
    stage: Stage,
    outcome: ReducerStageOutcome,
    counts: StageIds,
) -> Result<(), AgentRunError> {
    submit(
        handle,
        NativeIds::environment(
            counts.records,
            counts.events,
            counts.effects,
            counts.turns,
            counts.model_requests,
            counts.messages,
        )?,
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor { cycle, stage },
            outcome,
        }),
    )
    .await
}

async fn submit(
    handle: &RunHandle,
    env: TransitionEnv,
    input: KernelInput,
) -> Result<(), AgentRunError> {
    let outcome = handle
        .submit(env, input)
        .await
        .map_err(AgentRunError::runtime)?;
    if let Some(fault) = outcome.fault {
        return Err(AgentRunError::runtime_message(format!(
            "runtime faulted after commit: {}",
            fault.code
        )));
    }
    Ok(())
}

async fn recover_state(
    store: Arc<dyn finstack_ai_runtime::JournalStore>,
    session_id: SessionId,
) -> Result<finstack_ai_kernel::KernelState, AgentRunError> {
    CommitCoordinator::recover(store, session_id)
        .await
        .map(|coordinator| coordinator.state().clone())
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))
}

async fn wait_for_phase(
    handle: &RunHandle,
    store: Arc<dyn finstack_ai_runtime::JournalStore>,
    session_id: SessionId,
    phases: &[RunPhase],
) -> Result<finstack_ai_kernel::KernelState, AgentRunError> {
    loop {
        if let finstack_ai_runtime::RunStatus::Faulted { code } = handle.status() {
            return Err(AgentRunError::runtime_message(format!(
                "runtime task faulted: {code}"
            )));
        }
        let state = recover_state(Arc::clone(&store), session_id).await?;
        if state.phase.is_some_and(|phase| phases.contains(&phase)) {
            return Ok(state);
        }
        finstack_ai_runtime::native_driver::yield_now().await;
    }
}

fn ensure_nonterminal_failure(
    state: &finstack_ai_kernel::KernelState,
) -> Result<(), AgentRunError> {
    match state.terminal.as_ref() {
        Some(TerminalState::Failed(failed)) => Err(AgentRunError::runtime_message(format!(
            "run failed: {}",
            failed.error.code
        ))),
        Some(TerminalState::Cancelled(_)) => {
            Err(AgentRunError::runtime_message("run was cancelled"))
        }
        _ => Ok(()),
    }
}

fn validate_model_name(model: &dyn Model, name: &ModelName) -> Result<(), AgentRunError> {
    let descriptor = model.descriptor();
    descriptor.validate().map_err(AgentRunError::model)?;
    if !descriptor.models.contains(name) {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "requested model is not present in the resolved descriptor",
        ));
    }
    Ok(())
}

fn model_draft(
    model: ModelName,
    messages: Arc<[Message]>,
    tools: Vec<finstack_ai_runtime::ToolSpec>,
    settings: ModelSettings,
    profile: &LockedModelContextProfile,
) -> Result<ModelRequestDraft, AgentRunError> {
    let margins = profile
        .profile
        .reserved_output_tokens
        .checked_add(profile.profile.provider_overhead_tokens)
        .ok_or_else(|| AgentRunError::runtime_message("model profile margin overflow"))?;
    let max_input_tokens = profile
        .profile
        .context_window_tokens
        .checked_sub(margins)
        .ok_or_else(|| AgentRunError::runtime_message("model profile margins exceed context"))?;
    Ok(ModelRequestDraft {
        model,
        messages,
        tools: tools.into(),
        output: finstack_ai_kernel::OutputSpec::PlainText,
        settings,
        limits: ModelRequestLimits {
            max_input_bytes: profile.profile.hard_input_bytes,
            max_input_tokens,
            max_output_tokens: profile.profile.reserved_output_tokens,
        },
    })
}

fn model_output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"{\"kind\":\"model_response\",\"schema_version\":1}"),
    }
}

fn text_message(
    id: MessageId,
    role: MessageRole,
    text: &str,
    now: Timestamp,
) -> Result<Message, AgentRunError> {
    Message::try_new(
        id,
        role,
        vec![ContentBlock::Text(TextBlock::try_new(text).map_err(
            |error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            },
        )?)],
        now,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })
}

fn run_task_config() -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: DEFAULT_QUEUE_CAPACITY,
        event_hub: EventHubConfig {
            source_capacity: DEFAULT_QUEUE_CAPACITY,
            max_subscribers: 4,
        },
        shutdown_deadline: Duration::from_secs(2),
    }
}

fn model_task_config() -> ModelTaskConfig {
    ModelTaskConfig {
        job_capacity: DEFAULT_QUEUE_CAPACITY,
        result_capacity: DEFAULT_QUEUE_CAPACITY,
        stream_limits: finstack_ai_runtime::ModelStreamLimits::default(),
        warmup_deadline: None,
        warmup_metadata: Metadata::empty(),
    }
}

fn tool_task_config() -> ToolTaskConfig {
    ToolTaskConfig {
        job_capacity: DEFAULT_QUEUE_CAPACITY,
        result_capacity: DEFAULT_QUEUE_CAPACITY,
        global_max_concurrency: 8,
        stream_limits: ToolStreamLimits::default(),
    }
}

struct ReadyModel(Arc<dyn Model>);

impl Model for ReadyModel {
    fn descriptor(&self) -> ModelDescriptor {
        self.0.descriptor()
    }

    fn capabilities(&self, model: &ModelName) -> ModelCapabilities {
        self.0.capabilities(model)
    }

    fn warmup(&self, _ctx: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
        Box::pin(async { Ok(()) })
    }

    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        self.0.estimate_input_tokens(model, canonical_request)
    }

    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        self.0.request(request)
    }

    fn reconcile(
        &self,
        ctx: ReconcileContext,
        effect: PendingModelEffect,
    ) -> PortFuture<Result<ModelReconcileResult, ModelError>> {
        self.0.reconcile(ctx, effect)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use finstack_ai_kernel::{AgentId, BundleId, ComponentId, ComponentRef, Usage, Version};
    use finstack_ai_runtime::{
        JournalStore, ModelContextProfile, ModelResponse, ModelStreamItem, ModelToolCall,
        TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, Toolset,
    };
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
    use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
    use finstack_ai_tools_calculator::CalculatorToolset;

    use super::*;
    use crate::{
        AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog,
        BundleDefaults, BundleResolver, BundleSpec, CompatibilityRequirements, Extension,
        ExtensionDescriptor, ReadyComponent, Registrar, RegistrationError, RegistrationMetadata,
        RuntimeServices,
    };

    const VERSION: Version = Version {
        major: 0,
        minor: 0,
        patch: 1,
    };

    struct PreviewExtension {
        model_id: ComponentId,
        store_id: ComponentId,
        model: Arc<ScriptedModel>,
        store: Arc<MemoryJournalStore>,
        calculator: Option<Arc<CalculatorToolset>>,
    }

    impl Extension for PreviewExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            ExtensionDescriptor::trusted_in_process(
                ComponentId::parse("test.extension.preview").expect("extension id"),
                VERSION,
            )
        }

        fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
            let model: Arc<dyn Model> = self.model.clone();
            registrar.model(
                RegistrationMetadata::new(self.model_id.clone(), VERSION),
                ReadyComponent::new(model),
            )?;
            if let Some(calculator) = &self.calculator {
                let toolset: Arc<dyn Toolset> = calculator.clone();
                registrar.toolset(
                    RegistrationMetadata::new(
                        ComponentId::parse("test.tools.calculator").expect("toolset id"),
                        VERSION,
                    ),
                    ReadyComponent::new(toolset),
                )?;
            }
            let store: Arc<dyn JournalStore> = self.store.clone();
            registrar.store(
                RegistrationMetadata::new(self.store_id.clone(), VERSION),
                ReadyComponent::new(store),
            )
        }
    }

    fn profile() -> finstack_ai_runtime::ModelContextProfile {
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("preview-1").expect("model name"),
            hard_input_bytes: 1_048_576,
            context_window_tokens: 1_048_576,
            max_output_tokens: 256,
            reserved_output_tokens: 256,
            provider_overhead_tokens: 32,
            estimator: TokenEstimatorRef {
                id: Arc::from("bytes-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        }
    }

    fn completed(text: &str) -> ScriptedModelPlan {
        ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                    finstack_ai_runtime::TextDelta {
                        text: Arc::from(text),
                    },
                ))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([ContentBlock::Text(
                        TextBlock::try_new(text).expect("assistant text"),
                    )]),
                    tool_calls: Arc::from([]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::empty(),
                    completion_id: Arc::from("preview-completion"),
                    continuation_state: None,
                }))),
            ],
        }
    }

    fn calculator_call() -> ScriptedModelPlan {
        let arguments =
            RawJson::parse(br#"{"operation":"add","operands":[2,3]}"#).expect("arguments");
        let response = ModelResponse {
            assistant_content: Arc::from([]),
            tool_calls: Arc::from([ModelToolCall {
                name: Arc::from("calculator"),
                arguments: arguments.clone(),
            }]),
            usage: Usage::empty(),
            provider_ids: ProviderIds::empty(),
            completion_id: Arc::from("preview-tool-completion"),
            continuation_state: None,
        };
        ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    name: Some(Arc::from("calculator")),
                    arguments_delta: Arc::from(arguments.as_str()),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))),
            ],
        }
    }

    fn security() -> RunSecurityContext {
        RunSecurityContext::try_new(
            "tenant-preview",
            finstack_ai_kernel::PrincipalRef::try_new(
                "preview-tests",
                "developer",
                Some("tenant-preview"),
            )
            .expect("principal"),
            "local",
            "test",
            "preview-policy-v1",
            "preview-decision-v1",
            None,
        )
        .expect("security")
    }

    #[tokio::test]
    async fn model_only_agent_completes_without_double_warmup() {
        let model_id = ComponentId::parse("test.model.preview").expect("model id");
        let store_id = ComponentId::parse("test.store.preview").expect("store id");
        let model = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![completed("preview ready")],
        ));
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 64,
                records_per_session: 512,
                snapshot_bytes: 4_096,
            })
            .expect("store"),
        );
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&PreviewExtension {
                model_id: model_id.clone(),
                store_id: store_id.clone(),
                model: Arc::clone(&model),
                store,
                calculator: None,
            })
            .expect("registration");
        let mut registry = registrar.into_registry();
        let agent_id = AgentId::parse("test.agent.preview").expect("agent id");
        let spec = AgentBuilder::new(
            agent_id.clone(),
            ComponentRef::new(model_id, Some(VERSION)),
            ComponentRef::new(store_id, Some(VERSION)),
        )
        .build()
        .expect("spec");
        let bundle_id = BundleId::parse("test.bundle.preview").expect("bundle id");
        let mut catalog = BundleCatalog::default();
        catalog
            .install(BundleSpec {
                schema_version: BUNDLE_SCHEMA_VERSION,
                id: bundle_id.clone(),
                version: VERSION,
                agents: Arc::from([spec]),
                capabilities: Arc::from([]),
                requirements: Arc::from([]),
                conflicts: Arc::from([]),
                defaults: BundleDefaults::default(),
                config_schema: None,
                compatibility: CompatibilityRequirements::default(),
            })
            .expect("bundle");
        let bundle_resolver = BundleResolver::new(
            &catalog,
            VERSION,
            BTreeSet::new(),
            RuntimeServices::default(),
        );
        let composed_agent = bundle_resolver
            .resolve_agent(
                &mut registry,
                &bundle_id,
                &agent_id,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .expect("resolved");
        assert_eq!(model.warmup_count(), 1);
        let agent = Agent::try_from_resolved(Arc::new(composed_agent)).expect("Agent");
        let output = agent
            .run(
                AgentRunRequest::try_new(
                    ModelName::try_new("preview-1").expect("model name"),
                    "Say hello",
                    security(),
                )
                .expect("request"),
            )
            .await
            .expect("run");
        assert_eq!(output.text(), "preview ready");
        assert_eq!(model.request_count(), 1);
        assert_eq!(model.warmup_count(), 1);
    }

    #[tokio::test]
    async fn tool_loop_executes_read_only_calculator_then_completes() {
        let model_id = ComponentId::parse("test.model.preview-tool").expect("model id");
        let store_id = ComponentId::parse("test.store.preview-tool").expect("store id");
        let toolset_id = ComponentId::parse("test.tools.calculator").expect("toolset id");
        let model = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![calculator_call(), completed("five")],
        ));
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 64,
                records_per_session: 512,
                snapshot_bytes: 4_096,
            })
            .expect("store"),
        );
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&PreviewExtension {
                model_id: model_id.clone(),
                store_id: store_id.clone(),
                model: Arc::clone(&model),
                store,
                calculator: Some(Arc::new(CalculatorToolset::try_new().expect("calculator"))),
            })
            .expect("registration");
        let mut registry = registrar.into_registry();
        let agent_id = AgentId::parse("test.agent.preview-tool").expect("agent id");
        let spec = AgentBuilder::new(
            agent_id.clone(),
            ComponentRef::new(model_id, Some(VERSION)),
            ComponentRef::new(store_id, Some(VERSION)),
        )
        .toolsets(Arc::from([ComponentRef::new(toolset_id, Some(VERSION))]))
        .build()
        .expect("spec");
        let bundle_id = BundleId::parse("test.bundle.preview-tool").expect("bundle id");
        let mut catalog = BundleCatalog::default();
        catalog
            .install(BundleSpec {
                schema_version: BUNDLE_SCHEMA_VERSION,
                id: bundle_id.clone(),
                version: VERSION,
                agents: Arc::from([spec]),
                capabilities: Arc::from([]),
                requirements: Arc::from([]),
                conflicts: Arc::from([]),
                defaults: BundleDefaults::default(),
                config_schema: None,
                compatibility: CompatibilityRequirements::default(),
            })
            .expect("bundle");
        let bundle_resolver = BundleResolver::new(
            &catalog,
            VERSION,
            BTreeSet::new(),
            RuntimeServices::default(),
        );
        let composed_agent = bundle_resolver
            .resolve_agent(
                &mut registry,
                &bundle_id,
                &agent_id,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .expect("resolved");
        let output = Agent::try_from_resolved(Arc::new(composed_agent))
            .expect("Agent")
            .run(
                AgentRunRequest::try_new(
                    ModelName::try_new("preview-1").expect("model name"),
                    "What is two plus three?",
                    security(),
                )
                .expect("request"),
            )
            .await
            .expect("run");
        assert_eq!(output.text(), "five");
        assert_eq!(model.request_count(), 2);
        assert_eq!(model.warmup_count(), 1);
    }
}
