//! Native developer-preview execution facade.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::{
    AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleDefaults,
    BundleResolver, BundleSpec, CapabilityActivation, CapabilityRef, CapabilitySpec,
    CompatibilityRequirements, Extension, ExtensionDescriptor, InstructionSpec, ReadyComponent,
    Registrar, RegistrationError, RegistrationMetadata, Registry, ResolvedAgent, RuntimeServices,
};
use finstack_ai_kernel::{
    AcceptRun, ActiveCapability, AgentId, AllocatedIds, AppendBatchTag, AuthorizationEvidence,
    BudgetPropagation, BundleId, CancelRequested, CancellationInitiator, CancellationPropagation,
    CancellationRequestTag, CapabilitiesActivated, CapabilityActivationSource, CapabilityId,
    ComponentRef, ContentBlock, ConversationEntry, DeadlinePropagation, Digest,
    EffectOutputContract, EffectOutputKind, EventTag, JsonSchemaDraft, KernelInput, LaneCreated,
    LaneId, LaneMoved, LaneTag, Message, MessageId, MessageRole, MessageTag, Metadata,
    MiddlewareRef, ModelRequestTag, OperationLocator, OutputConfiguration, OutputEndStrategy,
    OutputSpec, OutputValidated, PrincipalPropagation, ProviderIds, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RawJson, RecordBody, RecordDraft, RecordTag, ReducerStageOutcome,
    RetryClassification, RetryDirective, RetrySafety, RunAccepted, RunPhase, RunPropagationPolicy,
    RunRelation, RunSecurityContext, RunTag, SchemaRef, Sensitivity, SessionCreated, SessionId,
    SessionTag, Stage, StageCursor, StageSettled, StructuredResultSource, TerminalState, TextBlock,
    Timestamp, TransitionEnv, TurnTag, Version,
};
use finstack_ai_runtime::{
    CommitCoordinator, ContextProvider, EventBatch, EventBatchConfig, EventFilter, EventHubConfig,
    EventLagPolicy, EventSubscription, EventSubscriptionConfig, IdGenerationError,
    JsonSchemaToolValidatorCompiler, LaneAppendIds, LoadRequest, LockedModelContextProfile,
    Middleware, Model, ModelCapabilities, ModelContextProfileOverride, ModelDescriptor, ModelError,
    ModelEventStream, ModelName, ModelReconcileResult, ModelRequest, ModelRequestDraft,
    ModelRequestLimits, ModelSettings, ModelTaskConfig, ModelTokenEstimate, ModelWarmupContext,
    Observer, PendingModelEffect, PortFuture, ProgressCoalescing, ReconcileContext,
    ResolvedToolCatalog, RunEvent, RunHandle, RunHandleError, RunTaskConfig, RunTaskOwner,
    SessionError, SessionRuntime, SideEffectClass, StructuredOutputCapability, ToolExecutionPolicy,
    ToolFailurePolicy, ToolPolicyDecision, ToolStreamLimits, ToolTaskConfig, ToolValidator,
    ToolValidatorCompiler, Toolset, ToolsetRegistration, UuidV7Generator,
    resolve_model_context_profile,
};
use thiserror::Error;

#[cfg(feature = "native-tokio")]
use finstack_ai_kernel::{InteractionRequest, InteractionResolution, InteractionSettled};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::InteractionRouter;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver as driver;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver::{
    InstalledClock as AgentClock, InstalledRandom as AgentRandom,
};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::native_driver as driver;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::{OsRandomSource as AgentRandom, SystemClock as AgentClock};

/// Invalid public run configuration.
pub const AGENT_RUN_INVALID_CONFIGURATION: &str = "agent_run_invalid_configuration";
/// The resolved plan contains a stage not supported by the native preview driver.
pub const AGENT_RUN_UNSUPPORTED_PLAN: &str = "agent_run_unsupported_plan";
/// The commit-before-effect runtime failed.
pub const AGENT_RUN_RUNTIME_FAILURE: &str = "agent_run_runtime_failure";
/// The operational run deadline elapsed.
pub const AGENT_RUN_TIMEOUT: &str = "agent_run_timeout";
/// The run reached its durable cancelled terminal state.
pub const AGENT_RUN_CANCELLED: &str = "agent_run_cancelled";

const DEFAULT_QUEUE_CAPACITY: usize = 32;
const DEFAULT_MAX_CYCLES: u64 = 16;
const MAX_CONFIGURED_CYCLES: u64 = 1_024;
const DEFAULT_MAX_OUTPUT_RETRIES: u32 = 1;
const MAX_CONFIGURED_OUTPUT_RETRIES: u32 = 1_024;
const DEFAULT_EVENT_BATCH_COUNT: usize = 32;
const DEFAULT_EVENT_BATCH_BYTES: usize = 64 * 1_024;
const DEFAULT_EVENT_BATCH_INTERVAL: Duration = Duration::from_millis(10);
const MAX_COMPACT_CATALOG_BYTES: usize = 8 * 1_024;

/// One compact model-visible capability catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCatalogEntry {
    id: CapabilityId,
    description: Arc<str>,
}

impl CapabilityCatalogEntry {
    /// Borrow the stable capability identity.
    #[must_use]
    pub const fn id(&self) -> &CapabilityId {
        &self.id
    }

