use std::sync::{Arc, Weak};
use std::time::Duration;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, AuthorizationEvidence, BudgetPropagation, CancelRequested,
    CancellationInitiator, CancellationPropagation, CancellationRequestTag, ContentBlock,
    DeadlinePropagation, Digest, EffectOutputContract, EffectOutputKind, EventTag, KernelInput,
    LaneId, LaneTag, MediaRef, Message, MessageId, MessageRole, MessageTag, Metadata,
    ModelRequestTag, OperationLocator, OutputSpec, PrincipalPropagation, ProviderIds, RawJson,
    RecordTag, ReducerStageOutcome, RunAccepted, RunPhase, RunPropagationPolicy, RunRelation,
    RunTag, Sensitivity, SessionId, SessionTag, Stage, StageCursor, StageSettled,
    StructuredResultSource, TerminalState, TextBlock, Timestamp, TransitionEnv, TurnTag,
};
use finstack_ai_runtime::{
    ApprovalGrantMode, ContextProvider, EventBatchConfig, EventFilter, EventHubConfig,
    EventLagPolicy, EventSubscriptionConfig, LaneAppendIds, LockedModelContextProfile, Model,
    ModelContextProfileOverride, ModelName, ModelRequestDraft, ModelRequestLimits, ModelSettings,
    ModelTaskConfig, Observer, ProgressCoalescing, ReadyModel, RunHandle, RunTaskConfig,
    RunTaskOwner, SameIdentityRetryPolicy, SessionCreateIds, SessionError, SessionRuntime,
    StructuredOutputCapability, ToolStreamLimits, ToolTaskConfig, UuidV7Generator,
    resolve_model_context_profile,
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

use super::drive::output_from_live_state;
use super::handle::Agent;
use super::run::{AgentRunInner, publish_start_failure, publish_started};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunOutput, AgentRunRequest,
    AttachmentInput, DEFAULT_EVENT_BATCH_BYTES, DEFAULT_EVENT_BATCH_COUNT,
    DEFAULT_EVENT_BATCH_INTERVAL, DEFAULT_QUEUE_CAPACITY,
};

pub(super) struct PreparedAgentRun {
    pub(super) model: Arc<ReadyModel>,
    pub(super) profile: LockedModelContextProfile,
    pub(super) store: Arc<dyn finstack_ai_runtime::JournalStore>,
    pub(super) session_id: SessionId,
    pub(super) lane_id: LaneId,
    pub(super) accepted: RunAccepted,
    pub(super) request: AgentRunRequest,
    pub(super) locator: OperationLocator,
    pub(super) session: Option<crate::Session>,
}

pub(super) struct RunContextSeed {
    pub(super) messages: Arc<[Message]>,
    pub(super) source_leaf_id: finstack_ai_kernel::EntryId,
    pub(super) journal_sequence: u64,
    pub(super) head_checksum: Option<Digest>,
}

