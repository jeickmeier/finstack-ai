use std::sync::{Arc, Weak};
use std::time::Duration;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, AuthorizationEvidence, BudgetPropagation, CancellationInitiator,
    CancellationPropagation, CancellationRequestTag, ContentBlock, ConversationEntry,
    DeadlinePropagation, Digest, EffectOutputContract, EffectOutputKind, EventTag, KernelInput,
    LaneCreated, LaneId, LaneMoved, LaneTag, Message, MessageId, MessageRole, MessageTag, Metadata,
    ModelRequestTag, OperationLocator, OutputSpec, PrincipalPropagation, ProviderIds,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RawJson, RecordBody, RecordDraft, RecordTag,
    ReducerStageOutcome, RunAccepted, RunPhase, RunPropagationPolicy, RunRelation, RunTag,
    Sensitivity, SessionCreated, SessionId, SessionTag, Stage, StageCursor, StageSettled,
    StructuredResultSource, TerminalState, TextBlock, Timestamp, TransitionEnv, TurnTag,
};
use finstack_ai_runtime::{
    CommitCoordinator, EventBatchConfig, EventFilter, EventHubConfig, EventLagPolicy,
    EventSubscriptionConfig, LaneAppendIds, LockedModelContextProfile, Model, ModelCapabilities,
    ModelContextProfileOverride, ModelDescriptor, ModelError, ModelEventStream, ModelName,
    ModelReconcileResult, ModelRequest, ModelRequestDraft, ModelRequestLimits, ModelSettings,
    ModelTaskConfig, ModelTokenEstimate, ModelWarmupContext, Observer, PendingModelEffect,
    PortFuture, ProgressCoalescing, ReconcileContext, RunEvent, RunHandle, RunTaskConfig,
    RunTaskOwner, SessionError, SessionRuntime, StructuredOutputCapability, ToolStreamLimits,
    ToolTaskConfig, UuidV7Generator, resolve_model_context_profile,
};

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

use super::Agent;
use super::run::{AgentRun, AgentRunInner, publish_start_failure, publish_started};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunOutput, AgentRunRequest,
    DEFAULT_EVENT_BATCH_BYTES, DEFAULT_EVENT_BATCH_COUNT, DEFAULT_EVENT_BATCH_INTERVAL,
    DEFAULT_QUEUE_CAPACITY,
};

pub(super) struct PreparedAgentRun {
    pub(super) model: Arc<dyn Model>,
    pub(super) profile: LockedModelContextProfile,
    pub(super) store: Arc<dyn finstack_ai_runtime::JournalStore>,
    pub(super) session_id: SessionId,
    pub(super) lane_id: LaneId,
    pub(super) accepted: RunAccepted,
    pub(super) request: AgentRunRequest,
    pub(super) locator: OperationLocator,
    pub(super) bootstrap: bool,
    pub(super) session: Option<crate::Session>,
}

impl PreparedAgentRun {
    pub(super) fn cancellation_initiator(&self) -> Result<CancellationInitiator, AgentRunError> {
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

impl Agent {
    pub(super) fn prepare(
        &self,
        request: AgentRunRequest,
    ) -> Result<PreparedAgentRun, AgentRunError> {
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

    pub(super) fn prepare_on(
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
    pub(super) async fn execute_started(
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
        coordinator
            .install_middleware_chain(Arc::clone(self.resolved.run_plan().middleware_chain()));
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
            Err(_) => Err(AgentRunError::Timeout { timeout }),
        }
    }
    pub(super) fn context_messages(
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
pub(super) struct NativeIds;

impl NativeIds {
    pub(super) fn now() -> Result<Timestamp, AgentRunError> {
        finstack_ai_runtime::Clock::now(&AgentClock).map_err(AgentRunError::from)
    }

    pub(super) fn generate<T: finstack_ai_kernel::IdTag>()
    -> Result<finstack_ai_kernel::Id<T>, AgentRunError> {
        UuidV7Generator::new(AgentClock, AgentRandom)
            .generate()
            .map_err(AgentRunError::from)
    }

    pub(super) fn environment(
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
    pub(super) fn interaction_resolve_environment() -> Result<TransitionEnv, AgentRunError> {
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

    pub(super) fn cancellation_environment() -> Result<TransitionEnv, AgentRunError> {
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
pub(super) struct StageIds {
    records: usize,
    events: usize,
    effects: usize,
    turns: usize,
    model_requests: usize,
    messages: usize,
}

impl StageIds {
    pub(super) const fn continued() -> Self {
        Self::new(1, 0, 0, 0, 0, 0)
    }

    pub(super) const fn context() -> Self {
        Self::new(2, 0, 0, 1, 0, 0)
    }

    pub(super) const fn model_request() -> Self {
        Self::new(2, 1, 1, 0, 1, 0)
    }

    pub(super) const fn finalize() -> Self {
        Self::new(2, 1, 0, 0, 0, 0)
    }

    pub(super) const fn retry() -> Self {
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

pub(super) async fn submit_stage(
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

pub(super) async fn submit(
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

pub(super) async fn recover_state(
    store: Arc<dyn finstack_ai_runtime::JournalStore>,
    session_id: SessionId,
) -> Result<finstack_ai_kernel::KernelState, AgentRunError> {
    CommitCoordinator::recover(store, session_id)
        .await
        .map(|coordinator| coordinator.state().clone())
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))
}

pub(super) async fn wait_for_phase(
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

pub(super) fn ensure_nonterminal_failure(
    state: &finstack_ai_kernel::KernelState,
) -> Result<(), AgentRunError> {
    match state.terminal.as_ref() {
        Some(TerminalState::Failed(failed)) => Err(AgentRunError::runtime_message(format!(
            "run failed: {}",
            failed.error.code
        ))),
        Some(TerminalState::Cancelled(_)) => Err(AgentRunError::Cancelled),
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

pub(super) fn model_draft(
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

pub(super) fn structured_candidate(
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

pub(super) fn model_output_contract() -> EffectOutputContract {
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
    /// Prefer [`Agent::start_on_lane`].
    ///
    /// # Errors
    ///
    /// Returns a busy-lane or agent configuration/runtime failure.
    #[deprecated(since = "1.0.0", note = "use Agent::start_on_lane")]
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