    /// Borrow the compact non-secret description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Clone)]
struct ModelCapabilityVariant {
    entry: CapabilityCatalogEntry,
    agent: Agent,
}

/// Immutable native facade over one fully resolved agent.
#[derive(Clone)]
pub struct Agent {
    resolved: Arc<ResolvedAgent>,
    tools: Arc<ResolvedToolCatalog>,
    structured_output: Option<StructuredOutputConfig>,
    model_capabilities: Arc<[ModelCapabilityVariant]>,
}

#[derive(Clone)]
struct StructuredOutputConfig {
    schema_ref: SchemaRef,
    validator: Arc<dyn ToolValidator>,
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
    /// The developer preview supports direct model, Toolset, context-provider,
    /// middleware, and observer handles. Observer failures are isolated from
    /// run semantics.
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
            structured_output: None,
            model_capabilities: Arc::from([]),
        })
    }

    /// Return an agent configured for one compile-once Draft 2020-12 output schema.
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
        let output = self.structured_output.clone();
        let mut model_capabilities = self.model_capabilities.to_vec();
        for variant in &mut model_capabilities {
            variant.agent.structured_output.clone_from(&output);
        }
        self.model_capabilities = model_capabilities.into();
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
            .map(|variant| format!("{}: {}", variant.entry.id, variant.entry.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Start one bounded native run and return its detached control handle.
    ///
    /// The caller must already be inside the selected runtime driver. Dropping the returned
    /// handle detaches frontend observation; it does not cancel the durable run.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration or runtime error before the background
    /// run task is accepted.
    pub fn start(&self, request: AgentRunRequest) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_input(&request.input).clone();
        let prepared = selected.prepare(request)?;
        let locator = prepared.locator.clone();
        let store = Arc::clone(&prepared.store);
        let cancellation_initiator = prepared.cancellation_initiator()?;
        let inner = Arc::new(AgentRunInner {
            locator,
            store,
            cancellation_initiator,
            handle: Mutex::new(None),
            handle_ready: driver::Signal::new(),
            result: Mutex::new(None),
            result_ready: driver::Signal::new(),
            events: Mutex::new(EventStreamState::Waiting),
            cancellation: Mutex::new(CancellationState::default()),
            cancellation_ready: driver::Signal::new(),
        });
        let execution = Arc::downgrade(&inner);
        let agent = selected;
        driver::spawn(Box::pin(async move {
            let result = Box::pin(agent.execute_started(prepared, &execution)).await;
            publish_result(&execution, result);
        }))
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        Ok(AgentRun { inner })
    }

    /// Start a new root run on an existing idle lane.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane or configuration/runtime failure.
    pub fn start_on_lane(
        &self,
        lane: &crate::Lane,
        request: AgentRunRequest,
    ) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_input(&request.input).clone();
        let prepared = selected.prepare_on(request, lane)?;
        let locator = prepared.locator.clone();
        let store = Arc::clone(&prepared.store);
        let cancellation_initiator = prepared.cancellation_initiator()?;
        let inner = Arc::new(AgentRunInner {
            locator,
            store,
            cancellation_initiator,
            handle: Mutex::new(None),
            handle_ready: driver::Signal::new(),
            result: Mutex::new(None),
            result_ready: driver::Signal::new(),
            events: Mutex::new(EventStreamState::Waiting),
            cancellation: Mutex::new(CancellationState::default()),
            cancellation_ready: driver::Signal::new(),
        });
        let execution = Arc::downgrade(&inner);
        let agent = selected;
        driver::spawn(Box::pin(async move {
            let result = Box::pin(agent.execute_started(prepared, &execution)).await;
            publish_result(&execution, result);
        }))
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        Ok(AgentRun { inner })
    }

    /// Borrow the resolved journal store.
    #[must_use]
    pub fn journal_store(&self) -> Arc<dyn finstack_ai_runtime::JournalStore> {
        Arc::clone(self.resolved.run_plan().store().handle())
    }

    fn select_for_input(&self, input: &str) -> &Self {
        let input_tokens = activation_tokens(input);
        self.model_capabilities
            .iter()
            .filter_map(|variant| {
                let score = activation_tokens(variant.entry.id.as_str())
                    .union(&activation_tokens(&variant.entry.description))
                    .filter(|token| input_tokens.contains(*token))
                    .count();
                (score > 0).then_some((score, variant))
            })
            .max_by(|(left_score, left), (right_score, right)| {
                left_score
                    .cmp(right_score)
                    .then_with(|| right.entry.id.cmp(&left.entry.id))
            })
            .map_or(self, |(_, variant)| &variant.agent)
    }

    /// Execute one bounded native run through the commit-before-effect runtime.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration, runtime, cancellation, or timeout error.
    pub async fn run(&self, request: AgentRunRequest) -> Result<AgentRunOutput, AgentRunError> {
        self.start(request)?.result().await
    }

    fn prepare(&self, request: AgentRunRequest) -> Result<PreparedAgentRun, AgentRunError> {
        request.validate()?;
        let plan = self.resolved.run_plan();
        let model = Arc::clone(plan.model().handle());
        validate_model_name(model.as_ref(), &request.model)?;
        let capabilities = model.capabilities(&request.model);
        if self.structured_output.is_some()
            && capabilities.structured_output == StructuredOutputCapability::Unsupported
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "selected model does not support structured output",
            ));
        }
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
        let mut limits = spec.limits.clone();
        if self.structured_output.is_some() {
            limits.max_retries = Some(
                limits
                    .max_retries
                    .map_or(request.max_output_retries, |limit| {
                        limit.min(request.max_output_retries)
                    }),
            );
        }
        let accepted = RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?,
            request.security.clone(),
            None,
            limits,
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

        Ok(PreparedAgentRun {
            model,
            profile,
            store,
            session_id,
            lane_id,
            accepted,
            request,
            locator,
            bootstrap: true,
            session: None,
        })
    }

    fn prepare_on(
        &self,
        request: AgentRunRequest,
        lane: &crate::Lane,
    ) -> Result<PreparedAgentRun, AgentRunError> {
        let mut prepared = self.prepare(request)?;
        prepared.session_id = lane.session().session_id();
        prepared.lane_id = lane.lane_id();
        prepared.locator = OperationLocator::try_new(
            prepared.request.security.tenant_scope(),
            prepared.session_id,
            prepared.lane_id,
            prepared.accepted.run_id(),
        )
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        prepared.bootstrap = false;
        prepared.session = Some(lane.session().clone());
        Ok(prepared)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "bootstrap and sibling-lane start share one acquire/release path"
    )]
    async fn execute_started(
        &self,
        prepared: PreparedAgentRun,
        execution: &Weak<AgentRunInner>,
    ) -> Result<AgentRunOutput, AgentRunError> {
        let ready_model: Arc<dyn Model> = Arc::new(ReadyModel(Arc::clone(&prepared.model)));
        let mut coordinator = CommitCoordinator::new(Arc::clone(&prepared.store));
        let mut acquired_lane = None;
        if prepared.bootstrap {
            if let Err(error) = bootstrap_main_lane(
                &mut coordinator,
                prepared.session_id,
                prepared.lane_id,
                &prepared.request.input,
            )
            .await
            {
                publish_start_failure(execution, &error);
                return Err(error);
            }
            if coordinator
                .session()
                .active_on_lane(prepared.lane_id)
                .is_some()
            {
                let error = AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "main lane already has an active operation",
                );
                publish_start_failure(execution, &error);
                return Err(error);
            }
        } else if let Some(session) = &prepared.session {
            let runtime = match session.runtime().await {
                Ok(runtime) => runtime,
                Err(error) => {
                    let error = session_error(&error);
                    publish_start_failure(execution, &error);
                    return Err(error);
                }
            };
            if let Err(error) =
                append_lane_input(&runtime, prepared.lane_id, &prepared.request.input).await
            {
                publish_start_failure(execution, &error);
                return Err(error);
            }
            if let Err(error) = runtime
                .try_acquire_run(prepared.lane_id, prepared.accepted.run_id())
                .map_err(|error| session_error(&error))
            {
                publish_start_failure(execution, &error);
                return Err(error);
            }
            acquired_lane = Some((Arc::clone(&runtime), prepared.lane_id));
            coordinator = match runtime
                .coordinator_for_run(Some(prepared.accepted.run_id()))
                .await
            {
                Ok(coordinator) => coordinator,
                Err(error) => {
                    runtime.release(prepared.lane_id);
                    let error = session_error(&error);
                    publish_start_failure(execution, &error);
                    return Err(error);
                }
            };
        }
        let observer_count = self.resolved.run_plan().observers().len();
        let owner = if self.tools.is_empty() {
            Box::pin(RunTaskOwner::spawn_with_model(
                coordinator,
                run_task_config(observer_count),
                model_task_config(),
                ready_model,
                prepared.profile.clone(),
                AgentClock,
                AgentRandom,
            ))
            .await
            .map_err(AgentRunError::runtime)
        } else {
            Box::pin(RunTaskOwner::spawn_with_model_and_tools(
                coordinator,
                run_task_config(observer_count),
                model_task_config(),
                tool_task_config(),
                ready_model,
                prepared.profile.clone(),
                Arc::clone(&self.tools),
                AgentClock,
                AgentRandom,
            ))
            .await
            .map_err(AgentRunError::runtime)
        };
        let mut owner = match owner {
            Ok(owner) => owner,
            Err(error) => {
                if let Some((runtime, lane_id)) = &acquired_lane {
                    runtime.release(*lane_id);
                }
                publish_start_failure(execution, &error);
                return Err(error);
            }
        };
        let handle = owner.handle();
        let subscription = match handle.subscribe_events(default_event_subscription()).await {
            Ok(subscription) => subscription,
            Err(error) => {
                let error = AgentRunError::runtime_message(error.to_string());
                let _shutdown = owner.shutdown().await;
                if let Some((runtime, lane_id)) = &acquired_lane {
                    runtime.release(*lane_id);
                }
                publish_start_failure(execution, &error);
                return Err(error);
            }
        };
        attach_plan_observers(&handle, self.resolved.run_plan().observers());
        publish_started(execution, handle.clone(), subscription);
        let timeout = prepared.request.timeout;
        let result = driver::timeout(
            timeout,
            Box::pin(self.drive(
                &handle,
                prepared.store,
                prepared.session_id,
                prepared.lane_id,
                prepared.accepted,
                prepared.request,
                prepared.profile,
                prepared.locator,
            )),
        )
        .await;
        let _shutdown = owner.shutdown().await;
        if let Some((runtime, lane_id)) = acquired_lane {
            runtime.release(lane_id);
        }
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
        let lock = self.resolved.lock().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires an exact resolved lock",
            )
        })?;
        let active = lock
            .capabilities
            .iter()
            .filter(|capability| capability.active)
            .map(|capability| ActiveCapability {
                capability_id: capability.id.clone(),
                source: match capability.activation {
                    CapabilityActivation::Always => CapabilityActivationSource::Always,
                    CapabilityActivation::Application => CapabilityActivationSource::Application,
                    CapabilityActivation::Model => CapabilityActivationSource::Model,
                    CapabilityActivation::Disabled => {
                        unreachable!("a disabled capability cannot be active in a validated lock")
                    }
                },
            })
            .collect::<Vec<_>>();
        if !active.is_empty() {
            submit(
                handle,
                NativeIds::environment(1, 0, 0, 0, 0, 0)?,
                KernelInput::CapabilitiesActivated(CapabilitiesActivated {
                    prior_plan_digest: None,
                    resolved_plan_digest: lock.fingerprint().map_err(|error| {
                        AgentRunError::configuration(
                            AGENT_RUN_INVALID_CONFIGURATION,
                            error.to_string(),
                        )
                    })?,
                    active: active.into(),
                }),
            )
            .await?;
        }
        if let Some(output) = &self.structured_output {
            submit(
                handle,
                NativeIds::environment(1, 0, 0, 0, 0, 0)?,
                KernelInput::ConfigureOutput(OutputConfiguration {
                    output: OutputSpec::JsonSchema {
                        schema: output.schema_ref.clone(),
                    },
                    end_strategy: OutputEndStrategy::Early,
                }),
            )
            .await?;
        }
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
                self.structured_output
                    .as_ref()
                    .map_or(OutputSpec::PlainText, |output| OutputSpec::JsonSchema {
                        schema: output.schema_ref.clone(),
                    }),
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

            let mut after_model = wait_for_phase(
                handle,
                Arc::clone(&store),
                session_id,
                &[
                    RunPhase::AfterModel,
                    RunPhase::AfterToolBatch,
                    RunPhase::BeforeFinalize,
                    RunPhase::Failed,
                    RunPhase::Cancelled,
                ],
            )
            .await?;
            ensure_nonterminal_failure(&after_model)?;
            if after_model.phase == Some(RunPhase::AfterToolBatch) {
                submit_stage(
                    handle,
                    after_model.cycle,
                    Stage::AfterToolBatch,
                    ReducerStageOutcome::Continue,
                    StageIds::continued(),
                )
                .await?;
                continue;
            }
            if let Some(output) = &self.structured_output
                && after_model.phase == Some(RunPhase::AfterModel)
            {
                let (message_id, candidate, source) = structured_candidate(&after_model)
                    .ok_or_else(|| {
                        AgentRunError::runtime_message(
                            "structured model response did not contain a JSON candidate",
                        )
                    })?;
                submit(
                    handle,
                    NativeIds::environment(1, 0, 0, 0, 0, 0)?,
                    KernelInput::OutputValidated(OutputValidated {
                        message_id,
                        schema: output.schema_ref.clone(),
                        candidate: candidate.clone(),
                        source,
                        outcome: output.validator.validate(&candidate),
                    }),
                )
                .await?;
                after_model = recover_state(Arc::clone(&store), session_id).await?;
            }
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

            if next
                .validation_failure
                .as_ref()
                .is_some_and(|failure| failure.error.retryable)
            {
                submit_stage(
                    handle,
                    next.cycle,
                    Stage::BeforeFinalize,
                    ReducerStageOutcome::Retry(
                        RetryDirective::try_new(
                            RetryClassification::Validation,
                            finstack_ai_kernel::Duration::from_millis(1),
                            "native-structured-output-v1",
                        )
                        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
                    ),
                    StageIds::retry(),
                )
                .await?;
                let retry = wait_for_phase(
                    handle,
                    Arc::clone(&store),
                    session_id,
                    &[
                        RunPhase::PreparingContext,
                        RunPhase::Failed,
                        RunPhase::Cancelled,
                    ],
                )
                .await?;
                ensure_nonterminal_failure(&retry)?;
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
            let terminal = recover_state(Arc::clone(&store), session_id).await?;
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
            let loaded = store
                .load(LoadRequest { session_id })
                .await
                .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
            let record_kinds = loaded
                .committed_batches
                .iter()
                .flat_map(|batch| batch.records.iter())
                .map(|record| Arc::from(record.body().kind_name()))
                .collect::<Vec<_>>();
            return Ok(AgentRunOutput {
                locator,
                message,
                retry_attempts: terminal.retry.attempts,
                active_capabilities: terminal.active_capabilities.clone(),
                record_kinds: record_kinds.into(),
            });
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

struct PreparedAgentRun {
    model: Arc<dyn Model>,
    profile: LockedModelContextProfile,
    store: Arc<dyn finstack_ai_runtime::JournalStore>,
    session_id: SessionId,
    lane_id: LaneId,
    accepted: RunAccepted,
    request: AgentRunRequest,
    locator: OperationLocator,
    bootstrap: bool,
    session: Option<crate::Session>,
}

impl PreparedAgentRun {
    fn cancellation_initiator(&self) -> Result<CancellationInitiator, AgentRunError> {
        let security = &self.request.security;
        let authorization = AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        Ok(CancellationInitiator::Principal {
            principal: security.principal().clone(),
            authorization,
        })
    }
}

struct AgentRunInner {
    locator: OperationLocator,
    store: Arc<dyn finstack_ai_runtime::JournalStore>,
    cancellation_initiator: CancellationInitiator,
    handle: Mutex<Option<Result<RunHandle, AgentRunError>>>,
    handle_ready: driver::Signal,
    result: Mutex<Option<Result<AgentRunOutput, AgentRunError>>>,
    result_ready: driver::Signal,
    events: Mutex<EventStreamState>,
    cancellation: Mutex<CancellationState>,
    cancellation_ready: driver::Signal,
}

enum EventStreamState {
    Waiting,
    Active(EventSubscription),
    Busy,
    CloseRequested,
    Closed,
    StartupFailed(AgentRunError),
}

struct EventConsumerGuard {
    inner: Arc<AgentRunInner>,
    subscription: Option<EventSubscription>,
}

impl EventConsumerGuard {
    fn subscription_mut(&mut self) -> &mut EventSubscription {
        self.subscription
            .as_mut()
            .expect("event consumer guard owns its subscription")
    }

    fn finish(mut self, batch: Option<EventBatch>) -> Result<Option<EventBatch>, AgentRunError> {
        let mut state = self
            .inner
            .events
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run event lock is poisoned"))?;
        let mut subscription = self
            .subscription
            .take()
            .expect("event consumer guard owns its subscription");
        match &*state {
            EventStreamState::CloseRequested | EventStreamState::Closed => {
                subscription.close();
                *state = EventStreamState::Closed;
                Ok(None)
            }
            EventStreamState::Busy => {
                if batch.is_some() {
                    *state = EventStreamState::Active(subscription);
                } else {
                    *state = EventStreamState::Closed;
                }
                Ok(batch)
            }
            _ => {
                subscription.close();
                *state = EventStreamState::Closed;
                Err(AgentRunError::runtime_message(
                    "run event consumer state is inconsistent",
                ))
            }
        }
    }
}

impl Drop for EventConsumerGuard {
    fn drop(&mut self) {
        let Some(mut subscription) = self.subscription.take() else {
            return;
        };
        let Ok(mut state) = self.inner.events.lock() else {
            subscription.close();
            return;
        };
        if matches!(&*state, EventStreamState::Busy) {
            *state = EventStreamState::Active(subscription);
        } else {
            subscription.close();
            *state = EventStreamState::Closed;
        }
    }
}

#[derive(Default)]
struct CancellationState {
    started: bool,
    result: Option<Result<(), AgentRunError>>,
}

/// Cloneable control and observation handle for one Rust-owned native run.
///
/// Dropping every clone detaches local observation but does not cancel the
/// durable run. Call [`AgentRun::cancel`] for explicit durable cancellation.
#[derive(Clone)]
pub struct AgentRun {
    inner: Arc<AgentRunInner>,
}

impl AgentRun {
    /// Borrow the immutable durable locator allocated before run execution.
    #[must_use]
    pub fn locator(&self) -> &OperationLocator {
        &self.inner.locator
    }

    /// Live session handle for this run. Does not respawn parked runs.
    #[must_use]
    pub fn session(&self) -> crate::Session {
        crate::Session::pending(
            Arc::clone(&self.inner.store),
            self.inner.locator.session_id,
            Arc::clone(&self.inner.locator.tenant_scope),
        )
    }

    /// List the outstanding typed interaction for this run (0 or 1).
    ///
    /// Native-only. Browser WASM list/resolve remains PR-048.
    ///
    /// The owned handle treats an unpublished or not-yet-accepted journal as
    /// empty. After accept, listing goes through [`InteractionRouter`].
    ///
    /// # Errors
    ///
    /// Returns a stable runtime failure when the authenticated locator cannot
    /// be listed through [`InteractionRouter`].
    #[cfg(feature = "native-tokio")]
    pub async fn list_interactions(&self) -> Result<Vec<InteractionRequest>, AgentRunError> {
        let CancellationInitiator::Principal {
            principal,
            authorization,
        } = &self.inner.cancellation_initiator
        else {
            return Err(AgentRunError::runtime_message(
                "interaction list requires a principal-authored run",
            ));
        };
        let Ok(recovered) = CommitCoordinator::recover(
            Arc::clone(&self.inner.store),
            self.inner.locator.session_id,
        )
        .await
        else {
            return Ok(Vec::new());
        };
        if recovered.state().accepted.is_none() {
            return Ok(Vec::new());
        }
        let router = InteractionRouter::trusted(Arc::clone(&self.inner.store))
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        router
            .list(
                &self.inner.locator,
                principal,
                authorization,
                NativeIds::now()?,
            )
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))
    }

    /// Resolve the outstanding interaction through the live run handle.
    ///
    /// Native-only. Browser WASM list/resolve remains PR-048.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime failure when the handle is unavailable or the
    /// settlement is rejected.
    #[cfg(feature = "native-tokio")]
    pub async fn resolve_interaction(
        &self,
        resolution: InteractionResolution,
    ) -> Result<(), AgentRunError> {
        let handle = self.runtime_handle().await?;
        submit(
            &handle,
            NativeIds::interaction_resolve_environment()?,
            KernelInput::InteractionSettled(InteractionSettled::Resolved(resolution)),
        )
        .await
    }

    /// Wait for the final committed result.
    ///
    /// The result is retained, so multiple callers observe the same terminal
    /// value without rerunning any model or tool.
    ///
    /// # Errors
    ///
    /// Returns the stable configuration, runtime, cancellation, or timeout
    /// error that settled the run.
    pub async fn result(&self) -> Result<AgentRunOutput, AgentRunError> {
        loop {
            let notified = self.inner.result_ready.notified();
            let result = self
                .inner
                .result
                .lock()
                .map_err(|_| AgentRunError::runtime_message("run result lock is poisoned"))?
                .clone();
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }

    /// Submit one idempotent durable cancellation request.
    ///
    /// Cancellation is explicit and independent of Python/Rust handle drops.
    /// Repeated calls share the first submission outcome.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime failure if run startup or cancellation commit
    /// fails.
    pub async fn cancel(&self) -> Result<(), AgentRunError> {
        let should_start = {
            let mut cancellation =
                self.inner.cancellation.lock().map_err(|_| {
                    AgentRunError::runtime_message("run cancellation lock is poisoned")
                })?;
            if let Some(result) = cancellation.result.clone() {
                return result;
            }
            if cancellation.started {
                false
            } else {
                cancellation.started = true;
                true
            }
        };
        if should_start {
            let run = self.clone();
            if let Err(error) = driver::spawn(Box::pin(async move {
                let result = run.submit_cancellation().await;
                if let Ok(mut cancellation) = run.inner.cancellation.lock() {
                    cancellation.result = Some(result);
                }
                run.inner.cancellation_ready.notify_waiters();
            })) {
                let error = AgentRunError::runtime_message(error.to_string());
                let mut cancellation = self.inner.cancellation.lock().map_err(|_| {
                    AgentRunError::runtime_message("run cancellation lock is poisoned")
                })?;
                cancellation.result = Some(Err(error));
                self.inner.cancellation_ready.notify_waiters();
            }
        }
        loop {
            let notified = self.inner.cancellation_ready.notified();
            let result = self
                .inner
                .cancellation
                .lock()
                .map_err(|_| AgentRunError::runtime_message("run cancellation lock is poisoned"))?
                .result
                .clone();
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }

    /// Receive the next bounded transport batch in source order.
    ///
    /// Exactly one consumer may advance this subscription. `None` means the
    /// event hub closed after terminal settlement or explicit event closure.
    ///
    /// # Errors
    ///
    /// Returns the stable startup failure if the run could not publish its
    /// event subscription.
    pub async fn next_event_batch(&self) -> Result<Option<EventBatch>, AgentRunError> {
        loop {
            let subscription = {
                let mut state =
                    self.inner.events.lock().map_err(|_| {
                        AgentRunError::runtime_message("run event lock is poisoned")
                    })?;
                match &*state {
                    EventStreamState::Waiting
                    | EventStreamState::Busy
                    | EventStreamState::CloseRequested => None,
                    EventStreamState::Closed => return Ok(None),
                    EventStreamState::StartupFailed(error) => return Err(error.clone()),
                    EventStreamState::Active(_) => {
                        let EventStreamState::Active(subscription) =
                            std::mem::replace(&mut *state, EventStreamState::Busy)
                        else {
                            unreachable!("active event state changed while locked")
                        };
                        Some(subscription)
                    }
                }
            };
            let Some(subscription) = subscription else {
                driver::yield_now().await;
                continue;
            };
            let mut guard = EventConsumerGuard {
                inner: Arc::clone(&self.inner),
                subscription: Some(subscription),
            };
            let batch = guard.subscription_mut().next_batch().await;
            return guard.finish(batch);
        }
    }

    /// Close frontend event delivery without cancelling the owning run.
    pub fn close_events(&self) {
        let Ok(mut state) = self.inner.events.lock() else {
            return;
        };
        match &mut *state {
            EventStreamState::Active(subscription) => {
                subscription.close();
                *state = EventStreamState::Closed;
            }
            EventStreamState::Busy => *state = EventStreamState::CloseRequested,
            _ => *state = EventStreamState::Closed,
        }
    }

    async fn runtime_handle(&self) -> Result<RunHandle, AgentRunError> {
        loop {
            let notified = self.inner.handle_ready.notified();
            let result = self
                .inner
                .handle
                .lock()
                .map_err(|_| AgentRunError::runtime_message("run handle lock is poisoned"))?
                .clone();
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }

    async fn submit_cancellation(&self) -> Result<(), AgentRunError> {
        if self
            .inner
            .result
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run result lock is poisoned"))?
            .is_some()
        {
            return Ok(());
        }
        let handle = self.runtime_handle().await?;
        submit(
            &handle,
            NativeIds::cancellation_environment()?,
            KernelInput::CancelRequested(CancelRequested {
                initiator: self.inner.cancellation_initiator.clone(),
                reason: Some(Arc::from("frontend cancellation")),
            }),
        )
        .await
    }
}

fn publish_start_failure(execution: &Weak<AgentRunInner>, error: &AgentRunError) {
    let Some(inner) = execution.upgrade() else {
        return;
    };
    if let Ok(mut handle) = inner.handle.lock() {
        *handle = Some(Err(error.clone()));
    }
    inner.handle_ready.notify_waiters();
    if let Ok(mut events) = inner.events.lock()
        && matches!(*events, EventStreamState::Waiting)
    {
        *events = EventStreamState::StartupFailed(error.clone());
    }
}

fn publish_started(
    execution: &Weak<AgentRunInner>,
    runtime_handle: RunHandle,
    subscription: EventSubscription,
) {
    let Some(inner) = execution.upgrade() else {
        return;
    };
    if let Ok(mut handle) = inner.handle.lock() {
        *handle = Some(Ok(runtime_handle));
    }
    inner.handle_ready.notify_waiters();
    if let Ok(mut events) = inner.events.lock()
        && matches!(*events, EventStreamState::Waiting)
    {
        *events = EventStreamState::Active(subscription);
    }
}

fn publish_result(execution: &Weak<AgentRunInner>, result: Result<AgentRunOutput, AgentRunError>) {
    let Some(inner) = execution.upgrade() else {
        return;
    };
    if let Ok(mut retained) = inner.result.lock() {
        *retained = Some(result);
    }
    inner.result_ready.notify_waiters();
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
    context_providers: Vec<(ComponentRef, Arc<dyn ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn Observer>)>,
    instructions: Vec<InstructionSpec>,
    capabilities: Vec<CapabilitySpec>,
    active_application: BTreeSet<CapabilityId>,
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
            context_providers: Vec::new(),
            middleware: Vec::new(),
            observers: Vec::new(),
            instructions: Vec::new(),
            capabilities: Vec::new(),
            active_application: BTreeSet::new(),
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

    /// Add one ordered direct [`ContextProvider`] handle.
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
    #[must_use]
    pub fn middleware(mut self, component: ComponentRef, middleware: Arc<dyn Middleware>) -> Self {
        self.middleware.push((component, middleware));
        self
    }

    /// Add one ordered direct [`Observer`] handle.
    #[must_use]
    pub fn observer(mut self, component: ComponentRef, observer: Arc<dyn Observer>) -> Self {
        self.observers.push((component, observer));
        self
    }

    /// Add one validated declarative capability to the finite catalog.
    #[must_use]
    pub fn capability(mut self, capability: CapabilitySpec) -> Self {
        self.capabilities.push(capability);
        self
    }

    /// Select one application capability for the initial immutable plan.
    #[must_use]
    pub fn activate_application(mut self, capability: CapabilityId) -> Self {
        self.active_application.insert(capability);
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

fn validate_compact_catalog(capabilities: &[CapabilitySpec]) -> Result<(), AgentRunError> {
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

fn activation_tokens(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() >= 4)
        .filter(|token| {
            !matches!(
                token.as_str(),
                "capability" | "model" | "with" | "from" | "that" | "this"
            )
        })
        .collect()
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
    /// Maximum structured-output validation retries.
    pub max_output_retries: u32,
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
            max_output_retries: DEFAULT_MAX_OUTPUT_RETRIES,
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
            || self.max_output_retries > MAX_CONFIGURED_OUTPUT_RETRIES
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
    /// Durable retry attempts consumed by the completed run.
    pub retry_attempts: u32,
    /// Complete sorted active capability set committed for this run.
    pub active_capabilities: Arc<[ActiveCapability]>,
    /// Stable committed record-kind trace in journal order.
    pub record_kinds: Arc<[Arc<str>]>,
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

    /// Borrow the structured JSON result when the final message contains one.
    #[must_use]
    pub fn structured_json(&self) -> Option<&RawJson> {
        self.message.content().iter().find_map(|block| match block {
            ContentBlock::Json(value) => Some(value.value()),
            _ => None,
        })
    }

    /// Return the durable retry-attempt count.
    #[must_use]
    pub const fn retry_attempts(&self) -> u32 {
        self.retry_attempts
    }

    /// Borrow the complete committed active capability set.
    #[must_use]
    pub fn active_capabilities(&self) -> &[ActiveCapability] {
        &self.active_capabilities
    }

    /// Borrow the stable committed record-kind trace in journal order.
    #[must_use]
    pub fn record_kinds(&self) -> &[Arc<str>] {
        &self.record_kinds
    }
}

/// Stable native facade failure.
#[derive(Debug, Clone, Error)]
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
    /// Explicit durable cancellation reached its terminal state.
    #[error("{code}: run was cancelled")]
    Cancelled {
        /// Stable error code.
        code: &'static str,
    },
}

impl AgentRunError {
    /// Stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Configuration { code, .. }
            | Self::Runtime { code, .. }
            | Self::Timeout { code, .. }
            | Self::Cancelled { code } => code,
        }
    }

    /// Whether an identical frontend call is safe to retry automatically.
    ///
    /// Native run errors remain non-retryable because committed acceptance or
    /// external-effect state may already exist.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        false
    }

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
        finstack_ai_runtime::Clock::now(&AgentClock).map_err(AgentRunError::from)
    }

    fn generate<T: finstack_ai_kernel::IdTag>() -> Result<finstack_ai_kernel::Id<T>, AgentRunError>
    {
        UuidV7Generator::new(AgentClock, AgentRandom)
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

    #[cfg(feature = "native-tokio")]
    fn interaction_resolve_environment() -> Result<TransitionEnv, AgentRunError> {
        Ok(TransitionEnv {
            now: Self::now()?,
            ids: AllocatedIds::try_new(
                generate_many::<RecordTag>(2)?,
                generate_many::<EventTag>(2)?,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                generate_many::<AppendBatchTag>(1)?,
                Vec::new(),
            )
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
        })
    }

    fn cancellation_environment() -> Result<TransitionEnv, AgentRunError> {
        Ok(TransitionEnv {
            now: Self::now()?,
            ids: AllocatedIds::try_new(
                generate_many::<RecordTag>(1)?,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                generate_many::<AppendBatchTag>(1)?,
                generate_many::<CancellationRequestTag>(1)?,
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

    const fn retry() -> Self {
        Self::new(3, 1, 1, 0, 0, 0)
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
        let state = recover_state(Arc::clone(&store), session_id).await?;
        if state.phase.is_some_and(|phase| phases.contains(&phase)) {
            return Ok(state);
        }
        if let finstack_ai_runtime::RunStatus::Faulted { code } = handle.status() {
            return Err(AgentRunError::runtime_message(format!(
                "runtime task faulted: {code}"
            )));
        }
        driver::yield_now().await;
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
        Some(TerminalState::Cancelled(_)) => Err(AgentRunError::Cancelled {
            code: AGENT_RUN_CANCELLED,
        }),
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
    output: OutputSpec,
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
        output,
        settings,
        limits: ModelRequestLimits {
            max_input_bytes: profile.profile.hard_input_bytes,
            max_input_tokens,
            max_output_tokens: profile.profile.reserved_output_tokens,
        },
    })
}

fn structured_candidate(
    state: &finstack_ai_kernel::KernelState,
) -> Option<(MessageId, RawJson, StructuredResultSource)> {
    let message = state.messages.last()?;
    for (index, block) in message.content().iter().enumerate() {
        match block {
            ContentBlock::Json(value) => {
                return Some((
                    *message.id(),
                    value.value().clone(),
                    StructuredResultSource::JsonBlock {
                        content_index: u32::try_from(index).ok()?,
                    },
                ));
            }
            ContentBlock::ToolCall(call)
                if call.tool_name() == finstack_ai_kernel::SUBMIT_FINAL_OUTPUT_TOOL =>
            {
                return Some((
                    *message.id(),
                    call.arguments().clone(),
                    StructuredResultSource::InternalTool {
                        tool_call_id: *call.tool_call_id(),
                    },
                ));
            }
            _ => {}
        }
    }
    None
}

fn model_output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"{\"kind\":\"model_response\",\"schema_version\":1}"),
    }
}