const OWNER_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

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
        let mut limits = attenuated_run_limits(&spec.limits, &request)?;
        if self.structured_output.is_some() {
            limits.max_retries = Some(
                limits
                    .max_retries
                    .map_or(request.max_output_retries, |limit| {
                        limit.min(request.max_output_retries)
                    }),
            );
        }
        let accepted_at = NativeIds::now()?;
        let effective_deadline = request_deadline(accepted_at, request.timeout, None)?;
        let accepted = RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).map_err(|error| {
                AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
            })?,
            request.security.clone(),
            effective_deadline,
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
        prepared.session = Some(lane.session().clone());
        Ok(prepared)
    }

    #[cfg(feature = "native-tokio")]
    pub(super) fn prepare_accepted(
        &self,
        request: AgentRunRequest,
        locator: OperationLocator,
        accepted: RunAccepted,
        session: crate::Session,
    ) -> Result<PreparedAgentRun, AgentRunError> {
        let mut prepared = self.prepare(request)?;
        prepared.store = session.journal_store();
        prepared.session_id = locator.session_id;
        prepared.lane_id = locator.lane_id;
        prepared.accepted = accepted;
        prepared.locator = locator;
        prepared.session = Some(session);
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
        let runtime = if let Some(session) = &prepared.session {
            match session.runtime().await {
                Ok(runtime) => runtime,
                Err(error) => {
                    let error = session_error(&error);
                    publish_start_failure(execution, &error);
                    return Err(error);
                }
            }
        } else {
            match create_session_runtime(
                Arc::clone(&prepared.store),
                prepared.request.security.tenant_scope(),
                prepared.session_id,
                prepared.lane_id,
            )
            .await
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    publish_start_failure(execution, &error);
                    return Err(error);
                }
            }
        };
        let context_seed = match append_lane_input(
            &runtime,
            prepared.lane_id,
            prepared.accepted.run_id(),
            &prepared.request.input,
            &prepared.request.attachments,
        )
        .await
        {
            Ok(seed) => seed,
            Err(error) => {
                publish_start_failure(execution, &error);
                return Err(error);
            }
        };
        let seeded_leaf = runtime.projection().ok().and_then(|projection| {
            projection
                .lane_by_id(prepared.lane_id)
                .and_then(|lane| lane.leaf_id)
        });
        if seeded_leaf != Some(context_seed.source_leaf_id)
            || context_seed.journal_sequence == 0
            || context_seed.head_checksum.is_none()
        {
            runtime.release_run(prepared.lane_id, prepared.accepted.run_id());
            let error = AgentRunError::runtime_message(
                "accepted lane context does not match the confirmed session head",
            );
            publish_start_failure(execution, &error);
            return Err(error);
        }
        let acquired_lane = Some((
            Arc::clone(&runtime),
            prepared.lane_id,
            prepared.accepted.run_id(),
        ));
        let mut coordinator = match runtime
            .coordinator_for_run(Some(prepared.accepted.run_id()))
            .await
        {
            Ok(coordinator) => coordinator,
            Err(error) => {
                runtime.release_run(prepared.lane_id, prepared.accepted.run_id());
                let error = session_error(&error);
                publish_start_failure(execution, &error);
                return Err(error);
            }
        };
        coordinator
            .install_middleware_chain(Arc::clone(self.resolved.run_plan().middleware_chain()));
        coordinator.install_capability_owners(self.capability_index().as_arc_owners());
        let providers: Arc<[Arc<dyn ContextProvider>]> = self
            .resolved
            .run_plan()
            .context_providers()
            .iter()
            .map(|component| Arc::clone(component.handle()))
            .collect::<Vec<_>>()
            .into();
        coordinator.install_context_providers(providers);
        if let Ok(mut cache) = self.history_cache.lock()
            && let Some(checkpoint) = cache.candidate(
                prepared.session_id,
                prepared.lane_id,
                prepared.profile.digest,
            )
        {
            coordinator.seed_compaction_checkpoint(checkpoint);
        }
        let observer_count = self.resolved.run_plan().observers().len();
        let approval_grant = self
            .resolved
            .spec()
            .map(|spec| spec.policy.approval_grant)
            .unwrap_or_default();
        let owner = if self.tools.is_empty() {
            Box::pin(RunTaskOwner::spawn_with_model_and_artifacts(
                coordinator,
                run_task_config(observer_count, approval_grant),
                model_task_config(),
                Arc::clone(&prepared.model),
                prepared.profile.clone(),
                self.artifact_store
                    .clone()
                    .map(|store| (store, prepared.locator.clone())),
                AgentClock,
                AgentRandom,
            ))
            .await
            .map_err(AgentRunError::runtime)
        } else {
            Box::pin(RunTaskOwner::spawn_with_model_tools_and_artifacts(
                coordinator,
                run_task_config(observer_count, approval_grant),
                model_task_config(),
                tool_task_config(),
                Arc::clone(&prepared.model),
                prepared.profile.clone(),
                Arc::clone(&self.tools),
                self.artifact_store
                    .clone()
                    .map(|store| (store, prepared.locator.clone())),
                AgentClock,
                AgentRandom,
            ))
            .await
            .map_err(AgentRunError::runtime)
        };
        let mut owner = match owner {
            Ok(owner) => owner,
            Err(error) => {
                if let Some((runtime, lane_id, run_id)) = &acquired_lane {
                    runtime.release_run(*lane_id, *run_id);
                }
                publish_start_failure(execution, &error);
                return Err(error);
            }
        };
        let handle = owner.handle();
        if let Some((runtime, lane_id, run_id)) = &acquired_lane
            && let Err(error) = runtime.bind_run_handle(*lane_id, *run_id, handle.clone())
        {
            let _shutdown = owner.shutdown().await;
            runtime.release_run(*lane_id, *run_id);
            let error = session_error(&error);
            publish_start_failure(execution, &error);
            return Err(error);
        }
        let subscription = match handle.subscribe_events(default_event_subscription()).await {
            Ok(subscription) => subscription,
            Err(error) => {
                let error = AgentRunError::runtime_message(error.to_string());
                let _shutdown = owner.shutdown().await;
                if let Some((runtime, lane_id, run_id)) = &acquired_lane {
                    runtime.release_run(*lane_id, *run_id);
                }
                publish_start_failure(execution, &error);
                return Err(error);
            }
        };
        attach_plan_observers(&mut owner, self.resolved.run_plan().observers()).await;
        publish_started(execution, handle.clone(), subscription);
        let timeout = prepared.request.timeout;
        let locator = prepared.locator.clone();
        let effective_deadline = prepared.accepted.effective_deadline();
        let cache_session_id = prepared.session_id;
        let cache_lane_id = prepared.lane_id;
        let result = driver::timeout(
            timeout,
            Box::pin(self.drive(
                &handle,
                prepared.session_id,
                prepared.lane_id,
                prepared.accepted,
                prepared.request,
                prepared.profile,
                prepared.locator,
                context_seed,
            )),
        )
        .await;
        let result = match result {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) => {
                let state = handle.live_state();
                if state.terminal.is_none() {
                    match settle_controller_cancellation(
                        &handle,
                        CancellationInitiator::RuntimeShutdown,
                        None,
                    )
                    .await
                    {
                        Ok(_) => Err(error),
                        Err(uncertain) => Err(uncertain),
                    }
                } else {
                    Err(error)
                }
            }
            Err(_) => settle_deadline_timeout(&handle, locator, timeout, effective_deadline).await,
        };
        let shutdown = owner.shutdown().await;
        if let Some((runtime, lane_id, run_id)) = acquired_lane {
            let graceful = matches!(
                shutdown.outcome,
                finstack_ai_runtime::ShutdownOutcome::Graceful
            ) && !matches!(
                handle.status(),
                finstack_ai_runtime::RunStatus::Faulted { .. }
            );
            if graceful {
                let Some(update) = owner.take_session_head() else {
                    let _invalidated = runtime.invalidate_session_head();
                    runtime.release_run(lane_id, run_id);
                    return Err(runtime_uncertainty(
                        "graceful owner shutdown did not retain a confirmed session head",
                    ));
                };
                if let Err(error) = runtime.adopt_session_head(update).await {
                    let _invalidated = runtime.invalidate_session_head();
                    runtime.release_run(lane_id, run_id);
                    return Err(session_error(&error));
                }
                if let Some(checkpoint) = owner.take_compaction_checkpoint()
                    && let Ok(mut cache) = self.history_cache.lock()
                {
                    cache.insert(cache_session_id, cache_lane_id, checkpoint);
                }
            } else if let Err(error) = runtime.invalidate_session_head() {
                runtime.release_run(lane_id, run_id);
                return Err(session_error(&error));
            }
            if let Ok(output) = &result
                && let Err(error) = runtime
                    .append_message_from_run(
                        lane_id,
                        run_id,
                        &output.message,
                        LaneAppendIds {
                            entry_record_id: NativeIds::generate()?,
                            lane_moved_record_id: NativeIds::generate()?,
                            batch_id: NativeIds::generate()?,
                        },
                    )
                    .await
            {
                runtime.release_run(lane_id, run_id);
                return Err(session_error(&error));
            }
            runtime.release_run(lane_id, run_id);
        }
        result
    }
    pub(super) fn context_messages(
        &self,
        seed: &RunContextSeed,
        committed: &[Message],
        extra_capability_instructions: &[crate::InstructionSpec],
    ) -> Result<Arc<[Message]>, AgentRunError> {
        let now = NativeIds::now()?;
        let spec = self.resolved.spec().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires a bundle-resolved specification",
            )
        })?;
        let mut messages =
            Vec::with_capacity(spec.instructions.len() + seed.messages.len() + committed.len());
        for instruction in spec.instructions.iter() {
            messages.push(text_message(
                NativeIds::generate::<MessageTag>()?,
                MessageRole::System,
                instruction.text(),
                now,
                &[],
            )?);
        }
        for instruction in extra_capability_instructions {
            messages.push(text_message(
                NativeIds::generate::<MessageTag>()?,
                MessageRole::System,
                instruction.text(),
                now,
                &[],
            )?);
        }
        messages.extend_from_slice(&seed.messages);
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