fn session_error(error: &SessionError) -> AgentRunError {
    AgentRunError::configuration(error.code(), error.to_string())
}

async fn append_lane_input(
    runtime: &SessionRuntime,
    lane_id: LaneId,
    input: &str,
) -> Result<(), AgentRunError> {
    let now = NativeIds::now()?;
    let message = text_message(
        NativeIds::generate::<MessageTag>()?,
        MessageRole::User,
        input,
        now,
    )?;
    runtime
        .append_message(
            lane_id,
            &message,
            LaneAppendIds {
                entry_record_id: NativeIds::generate()?,
                lane_moved_record_id: NativeIds::generate()?,
                batch_id: NativeIds::generate()?,
            },
        )
        .await
        .map_err(|error| session_error(&error))?;
    Ok(())
}

impl crate::Lane {
    /// Start a new root run on this idle lane.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane or agent configuration/runtime failure.
    pub fn run(&self, agent: &Agent, request: AgentRunRequest) -> Result<AgentRun, AgentRunError> {
        agent.start_on_lane(self, request)
    }
}

async fn bootstrap_main_lane(
    coordinator: &mut CommitCoordinator,
    session_id: SessionId,
    lane_id: LaneId,
    input: &str,
) -> Result<(), AgentRunError> {
    let now = NativeIds::now()?;
    let message = text_message(
        NativeIds::generate::<MessageTag>()?,
        MessageRole::User,
        input,
        now,
    )?;
    let entry = ConversationEntry::from_message(&message, None, lane_id, 0).map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })?;
    let leaf = entry.id();
    let records = vec![
        session_record_draft(
            NativeIds::generate()?,
            session_id,
            lane_id,
            now,
            RecordBody::SessionCreated(SessionCreated::new(Metadata::empty())),
        )?,
        session_record_draft(
            NativeIds::generate()?,
            session_id,
            lane_id,
            now,
            RecordBody::LaneCreated(LaneCreated::try_new("main").map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?),
        )?,
        session_record_draft(
            NativeIds::generate()?,
            session_id,
            lane_id,
            now,
            RecordBody::ConversationEntry(entry),
        )?,
        session_record_draft(
            NativeIds::generate()?,
            session_id,
            lane_id,
            now,
            RecordBody::LaneMoved(LaneMoved::new(leaf)),
        )?,
    ];
    coordinator
        .commit_session_records(NativeIds::generate()?, records)
        .await
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
    Ok(())
}

fn session_record_draft(
    record_id: finstack_ai_kernel::RecordId,
    session_id: SessionId,
    lane_id: LaneId,
    timestamp: Timestamp,
    body: RecordBody,
) -> Result<RecordDraft, AgentRunError> {
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        record_id,
        session_id,
        lane_id,
        None,
        timestamp,
        Vec::new(),
        body,
    )
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })
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

fn run_task_config(observer_count: usize) -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: DEFAULT_QUEUE_CAPACITY,
        event_hub: EventHubConfig {
            source_capacity: DEFAULT_QUEUE_CAPACITY,
            max_subscribers: 4usize.saturating_add(observer_count),
        },
        shutdown_deadline: Duration::from_secs(2),
    }
}

fn observer_event_subscription() -> EventSubscriptionConfig {
    EventSubscriptionConfig {
        queue_capacity: DEFAULT_QUEUE_CAPACITY,
        filter: EventFilter {
            include_durable: true,
            include_transient: true,
            kinds: Arc::from([]),
            max_sensitivity: Sensitivity::Credential,
        },
        batching: EventBatchConfig {
            flush_count: DEFAULT_EVENT_BATCH_COUNT,
            flush_bytes: DEFAULT_EVENT_BATCH_BYTES,
            flush_interval: DEFAULT_EVENT_BATCH_INTERVAL,
        },
        progress_coalescing: ProgressCoalescing::Enabled,
        lag_policy: EventLagPolicy::DropProgress {
            durable_timeout: Duration::from_secs(2),
        },
    }
}