async fn settle_deadline_timeout(
    handle: &RunHandle,
    locator: OperationLocator,
    timeout: Duration,
    effective_deadline: Option<Timestamp>,
) -> Result<AgentRunOutput, AgentRunError> {
    let state = handle.live_state();
    if state.terminal.is_some() {
        return output_from_live_state(handle, locator, &state, timeout);
    }
    let terminal =
        settle_controller_cancellation(handle, CancellationInitiator::Deadline, effective_deadline)
            .await?;
    match terminal.terminal.as_ref() {
        Some(TerminalState::Cancelled(_)) => Err(AgentRunError::Timeout { timeout }),
        Some(TerminalState::Failed(failed))
            if failed.error.code.as_ref() == "deadline_exceeded" =>
        {
            Err(AgentRunError::Timeout { timeout })
        }
        Some(TerminalState::Completed(_) | TerminalState::Failed(_)) => {
            output_from_live_state(handle, locator, &terminal, timeout)
        }
        None => Err(runtime_uncertainty(
            "deadline cancellation settled without a terminal state",
        )),
    }
}

async fn settle_controller_cancellation(
    handle: &RunHandle,
    initiator: CancellationInitiator,
    not_before: Option<Timestamp>,
) -> Result<finstack_ai_runtime::LiveRunState, AgentRunError> {
    let state = handle.live_state();
    if state.terminal.is_some() {
        return Ok(state);
    }
    if matches!(state.status, finstack_ai_runtime::RunStatus::Faulted { .. }) {
        return Err(runtime_uncertainty(
            "runtime faulted before cancellation acknowledgement became certain",
        ));
    }
    let mut env = NativeIds::cancellation_environment()?;
    if let Some(not_before) = not_before {
        env.now = env.now.max(not_before);
    }
    submit(
        handle,
        env,
        KernelInput::CancelRequested(CancelRequested {
            initiator,
            reason: None,
        }),
    )
    .await
    .map_err(|error| {
        runtime_uncertainty(format!(
            "cancellation acknowledgement is uncertain: {error}"
        ))
    })?;
    driver::timeout(OWNER_SHUTDOWN_GRACE, Box::pin(wait_for_terminal(handle)))
        .await
        .map_err(|_| {
            runtime_uncertainty("cancellation did not reach a durable terminal state within grace")
        })?
}