fn attach_plan_observers(handle: &RunHandle, observers: &[crate::ResolvedComponent<dyn Observer>]) {
    for component in observers {
        let observer = Arc::clone(component.handle());
        let handle = handle.clone();
        let _ = driver::spawn(Box::pin(async move {
            let Ok(mut subscription) = handle
                .subscribe_observer(observer_event_subscription())
                .await
            else {
                return;
            };
            while let Some(batch) = subscription.next_batch().await {
                let events: Arc<[RunEvent]> = Arc::from(batch.events().to_vec());
                let _ = observer.observe(events).await;
            }
        }));
    }
}

fn default_event_subscription() -> EventSubscriptionConfig {
    EventSubscriptionConfig {
        queue_capacity: DEFAULT_QUEUE_CAPACITY,
        filter: EventFilter {
            include_durable: true,
            include_transient: true,
            kinds: Arc::from([]),
            max_sensitivity: Sensitivity::Confidential,
        },
        batching: EventBatchConfig {
            flush_count: DEFAULT_EVENT_BATCH_COUNT,
            flush_bytes: DEFAULT_EVENT_BATCH_BYTES,
            flush_interval: DEFAULT_EVENT_BATCH_INTERVAL,
        },
        progress_coalescing: ProgressCoalescing::Enabled,
        lag_policy: EventLagPolicy::DropProgress {
            durable_timeout: Duration::from_secs(2),
        },
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

#[cfg(all(test, feature = "native-tokio"))]
mod tests {
    use std::collections::BTreeSet;

    use finstack_ai_kernel::{
        AgentId, BundleId, ComponentId, ComponentRef, RunEventClass, Usage, Version,
    };
    use finstack_ai_runtime::{
        JournalStore, ModelContextProfile, ModelResponse, ModelStreamItem, ModelToolCall,
        NoopObserver, ObserverDescriptor, ObserverError, ObserverPayloadMode, TokenEstimatorRef,
        TokenEstimatorSource, ToolCallDelta, Toolset,
    };
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
    use finstack_ai_test::{
        ManualGate, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedObserver,
        ScriptedObserverAction,
    };
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

    async fn model_only_agent(model: Arc<ScriptedModel>) -> (Agent, Arc<MemoryJournalStore>) {
        let model_id = ComponentId::parse("test.model.preview").expect("model id");
        let store_id = ComponentId::parse("test.store.preview").expect("store id");
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
                model,
                store: Arc::clone(&store),
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
        let composed_agent = BundleResolver::new(
            &catalog,
            VERSION,
            BTreeSet::new(),
            RuntimeServices::default(),
        )
        .resolve_agent(
            &mut registry,
            &bundle_id,
            &agent_id,
            BTreeMap::new(),
            AgentConstructionContext::new(),
        )
        .await
        .expect("resolved");
        (
            Agent::try_from_resolved(Arc::new(composed_agent)).expect("Agent"),
            store,
        )
    }

    fn request(input: &str) -> AgentRunRequest {
        AgentRunRequest::try_new(
            ModelName::try_new("preview-1").expect("model name"),
            input,
            security(),
        )
        .expect("request")
    }

    #[test]
    fn compact_model_catalog_is_bounded_and_tokenized_deterministically() {
        let oversized = CapabilitySpec {
            id: CapabilityId::parse("test.capability.oversized").expect("capability id"),
            description: Arc::from("x".repeat(MAX_COMPACT_CATALOG_BYTES + 1)),
            instructions: Arc::from([]),
            toolsets: Arc::from([]),
            context_providers: Arc::from([]),
            middleware: Arc::from([]),
            activation: CapabilityActivation::Model,
        };
        let error = validate_compact_catalog(&[oversized]).expect_err("catalog must be bounded");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(
            error
                .to_string()
                .contains("compact capability catalog exceeds")
        );
        assert_eq!(
            activation_tokens("Research this capability: financial-statements"),
            BTreeSet::from([
                "financial".to_owned(),
                "research".to_owned(),
                "statements".to_owned(),
            ])
        );
    }

    #[tokio::test]
    async fn model_only_agent_completes_without_double_warmup() {
        let model = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![completed("preview ready")],
        ));
        let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
        assert_eq!(model.warmup_count(), 1);
        let output = agent.run(request("Say hello")).await.expect("run");
        assert_eq!(output.text(), "preview ready");
        assert_eq!(model.request_count(), 1);
        assert_eq!(model.warmup_count(), 1);
    }

    #[tokio::test]
    async fn started_run_retains_result_and_delivers_bounded_batches() {
        let mut plan = completed("batched");
        plan.actions.splice(
            0..1,
            ["b", "a", "t", "c", "h", "e", "d"].map(|text| {
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                    finstack_ai_runtime::TextDelta {
                        text: Arc::from(text),
                    },
                )))
            }),
        );
        let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
        let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
        let run = agent.start(request("batch events")).expect("start");
        let observer = run.clone();

        let batches = tokio::time::timeout(Duration::from_secs(3), async move {
            let mut batches = Vec::new();
            while let Some(batch) = observer.next_event_batch().await.expect("event batch") {
                assert_eq!(
                    batch.first_sequence(),
                    batch
                        .events()
                        .first()
                        .expect("first event")
                        .transient_sequence()
                );
                assert_eq!(
                    batch.last_sequence(),
                    batch
                        .events()
                        .last()
                        .expect("last event")
                        .transient_sequence()
                );
                assert!(
                    batch
                        .events()
                        .windows(2)
                        .all(|events| events[0].transient_sequence()
                            < events[1].transient_sequence())
                );
                batches.push(batch);
            }
            batches
        })
        .await
        .expect("event delivery settled");
        let event_count = batches
            .iter()
            .map(|batch| batch.events().len())
            .sum::<usize>();
        assert!(batches.iter().any(|batch| batch.events().len() > 1));
        assert!(event_count > batches.len());
        assert!(batches.iter().any(|batch| {
            batch
                .events()
                .iter()
                .any(|event| event.class() == RunEventClass::Transient)
        }));

        let first = run.result().await.expect("first retained result");
        let second = run.result().await.expect("second retained result");
        assert_eq!(first.text(), "batched");
        assert_eq!(second.text(), "batched");
        assert_eq!(first.locator, second.locator);
        assert_eq!(model.request_count(), 1);
    }

    #[tokio::test]
    async fn cancelled_event_wait_restores_the_single_consumer() {
        let control_name = Arc::<str>::from("cancelled-event-wait");
        let mut plan = completed("event delivery resumed");
        plan.actions
            .insert(0, ScriptedModelAction::Block(Arc::clone(&control_name)));
        let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
        let control = model.control();
        let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
        let run = agent
            .start(request("resume event delivery"))
            .expect("start");
        tokio::time::timeout(Duration::from_secs(3), async {
            while control.entries(&control_name) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("model reached gate");

        let mut cancelled_wait = false;
        for _ in 0..32 {
            match tokio::time::timeout(Duration::from_millis(20), run.next_event_batch()).await {
                Ok(Ok(Some(_))) => {}
                Ok(Ok(None)) => panic!("event stream closed before model release"),
                Ok(Err(error)) => panic!("event delivery failed: {error}"),
                Err(_) => {
                    cancelled_wait = true;
                    break;
                }
            }
        }
        assert!(
            cancelled_wait,
            "expected one pending batch wait to be cancelled"
        );

        control.release(&control_name);
        tokio::time::timeout(Duration::from_secs(3), async {
            while run.next_event_batch().await.expect("event batch").is_some() {}
        })
        .await
        .expect("event delivery resumed after waiter cancellation");
        assert_eq!(
            run.result().await.expect("retained result").text(),
            "event delivery resumed"
        );
    }

    #[tokio::test]
    async fn explicit_cancellation_is_idempotent_and_terminal() {
        let model = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![ScriptedModelPlan {
                actions: vec![ScriptedModelAction::AwaitCancellation],
            }],
        ));
        let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
        let run = agent
            .start(request("wait for cancellation"))
            .expect("start");
        tokio::time::timeout(Duration::from_secs(3), async {
            while model.request_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("model request started");

        run.cancel().await.expect("first cancellation");
        run.cancel().await.expect("idempotent cancellation");
        let error = run.result().await.expect_err("cancelled result");
        assert_eq!(error.code(), AGENT_RUN_CANCELLED);
        assert!(!error.retryable());
        assert_eq!(model.cancellation_acknowledgement_count(), 1);
        assert_eq!(model.active_stream_count(), 0);
    }

    #[tokio::test]
    async fn dropping_last_handle_detaches_without_cancelling_execution() {
        let control_name = Arc::<str>::from("detached-run");
        let mut plan = completed("detached completion");
        plan.actions
            .insert(0, ScriptedModelAction::Block(Arc::clone(&control_name)));
        let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
        let control = model.control();
        let (agent, store) = model_only_agent(Arc::clone(&model)).await;
        let run = agent.start(request("detach")).expect("start");
        let session_id = run.locator().session_id;
        tokio::time::timeout(Duration::from_secs(3), async {
            while control.entries(&control_name) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("model reached gate");

        drop(run);
        control.release(&control_name);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let journal: Arc<dyn JournalStore> = store.clone();
                let state = CommitCoordinator::recover(journal, session_id)
                    .await
                    .expect("recover detached run");
                if matches!(state.state().terminal, Some(TerminalState::Completed(_))) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached run completed");
        assert_eq!(model.cancellation_acknowledgement_count(), 0);
        assert_eq!(model.active_stream_count(), 0);
        assert_eq!(model.dropped_stream_count(), 1);
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

    fn observer_descriptor(id: &str) -> ObserverDescriptor {
        ObserverDescriptor {
            component: ComponentRef::new(
                ComponentId::parse(id).expect("observer id"),
                Some(VERSION),
            ),
            payload_mode: ObserverPayloadMode::Redacted,
            metadata: Metadata::empty(),
        }
    }

    async fn run_with_observer(observer_id: &str, observer: Arc<dyn Observer>) -> AgentRunOutput {
        let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![completed("observer-ok")],
        ));
        let store: Arc<dyn JournalStore> = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 64,
                records_per_session: 512,
                snapshot_bytes: 4_096,
            })
            .expect("store"),
        );
        Agent::builder(
            AgentId::parse("test.agent.observer").expect("agent"),
            BundleId::parse("test.bundle.observer").expect("bundle"),
            (
                ComponentRef::new(
                    ComponentId::parse("test.model.preview").expect("model"),
                    Some(VERSION),
                ),
                model,
            ),
            (
                ComponentRef::new(
                    ComponentId::parse("test.store.preview").expect("store"),
                    Some(VERSION),
                ),
                store,
            ),
        )
        .observer(
            ComponentRef::new(
                ComponentId::parse(observer_id).expect("observer component"),
                Some(VERSION),
            ),
            observer,
        )
        .build()
        .await
        .expect("build")
        .run(request("observe me"))
        .await
        .expect("run")
    }

    #[tokio::test]
    async fn failing_or_stalled_observer_does_not_change_journal_prefix() {
        let noop: Arc<dyn Observer> =
            Arc::new(NoopObserver::new(observer_descriptor("test.observer.noop")));
        let failing: Arc<dyn Observer> = Arc::new(
            ScriptedObserver::try_new(
                observer_descriptor("test.observer.fail"),
                64,
                vec![ScriptedObserverAction::Return(Err(
                    ObserverError::Unavailable,
                ))],
            )
            .expect("failing"),
        );
        let gate = ManualGate::default();
        let stalled: Arc<dyn Observer> = Arc::new(
            ScriptedObserver::try_new(
                observer_descriptor("test.observer.stall"),
                64,
                vec![ScriptedObserverAction::Wait {
                    gate,
                    outcome: Ok(()),
                }],
            )
            .expect("stalled"),
        );
        let noop_out = run_with_observer("test.observer.noop", noop).await;
        let fail_out = run_with_observer("test.observer.fail", failing).await;
        let stall_out = run_with_observer("test.observer.stall", stalled).await;
        assert_eq!(noop_out.text(), "observer-ok");
        assert_eq!(fail_out.text(), noop_out.text());
        assert_eq!(stall_out.text(), noop_out.text());
        assert_eq!(fail_out.record_kinds(), noop_out.record_kinds());
        assert_eq!(stall_out.record_kinds(), noop_out.record_kinds());
    }
}