async fn wait_for_terminal(
    handle: &RunHandle,
) -> Result<finstack_ai_runtime::LiveRunState, AgentRunError> {
    loop {
        let state = handle.live_state();
        if state.terminal.is_some() {
            return Ok(state);
        }
        if matches!(
            state.status,
            finstack_ai_runtime::RunStatus::Faulted { .. }
                | finstack_ai_runtime::RunStatus::Stopped
        ) {
            return Err(runtime_uncertainty(
                "runtime stopped before cancellation reached a durable terminal state",
            ));
        }
        handle
            .wait_for_live_state(state.revision)
            .await
            .map_err(|error| {
                runtime_uncertainty(format!(
                    "live-state wait failed during cancellation settlement: {error}"
                ))
            })?;
    }
}

fn runtime_uncertainty(message: impl Into<String>) -> AgentRunError {
    AgentRunError::runtime_message(format!("runtime boundary uncertainty: {}", message.into()))
}

pub(super) async fn wait_for_phase(
    handle: &RunHandle,
    phases: &[RunPhase],
) -> Result<finstack_ai_runtime::LiveRunState, AgentRunError> {
    loop {
        let state = handle.live_state();
        if state.phase.is_some_and(|phase| phases.contains(&phase)) {
            return Ok(state);
        }
        if let finstack_ai_runtime::RunStatus::Faulted { code } = state.status {
            return Err(AgentRunError::runtime_message(format!(
                "runtime task faulted: {code}"
            )));
        }
        handle
            .wait_for_live_state(state.revision)
            .await
            .map_err(AgentRunError::runtime)?;
    }
}

pub(super) async fn wait_for_cycle(
    handle: &RunHandle,
    prior_cycle: u64,
) -> Result<finstack_ai_runtime::LiveRunState, AgentRunError> {
    loop {
        let state = handle.live_state();
        if state.cycle > prior_cycle || state.terminal.is_some() {
            return Ok(state);
        }
        if let finstack_ai_runtime::RunStatus::Faulted { code } = state.status {
            return Err(AgentRunError::runtime_message(format!(
                "runtime task faulted: {code}"
            )));
        }
        handle
            .wait_for_live_state(state.revision)
            .await
            .map_err(AgentRunError::runtime)?;
    }
}

pub(super) fn ensure_nonterminal_failure(
    state: &finstack_ai_runtime::LiveRunState,
    timeout: Duration,
) -> Result<(), AgentRunError> {
    match state.terminal.as_ref() {
        Some(TerminalState::Failed(failed))
            if failed.error.code.as_ref() == "deadline_exceeded" =>
        {
            Err(AgentRunError::Timeout { timeout })
        }
        Some(TerminalState::Failed(failed)) => Err(AgentRunError::runtime_message(format!(
            "run failed: {}: {}",
            failed.error.code, failed.error.message
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
    state: &finstack_ai_runtime::LiveRunState,
) -> Option<(MessageId, RawJson, StructuredResultSource)> {
    let message = state.committed_run_messages.last()?;
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

pub(super) fn attenuated_run_limits(
    configured: &finstack_ai_kernel::RunLimits,
    request: &AgentRunRequest,
) -> Result<finstack_ai_kernel::RunLimits, AgentRunError> {
    let mut limits = configured.clone();
    limits.max_model_requests = Some(
        limits
            .max_model_requests
            .map_or(request.max_cycles, |value| value.min(request.max_cycles)),
    );
    let request_wall_time = kernel_duration(request.timeout)?;
    limits.max_wall_time = Some(
        limits
            .max_wall_time
            .map_or(request_wall_time, |value| value.min(request_wall_time)),
    );
    Ok(limits)
}

pub(super) fn request_deadline(
    started_at: Timestamp,
    timeout: Duration,
    parent_deadline: Option<Timestamp>,
) -> Result<Option<Timestamp>, AgentRunError> {
    let request_deadline = started_at
        .checked_add(kernel_duration(timeout)?)
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
    Ok(Some(parent_deadline.map_or(request_deadline, |parent| {
        parent.min(request_deadline)
    })))
}

fn kernel_duration(timeout: Duration) -> Result<finstack_ai_kernel::Duration, AgentRunError> {
    let millis = u64::try_from(timeout.as_millis()).map_err(|_| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "run timeout exceeds the durable duration range",
        )
    })?;
    Ok(finstack_ai_kernel::Duration::from_millis(millis))
}

async fn append_lane_input(
    runtime: &SessionRuntime,
    lane_id: LaneId,
    run_id: finstack_ai_kernel::RunId,
    input: &str,
    attachments: &[AttachmentInput],
) -> Result<RunContextSeed, AgentRunError> {
    let now = NativeIds::now()?;
    let message = text_message(
        NativeIds::generate::<MessageTag>()?,
        MessageRole::User,
        input,
        now,
        attachments,
    )?;
    runtime
        .begin_run_with_message(
            lane_id,
            run_id,
            &message,
            LaneAppendIds {
                entry_record_id: NativeIds::generate()?,
                lane_moved_record_id: NativeIds::generate()?,
                batch_id: NativeIds::generate()?,
            },
        )
        .await
        .map(|context| RunContextSeed {
            messages: context.messages,
            source_leaf_id: context.source_leaf_id,
            journal_sequence: context.journal_sequence,
            head_checksum: context.head_checksum,
        })
        .map_err(|error| session_error(&error))
}

async fn create_session_runtime(
    store: Arc<dyn finstack_ai_runtime::JournalStore>,
    tenant_scope: &str,
    session_id: SessionId,
    lane_id: LaneId,
) -> Result<Arc<SessionRuntime>, AgentRunError> {
    SessionRuntime::create(
        store,
        Arc::<str>::from(tenant_scope),
        SessionCreateIds {
            session_id,
            main_lane_id: lane_id,
            session_created_record_id: NativeIds::generate()?,
            lane_created_record_id: NativeIds::generate()?,
            batch_id: NativeIds::generate()?,
            now: NativeIds::now()?,
        },
    )
    .await
    .map_err(|error| session_error(&error))
}

fn text_message(
    id: MessageId,
    role: MessageRole,
    text: &str,
    now: Timestamp,
    attachments: &[AttachmentInput],
) -> Result<Message, AgentRunError> {
    let mut blocks = vec![ContentBlock::Text(TextBlock::try_new(text).map_err(
        |error| AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string()),
    )?)];
    for attachment in attachments {
        blocks.push(ContentBlock::File(MediaRef::new(
            attachment.artifact.blob().clone(),
        )));
    }
    Message::try_new(
        id,
        role,
        blocks,
        now,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .map_err(|error| {
        AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
    })
}

fn run_task_config(observer_count: usize, approval_grant: ApprovalGrantMode) -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: DEFAULT_QUEUE_CAPACITY,
        event_hub: EventHubConfig {
            source_capacity: DEFAULT_QUEUE_CAPACITY,
            // One interactive consumer plus one short-lived internal phase waiter.
            max_subscribers: observer_count.checked_add(2).unwrap_or(0),
        },
        shutdown_deadline: OWNER_SHUTDOWN_GRACE,
        approval_grant,
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

async fn attach_plan_observers(
    owner: &mut RunTaskOwner,
    observers: &[crate::ResolvedComponent<dyn Observer>],
) {
    for component in observers {
        let _ = owner
            .attach_observer(
                Arc::clone(component.handle()),
                observer_event_subscription(),
            )
            .await;
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
        same_identity_retry: SameIdentityRetryPolicy::default(),
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
