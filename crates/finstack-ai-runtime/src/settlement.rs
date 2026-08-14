//! Shared commit-settlement helpers for native and host-driven run owners.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use finstack_ai_kernel::{
    ActiveToolCallStatus, AllocatedIds, AppendBatchId, AppendBatchTag, AuthorizationEvidence,
    CancellationReconciledInput, CancellationRequestTag, ComponentId, ComponentRef, ContentBlock,
    Digest, EffectCompleted, EffectDeferred, EffectFailed, EffectId, EffectInput, EffectTag,
    ErrorCategory, EventId, EventTag, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, ExternalEffectCompletedInput, ExternalEffectCompletion,
    ExternalEffectOutcome, Id, IdTag, InteractionExpired, InteractionKind, InteractionRequest,
    InteractionSettled, InteractionTag, InteractionTerminalOutcome, KernelError, KernelInput,
    Message, MessageId, MessageRole, MessageTag, Metadata, ModelRef, ModelRequestTag, ModelSettled,
    ModelSettlement, ProviderIds, RawJson, RecordExternalCommandRejected, RecordId, RecordTag,
    ReducerStageOutcome, RequestInteraction, RunPhase, Stage, StageCursor, StageSettled, TextBlock,
    ToolBatchContinuation, ToolBatchSettled, ToolBatchTag, ToolCallBlock, ToolCallId, ToolCallPlan,
    ToolCallTag, ToolFailurePolicy, ToolSettlement, TransitionEnv, TurnTag, Version,
};

use crate::coordinator::{
    CommitCoordinator, CommitCoordinatorError, ModelDispatchSeed, ToolDispatchSeed,
};
use crate::run_types::RunHandleError;
use crate::tool::AssembledToolTerminal;
use crate::{
    CancellationSignal, Clock, IdGenerationError, InteractionResumeAction,
    LockedModelContextProfile, Model, ModelContextProfileOverride, ModelDeferral, ModelError,
    ModelProgress, ModelReconcileResult, ModelRequestDraft, ModelResponse, ModelResumeAction,
    ModelTerminal, PendingToolEffect, RandomSource, ReconcileContext, ResolvedToolCatalog,
    RunCallContext, ToolCatalogPlan, ToolDeferral, ToolError, ToolProgress, ToolReconcileResult,
    ToolResult, ToolResumeAction, UuidV7Generator, interaction_resume_action,
    map_model_reconcile_result, map_tool_reconcile_result, model_resume_action,
    model_retry_allowed, normalize_tool_result, resolve_model_context_profile, tool_resume_action,
    tool_retry_allowed,
};

pub(crate) struct ModelDriverResult {
    pub(crate) seed: ModelDispatchSeed,
    pub(crate) draft: ModelRequestDraft,
    pub(crate) provider: Arc<str>,
    pub(crate) result: Result<ModelTerminal, ModelError>,
}

pub(crate) struct ToolDriverResult {
    pub(crate) seed: ToolDispatchSeed,
    pub(crate) result: Result<AssembledToolTerminal, ToolError>,
}

// --- extracted from task.rs 787-862 ---
pub(crate) struct SettlementSources<C, R> {
    clock: Arc<C>,
    random: R,
    progress_random: ProgressRandom,
}

impl<C: Clock, R: RandomSource> SettlementSources<C, R> {
    pub(crate) fn try_new(clock: C, random: R) -> Result<Self, RunHandleError> {
        let progress_random = ProgressRandom::try_new(&random)?;
        Ok(Self {
            clock: Arc::new(clock),
            random,
            progress_random,
        })
    }

    pub(crate) fn now(&self) -> Result<finstack_ai_kernel::Timestamp, RunHandleError> {
        self.clock.now().map_err(id_source_error)
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn clock(&self) -> Arc<C> {
        Arc::clone(&self.clock)
    }

    pub(crate) fn generate<T: IdTag>(&self) -> Result<Id<T>, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.random)
            .generate()
            .map_err(id_source_error)
    }

    pub(crate) fn generate_progress_event(&self) -> Result<EventId, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.progress_random)
            .generate()
            .map_err(id_source_error)
    }
}

struct ProgressRandom {
    seed: [u8; 32],
    counter: AtomicU64,
}

impl ProgressRandom {
    fn try_new(random: &impl RandomSource) -> Result<Self, RunHandleError> {
        let mut seed = [0_u8; 32];
        random.fill_bytes(&mut seed).map_err(id_source_error)?;
        Ok(Self {
            seed,
            counter: AtomicU64::new(0),
        })
    }
}

impl RandomSource for ProgressRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let mut written = 0_usize;
        while written < bytes.len() {
            let counter = self
                .counter
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| {
                    IdGenerationError::Source("progress event entropy exhausted".into())
                })?;
            let mut material = [0_u8; 40];
            material[..32].copy_from_slice(&self.seed);
            material[32..].copy_from_slice(&counter.to_be_bytes());
            let digest = finstack_ai_kernel::Digest::raw_json(&material);
            let count = (bytes.len() - written).min(digest.as_bytes().len());
            bytes[written..written + count].copy_from_slice(&digest.as_bytes()[..count]);
            written += count;
        }
        Ok(())
    }
}

// --- extracted from task.rs 1088-1161 ---
pub(crate) async fn prepare_tool_batch_if_ready<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<C, R>,
) -> Result<bool, RunHandleError> {
    if coordinator.state().phase != Some(RunPhase::BeforeToolBatch) {
        return Ok(false);
    }
    let now = sources.now()?;
    if coordinator
        .state()
        .accepted
        .as_ref()
        .and_then(finstack_ai_kernel::RunAccepted::effective_deadline)
        .is_some_and(|deadline| now >= deadline)
    {
        fail_closed_on_run_deadline(coordinator, sources, now).await?;
        return Ok(false);
    }
    let state = coordinator.state();
    let source = state
        .messages
        .last()
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_source_message_missing",
        })?;
    let calls = source
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call)
                if !finstack_ai_kernel::is_internal_tool_name(call.tool_name()) =>
            {
                Some(call.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return Err(RunHandleError::ToolSettlement {
            code: "tool_source_calls_missing",
        });
    }
    let deadline = state
        .accepted
        .as_ref()
        .and_then(finstack_ai_kernel::RunAccepted::effective_deadline);
    let granted = approval_released_for_current_cursor(state);
    let refused = approval_refused_for_current_cursor(state);
    let mut plans = Vec::with_capacity(calls.len());
    for call in calls {
        match catalog.decide_plan(call, deadline, None, granted, refused) {
            ToolCatalogPlan::Ready(plan) => plans.push(plan),
            ToolCatalogPlan::RequireApproval => {
                request_approval_interaction(coordinator, sources).await?;
                return Ok(false);
            }
        }
    }
    let continuation = if state.final_result.is_some() {
        ToolBatchContinuation::Finalize
    } else {
        ToolBatchContinuation::ContinueModel
    };
    let input = KernelInput::StageSettled(StageSettled {
        cursor: StageCursor {
            cycle: state.cycle,
            stage: Stage::BeforeToolBatch,
        },
        outcome: ReducerStageOutcome::ToolBatchPrepared {
            calls: plans.clone().into(),
            continuation,
        },
    });
    let ids = allocate_tool_opening(&plans, sources)?;
    let env = TransitionEnv { now, ids };
    let decision =
        coordinator
            .classify(&env, input.clone())
            .map_err(|_| RunHandleError::ToolSettlement {
                code: "tool_opening_allocation_mismatch",
            })?;
    debug_assert_eq!(decision.records.len(), env.ids.record_ids().len());
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(true)
}

async fn fail_closed_on_run_deadline<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    now: finstack_ai_kernel::Timestamp,
) -> Result<(), RunHandleError> {
    let input = KernelInput::StageSettled(StageSettled {
        cursor: StageCursor {
            cycle: coordinator.state().cycle,
            stage: Stage::BeforeToolBatch,
        },
        outcome: ReducerStageOutcome::Continue,
    });
    let ids = AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(2, sources)?,
        generate_tool_ids::<EventTag, _, _>(2, sources)?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "deadline_ids_invalid",
    })?;
    let env = TransitionEnv { now, ids };
    coordinator
        .classify(&env, input.clone())
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "deadline_allocation_mismatch",
        })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

// --- extracted from task.rs 1225-1353 ---
pub(crate) async fn reconcile_cancelled_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    cancelled: bool,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let request_id = coordinator
        .state()
        .cancellation
        .as_ref()
        .ok_or(RunHandleError::CancellationSettlement {
            code: "cancellation_request_missing",
        })?
        .request
        .request_id;
    let (completed_effects, cancelled_effects) = if cancelled {
        (Arc::from([]), Arc::from([effect_id]))
    } else {
        (Arc::from([effect_id]), Arc::from([]))
    };
    let input = KernelInput::CancellationReconciled(CancellationReconciledInput {
        request_id,
        completed_effects,
        cancelled_effects,
        uncertain_effects: Arc::from([]),
    });
    let now = sources.now()?;
    let ids = allocate_for_runtime_input(coordinator, now, &input, sources)?;
    let outcome = coordinator
        .submit(TransitionEnv { now, ids }, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

#[derive(Default)]
struct RuntimeIdAllocation {
    records: Vec<RecordId>,
    events: Vec<EventId>,
    effects: Vec<Id<EffectTag>>,
    interactions: Vec<Id<InteractionTag>>,
    messages: Vec<MessageId>,
    turns: Vec<Id<TurnTag>>,
    model_requests: Vec<Id<ModelRequestTag>>,
    tool_batches: Vec<Id<ToolBatchTag>>,
    tool_calls: Vec<Id<ToolCallTag>>,
    cancellations: Vec<Id<CancellationRequestTag>>,
}

impl RuntimeIdAllocation {
    fn freeze(&self, append_batch_id: AppendBatchId) -> Result<AllocatedIds, RunHandleError> {
        AllocatedIds::try_new(
            self.records.clone(),
            self.events.clone(),
            self.effects.clone(),
            self.interactions.clone(),
            self.messages.clone(),
            self.turns.clone(),
            self.model_requests.clone(),
            self.tool_batches.clone(),
            self.tool_calls.clone(),
            vec![append_batch_id],
            self.cancellations.clone(),
        )
        .map_err(|_| RunHandleError::CancellationSettlement {
            code: "runtime_input_ids_invalid",
        })
    }
}

fn allocate_for_runtime_input<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    now: finstack_ai_kernel::Timestamp,
    input: &KernelInput,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let append_batch_id = sources.generate::<AppendBatchTag>()?;
    let mut allocation = RuntimeIdAllocation::default();
    for _ in 0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS {
        let ids = allocation.freeze(append_batch_id)?;
        match coordinator.classify(
            &TransitionEnv {
                now,
                ids: ids.clone(),
            },
            input.clone(),
        ) {
            Ok(_) => return Ok(ids),
            Err(KernelError::AllocatedIdsExhausted { kind }) => match kind {
                "record_ids" => allocation.records.push(sources.generate::<RecordTag>()?),
                "event_ids" => allocation.events.push(sources.generate::<EventTag>()?),
                "effect_ids" => allocation.effects.push(sources.generate::<EffectTag>()?),
                "interaction_ids" => allocation
                    .interactions
                    .push(sources.generate::<InteractionTag>()?),
                "message_ids" => allocation.messages.push(sources.generate::<MessageTag>()?),
                "turn_ids" => allocation.turns.push(sources.generate::<TurnTag>()?),
                "model_request_ids" => allocation
                    .model_requests
                    .push(sources.generate::<ModelRequestTag>()?),
                "tool_batch_ids" => allocation
                    .tool_batches
                    .push(sources.generate::<ToolBatchTag>()?),
                "tool_call_ids" => allocation
                    .tool_calls
                    .push(sources.generate::<ToolCallTag>()?),
                "cancellation_request_ids" => allocation
                    .cancellations
                    .push(sources.generate::<CancellationRequestTag>()?),
                _ => {
                    return Err(RunHandleError::CancellationSettlement {
                        code: "runtime_input_id_kind_unknown",
                    });
                }
            },
            Err(_) => {
                return Err(RunHandleError::CancellationSettlement {
                    code: "runtime_input_rejected",
                });
            }
        }
    }
    Err(RunHandleError::CancellationSettlement {
        code: "runtime_input_allocation_exhausted",
    })
}

// --- extracted from task.rs 1355-1990 ---
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToolOpeningCounts {
    records: usize,
    events: usize,
    effects: usize,
    messages: usize,
}

fn tool_opening_counts(plans: &[ToolCallPlan]) -> Result<ToolOpeningCounts, RunHandleError> {
    let mut groups = Vec::with_capacity(plans.len());
    let mut group = 0_u32;
    for (index, plan) in plans.iter().enumerate() {
        if index > 0
            && !(plans[index - 1].execution() == finstack_ai_kernel::ToolExecutionMode::Parallel
                && plan.execution() == finstack_ai_kernel::ToolExecutionMode::Parallel)
        {
            group = group.checked_add(1).ok_or(RunHandleError::ToolSettlement {
                code: "tool_group_count_overflow",
            })?;
        }
        groups.push(group);
    }
    let first_executable_group = plans
        .iter()
        .zip(&groups)
        .find_map(|(plan, group)| matches!(plan, ToolCallPlan::Execute(_)).then_some(*group));
    let requests = first_executable_group.map_or(0, |first| {
        plans
            .iter()
            .zip(&groups)
            .filter(|(plan, group)| **group == first && matches!(plan, ToolCallPlan::Execute(_)))
            .count()
    });
    let messages = plans
        .iter()
        .take_while(|plan| matches!(plan, ToolCallPlan::SyntheticClosure(_)))
        .count();
    Ok(ToolOpeningCounts {
        records: 2 + requests + messages + usize::from(first_executable_group.is_none()),
        events: requests + 2 * messages,
        effects: plans.len(),
        messages,
    })
}

fn approval_cursor(state: &finstack_ai_kernel::KernelState) -> StageCursor {
    StageCursor {
        cycle: state.cycle,
        stage: Stage::BeforeToolBatch,
    }
}

fn approval_released_for_current_cursor(state: &finstack_ai_kernel::KernelState) -> bool {
    state
        .last_interaction_terminal
        .as_ref()
        .is_some_and(|terminal| {
            terminal.kind == InteractionKind::Approval
                && terminal.outcome == InteractionTerminalOutcome::Granted
                && terminal.cursor == approval_cursor(state)
        })
}

fn approval_refused_for_current_cursor(state: &finstack_ai_kernel::KernelState) -> bool {
    state
        .last_interaction_terminal
        .as_ref()
        .is_some_and(|terminal| {
            terminal.kind == InteractionKind::Approval
                && matches!(
                    terminal.outcome,
                    InteractionTerminalOutcome::Denied
                        | InteractionTerminalOutcome::Expired
                        | InteractionTerminalOutcome::Cancelled
                )
                && terminal.cursor == approval_cursor(state)
        })
}

fn approval_schema() -> Result<RawJson, RunHandleError> {
    RawJson::parse(
        r#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "approval_schema_invalid",
    })
}

async fn request_approval_interaction<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let now = sources.now()?;
    let interaction_id = generate_tool_id::<InteractionTag, _, _>(sources)?;
    let effect_id = generate_tool_id::<EffectTag, _, _>(sources)?;
    let expires_at = coordinator
        .state()
        .accepted
        .as_ref()
        .and_then(finstack_ai_kernel::RunAccepted::effective_deadline);
    let request = InteractionRequest::try_new(
        1,
        interaction_id,
        effect_id,
        InteractionKind::Approval,
        vec![ContentBlock::Text(
            TextBlock::try_new("approve the next tool action").map_err(|_| {
                RunHandleError::InteractionSettlement {
                    code: "approval_prompt_invalid",
                }
            })?,
        )],
        approval_schema()?,
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").map_err(|_| {
                RunHandleError::InteractionSettlement {
                    code: "approval_policy_invalid",
                }
            })?,
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        expires_at,
        false,
        Metadata::empty(),
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "approval_request_invalid",
    })?;
    let input = KernelInput::RequestInteraction(RequestInteraction { request });
    let ids = AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(2, sources)?,
        generate_tool_ids::<EventTag, _, _>(2, sources)?,
        vec![effect_id],
        vec![interaction_id],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "interaction_request_ids_invalid",
    })?;
    let env = TransitionEnv { now, ids };
    coordinator.classify(&env, input.clone()).map_err(|_| {
        RunHandleError::InteractionSettlement {
            code: "interaction_request_allocation_mismatch",
        }
    })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

pub(crate) async fn apply_interaction_resume<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
) -> Result<InteractionResumeAction, RunHandleError> {
    let now = sources.now()?;
    let action = interaction_resume_action(coordinator.state(), now);
    if action != InteractionResumeAction::ExpireIfDue {
        return Ok(action);
    }
    let pending = coordinator.state().pending_interaction.as_ref().ok_or(
        RunHandleError::InteractionSettlement {
            code: "interaction_pending_missing",
        },
    )?;
    let input = KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
        interaction_id: pending.request.interaction_id(),
        expired_at: now,
    }));
    let ids = AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(2, sources)?,
        generate_tool_ids::<EventTag, _, _>(2, sources)?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "interaction_expire_ids_invalid",
    })?;
    let env = TransitionEnv { now, ids };
    coordinator.classify(&env, input.clone()).map_err(|_| {
        RunHandleError::InteractionSettlement {
            code: "interaction_expire_allocation_mismatch",
        }
    })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(action)
}

fn allocate_tool_opening<C: Clock, R: RandomSource>(
    plans: &[ToolCallPlan],
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let counts = tool_opening_counts(plans)?;
    AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(counts.records, sources)?,
        generate_tool_ids::<EventTag, _, _>(counts.events, sources)?,
        generate_tool_ids::<EffectTag, _, _>(counts.effects, sources)?,
        Vec::new(),
        generate_tool_ids::<MessageTag, _, _>(counts.messages, sources)?,
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<ToolBatchTag, _, _>(sources)?],
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_opening_ids_invalid",
    })
}

pub(crate) async fn process_tool_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    mut driver_result: ToolDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let effect_id = driver_result.seed.requested.effect_id();
    let state = coordinator.state();
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(());
    };
    let Some(active) = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
        return Ok(());
    };
    if let Some(cancellation) = state.cancellation.as_ref()
        && cancellation.outstanding_effects.contains(&effect_id)
    {
        let cancelled = driver_result
            .result
            .as_ref()
            .is_err_and(|error| error.category() == ErrorCategory::Cancellation);
        return reconcile_cancelled_effect(coordinator, effect_id, cancelled, sources).await;
    }
    if batch.opened.tool_batch_id != driver_result.seed.tool_batch_id
        || !matches!(
            active.status,
            finstack_ai_kernel::ActiveToolCallStatus::Requested { deferred: None, .. }
        )
        || state.terminal.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    if driver_result
        .seed
        .requested
        .deadline()
        .is_some_and(|deadline| deadline <= now)
    {
        driver_result.result = Err(ToolError::try_new(
            crate::TOOL_DEADLINE_EXCEEDED,
            ErrorCategory::Deadline,
            false,
            "tool result arrived after the committed deadline",
            Metadata::empty(),
        )
        .map_err(|error| tool_handle_error(&error))?);
    }
    let settled = build_tool_settlement(driver_result)?;
    let input = KernelInput::ToolBatchSettled(settled.clone());
    let allocation = allocate_tool_settlement(coordinator.state(), &settled, sources)?;
    let env = TransitionEnv {
        now,
        ids: allocation,
    };
    coordinator
        .classify(&env, input.clone())
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_settlement_allocation_mismatch",
        })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

pub(crate) async fn process_tool_progress<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    progress: ToolProgress,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let state = coordinator.state();
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(());
    };
    let Some(active) = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
        return Ok(());
    };
    if !matches!(
        active.status,
        finstack_ai_kernel::ActiveToolCallStatus::Requested { deferred: None, .. }
    ) || state.terminal.is_some()
        || state.cancellation.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    let event_id = sources.generate_progress_event()?;
    let event = coordinator
        .materialize_tool_progress(&progress, event_id, effect_id, now)
        .map_err(|code| RunHandleError::ToolSettlement { code })?;
    coordinator
        .publish_events(Arc::from([event]))
        .await
        .map_err(RunHandleError::Coordinator)
}

fn build_tool_settlement(result: ToolDriverResult) -> Result<ToolBatchSettled, RunHandleError> {
    let requested = &result.seed.requested;
    let effect_id = requested.effect_id();
    let outcome = match result.result {
        Ok(assembled) => {
            let block = normalize_tool_result(result.seed.tool_call_id, assembled.result)
                .map_err(|error| tool_handle_error(&error))?;
            let bytes = serde_json_canonicalizer::to_vec(&block).map_err(|_| {
                RunHandleError::ToolSettlement {
                    code: "tool_result_serialize_failed",
                }
            })?;
            let output = RawJson::parse(bytes).map_err(|_| RunHandleError::ToolSettlement {
                code: "tool_result_output_invalid",
            })?;
            ToolSettlement::Completed(
                EffectCompleted::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    output,
                    assembled.usage,
                    Vec::new(),
                    ProviderIds::empty(),
                    None::<&str>,
                    None,
                )
                .map_err(|_| RunHandleError::ToolSettlement {
                    code: "tool_effect_completion_invalid",
                })?,
            )
        }
        Err(error) => {
            let descriptor = error
                .to_descriptor()
                .map_err(|error| tool_handle_error(&error))?;
            ToolSettlement::Failed(
                EffectFailed::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    descriptor,
                    None,
                    None::<&str>,
                )
                .map_err(|_| RunHandleError::ToolSettlement {
                    code: "tool_effect_failure_invalid",
                })?,
            )
        }
    };
    Ok(ToolBatchSettled {
        tool_batch_id: result.seed.tool_batch_id,
        outcome,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredictedToolStatus {
    Undispatched,
    Requested,
    Buffered,
    Settled,
}

#[expect(
    clippy::too_many_lines,
    reason = "exact allocation mirrors the kernel's contiguous-prefix and next-group cardinalities"
)]
fn allocate_tool_settlement<C: Clock, R: RandomSource>(
    state: &finstack_ai_kernel::KernelState,
    settled: &ToolBatchSettled,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_settlement_batch_missing",
        })?;
    let effect_id = match &settled.outcome {
        ToolSettlement::Completed(value) => value.effect_id(),
        ToolSettlement::Failed(value) => value.effect_id(),
        ToolSettlement::Deferred(value) => value.effect_id,
    };
    let target = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == effect_id)
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_settlement_call_missing",
        })?;
    let mut statuses = batch
        .calls
        .iter()
        .map(|call| match call.status {
            finstack_ai_kernel::ActiveToolCallStatus::Undispatched => {
                PredictedToolStatus::Undispatched
            }
            finstack_ai_kernel::ActiveToolCallStatus::Requested { .. } => {
                PredictedToolStatus::Requested
            }
            finstack_ai_kernel::ActiveToolCallStatus::Buffered { .. } => {
                PredictedToolStatus::Buffered
            }
            finstack_ai_kernel::ActiveToolCallStatus::Settled { .. } => {
                PredictedToolStatus::Settled
            }
        })
        .collect::<Vec<_>>();
    statuses[target] = PredictedToolStatus::Buffered;
    let target_plan = &batch.calls[target].assigned.plan;
    let fatal = batch.fatal_error.is_some()
        || (matches!(settled.outcome, ToolSettlement::Failed(_))
            && target_plan.failure_policy() == ToolFailurePolicy::FailRun);
    let current_complete = batch.calls.iter().enumerate().all(|(index, call)| {
        call.assigned.group_index != batch.current_group
            || matches!(
                statuses[index],
                PredictedToolStatus::Buffered | PredictedToolStatus::Settled
            )
    });
    if fatal && current_complete {
        for status in &mut statuses {
            if *status == PredictedToolStatus::Undispatched {
                *status = PredictedToolStatus::Buffered;
            }
        }
    }
    let mut messages = 0_usize;
    let start =
        usize::try_from(batch.next_source_index).map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_source_index_invalid",
        })?;
    for status in statuses.iter_mut().skip(start) {
        if *status != PredictedToolStatus::Buffered {
            break;
        }
        *status = PredictedToolStatus::Settled;
        messages += 1;
    }
    let requests = if !fatal && current_complete {
        let next_group = batch.calls.iter().enumerate().find_map(|(index, call)| {
            (statuses[index] == PredictedToolStatus::Undispatched
                && matches!(call.assigned.plan, ToolCallPlan::Execute(_)))
            .then_some(call.assigned.group_index)
        });
        next_group.map_or(0, |group| {
            batch
                .calls
                .iter()
                .enumerate()
                .filter(|(index, call)| {
                    statuses[*index] == PredictedToolStatus::Undispatched
                        && call.assigned.group_index == group
                        && matches!(call.assigned.plan, ToolCallPlan::Execute(_))
                })
                .count()
        })
    } else {
        0
    };
    let close = usize::from(
        statuses
            .iter()
            .all(|status| *status == PredictedToolStatus::Settled),
    );
    let records = 1 + messages + requests + close;
    let events = 1 + 2 * messages + requests;
    AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(records, sources)?,
        generate_tool_ids::<EventTag, _, _>(events, sources)?,
        Vec::new(),
        Vec::new(),
        generate_tool_ids::<MessageTag, _, _>(messages, sources)?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_settlement_ids_invalid",
    })
}

fn generate_tool_ids<T: IdTag, C: Clock, R: RandomSource>(
    count: usize,
    sources: &SettlementSources<C, R>,
) -> Result<Vec<Id<T>>, RunHandleError> {
    (0..count)
        .map(|_| generate_tool_id::<T, _, _>(sources))
        .collect()
}

fn generate_tool_id<T: IdTag, C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<Id<T>, RunHandleError> {
    sources
        .generate::<T>()
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_settlement_id_source_failed",
        })
}

pub(crate) async fn resume_pending_model_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<ModelResumeAction, RunHandleError> {
    let first_pass = model_resume_action(coordinator.state());
    match first_pass {
        ModelResumeAction::NoOutstanding
        | ModelResumeAction::UseRecorded
        | ModelResumeAction::WaitExternal => return Ok(first_pass),
        ModelResumeAction::Retry | ModelResumeAction::SuspendUncertain => {
            return Ok(first_pass);
        }
        ModelResumeAction::Reconcile => {}
    }
    let Some(seed) = coordinator.pending_model_seed() else {
        return Err(RunHandleError::ModelSettlement {
            code: "model_resume_seed_missing",
        });
    };
    let draft = pending_draft(&seed)?;
    let retry_allowed =
        model_retry_allowed(&seed.pending.requested, &model.capabilities(&draft.model));
    let context = ReconcileContext {
        run: RunCallContext {
            locator: seed.locator.clone(),
            authorization: seed.authorization.clone(),
            effect_id: seed.pending.requested.effect_id(),
            attempt: seed.attempt,
            deadline: seed.pending.requested.deadline(),
            budget_scope_id: seed.budget_scope_id,
            cancellation: cancellation.child(),
        },
        original_input_digest: seed.pending.requested.input_digest(),
    };
    let Ok(result) = model.reconcile(context, seed.pending.clone()).await else {
        return Ok(ModelResumeAction::SuspendUncertain);
    };
    apply_model_reconcile_result(
        coordinator,
        model,
        seed,
        draft,
        result,
        retry_allowed,
        sources,
    )
    .await
}

pub(crate) async fn resume_pending_tool_effects<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<ToolResumeAction, RunHandleError> {
    let Some(batch) = coordinator.state().active_tool_batch.clone() else {
        return Ok(ToolResumeAction::NoOutstanding);
    };
    let mut first_pass = Vec::new();
    let mut to_reconcile = Vec::new();
    for call in batch.calls.iter() {
        let effect_id = call.assigned.effect_id;
        let action = tool_resume_action(coordinator.state(), effect_id);
        first_pass.push(action);
        if action == ToolResumeAction::Reconcile {
            to_reconcile.push(effect_id);
        }
    }
    if to_reconcile.is_empty() {
        return Ok(aggregate_tool_resume_actions(&first_pass));
    }
    let seeds = coordinator.pending_tool_seeds();
    let mut mapped = Vec::new();
    for effect_id in to_reconcile {
        let Some(seed) = seeds
            .iter()
            .find(|seed| seed.requested.effect_id() == effect_id)
            .cloned()
            .or_else(|| tool_seed_for_deferred(coordinator, effect_id))
        else {
            return Ok(ToolResumeAction::SuspendUncertain);
        };
        let Some(resolved) = catalog.by_id(&seed.call.tool_id) else {
            return Ok(ToolResumeAction::SuspendUncertain);
        };
        let retry_allowed = tool_retry_allowed(&seed.requested, &resolved.spec);
        let context = ReconcileContext {
            run: RunCallContext {
                locator: seed.locator.clone(),
                authorization: seed.authorization.clone(),
                effect_id,
                attempt: seed.attempt,
                deadline: seed.requested.deadline(),
                budget_scope_id: seed.budget_scope_id,
                cancellation: cancellation.child(),
            },
            original_input_digest: seed.requested.input_digest(),
        };
        let Ok(result) = resolved
            .toolset
            .reconcile(
                context,
                PendingToolEffect {
                    call: seed.call.clone(),
                },
            )
            .await
        else {
            return Ok(ToolResumeAction::SuspendUncertain);
        };
        let action =
            map_tool_reconcile_result(coordinator.state(), effect_id, &result, retry_allowed);
        mapped.push((seed, result, action));
    }
    if mapped
        .iter()
        .any(|(_, _, action)| *action == ToolResumeAction::SuspendUncertain)
    {
        return Ok(ToolResumeAction::SuspendUncertain);
    }
    let mut applied = Vec::new();
    for (seed, result, mapped_action) in &mapped {
        let action = apply_tool_reconcile_result(coordinator, seed, result, sources).await?;
        if action == ToolResumeAction::SuspendUncertain {
            return Ok(ToolResumeAction::SuspendUncertain);
        }
        applied.push(*mapped_action);
    }
    let mut actions = first_pass
        .into_iter()
        .filter(|action| *action != ToolResumeAction::Reconcile)
        .collect::<Vec<_>>();
    actions.extend(applied);
    Ok(aggregate_tool_resume_actions(&actions))
}

fn aggregate_tool_resume_actions(actions: &[ToolResumeAction]) -> ToolResumeAction {
    if actions.contains(&ToolResumeAction::SuspendUncertain) {
        return ToolResumeAction::SuspendUncertain;
    }
    if actions.contains(&ToolResumeAction::Retry) {
        return ToolResumeAction::Retry;
    }
    if actions.contains(&ToolResumeAction::WaitExternal) {
        return ToolResumeAction::WaitExternal;
    }
    if actions.contains(&ToolResumeAction::UseRecorded) {
        return ToolResumeAction::UseRecorded;
    }
    if actions.contains(&ToolResumeAction::Reconcile) {
        return ToolResumeAction::Reconcile;
    }
    ToolResumeAction::NoOutstanding
}

fn tool_seed_for_deferred(
    coordinator: &CommitCoordinator,
    effect_id: EffectId,
) -> Option<ToolDispatchSeed> {
    coordinator
        .pending_tool_seeds()
        .into_iter()
        .find(|seed| seed.requested.effect_id() == effect_id)
        .or_else(|| deferred_tool_seed(coordinator, effect_id))
}

fn deferred_tool_seed(
    coordinator: &CommitCoordinator,
    effect_id: EffectId,
) -> Option<ToolDispatchSeed> {
    let state = coordinator.state();
    let batch = state.active_tool_batch.as_ref()?;
    let call = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)?;
    let ActiveToolCallStatus::Requested { requested, .. } = &call.status else {
        return None;
    };
    let ToolCallPlan::Execute(validated) = &call.assigned.plan else {
        return None;
    };
    let (locator, authorization, budget_scope_id) = dispatch_security_from_state(state)?;
    Some(ToolDispatchSeed {
        requested: requested.clone(),
        tool_batch_id: batch.opened.tool_batch_id,
        tool_call_id: *validated.call.tool_call_id(),
        call: validated.clone(),
        locator,
        authorization,
        budget_scope_id,
        attempt: 1,
        requested_at: state.accepted_at?,
    })
}

fn dispatch_security_from_state(
    state: &finstack_ai_kernel::KernelState,
) -> Option<(
    finstack_ai_kernel::OperationLocator,
    crate::AuthorizationContext,
    Option<finstack_ai_kernel::BudgetScopeId>,
)> {
    let accepted = state.accepted.as_ref()?;
    let security = accepted.security();
    let locator = finstack_ai_kernel::OperationLocator::try_new(
        security.tenant_scope(),
        state.session_id?,
        state.lane_id?,
        accepted.run_id(),
    )
    .ok()?;
    Some((
        locator,
        crate::AuthorizationContext {
            principal: security.principal().clone(),
            authentication_method: Arc::from(security.authentication_method()),
            assurance_level: Arc::from(security.assurance_level()),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([Arc::from(security.tenant_scope())]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from(security.authorization_policy_version()),
            decision_id: Arc::from(security.authorization_decision_id()),
        },
        accepted.relation().budget_scope_id(),
    ))
}

async fn apply_tool_reconcile_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: &ToolDispatchSeed,
    result: &ToolReconcileResult,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    match result {
        ToolReconcileResult::Completed(tool_result) => {
            settle_reconciled_tool(coordinator, seed.clone(), tool_result.clone(), sources).await
        }
        ToolReconcileResult::Deferred(deferral) | ToolReconcileResult::StillRunning(deferral) => {
            ensure_or_wait_tool_deferred(coordinator, seed, deferral, sources).await
        }
        ToolReconcileResult::NotStarted
        | ToolReconcileResult::RetrySafe
        | ToolReconcileResult::Unknown
        | ToolReconcileResult::NonRepeatable => Ok(ToolResumeAction::NoOutstanding),
    }
}

async fn settle_reconciled_tool<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: ToolDispatchSeed,
    result: ToolResult,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    let effect_id = seed.requested.effect_id();
    if coordinator
        .state()
        .tool_settlements
        .contains_key(&effect_id)
    {
        return Ok(ToolResumeAction::UseRecorded);
    }
    let deferred = coordinator
        .state()
        .active_tool_batch
        .as_ref()
        .is_some_and(|batch| {
            batch.calls.iter().any(|call| {
                call.assigned.effect_id == effect_id
                    && matches!(
                        call.status,
                        ActiveToolCallStatus::Requested {
                            deferred: Some(_),
                            ..
                        }
                    )
            })
        });
    let driver = ToolDriverResult {
        seed,
        result: Ok(AssembledToolTerminal {
            usage: None,
            result,
        }),
    };
    if deferred {
        return settle_external_tool(coordinator, driver, sources).await;
    }
    process_tool_result(coordinator, driver, sources).await?;
    Ok(ToolResumeAction::UseRecorded)
}

async fn settle_external_tool<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: ToolDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    let effect_id = driver.seed.requested.effect_id();
    let settled = build_tool_settlement(driver)?;
    let ToolSettlement::Completed(completion) = &settled.outcome else {
        return Err(RunHandleError::ToolSettlement {
            code: "tool_external_completion_invalid",
        });
    };
    let completion_id = effect_id.to_canonical_string();
    let input = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            effect_id,
            &completion_id,
            ExternalEffectOutcome::Completed {
                output: completion.output().clone(),
                usage: completion.usage().cloned(),
                artifacts: Arc::from(completion.artifacts()),
            },
        )
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_external_completion_invalid",
        })?,
        assistant_message: None,
    });
    let now = sources.now()?;
    let allocation = allocate_tool_settlement(coordinator.state(), &settled, sources)?;
    match coordinator
        .submit(
            TransitionEnv {
                now,
                ids: allocation,
            },
            input,
        )
        .await
    {
        Ok(outcome) => {
            if let Some(fault) = outcome.fault {
                return Err(RunHandleError::Faulted { code: fault.code });
            }
            Ok(ToolResumeAction::UseRecorded)
        }
        Err(CommitCoordinatorError::Decision {
            code:
                "conflicting_completion_id" | "conflicting_settlement" | "settlement_digest_mismatch",
        }) => {
            submit_tool_fail_closed(coordinator, effect_id, &completion_id, sources).await?;
            Ok(ToolResumeAction::SuspendUncertain)
        }
        Err(error) => Err(RunHandleError::Coordinator(error)),
    }
}

async fn ensure_or_wait_tool_deferred<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: &ToolDispatchSeed,
    deferral: &ToolDeferral,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    let effect_id = seed.requested.effect_id();
    let existing = coordinator
        .state()
        .active_tool_batch
        .as_ref()
        .and_then(|batch| {
            batch.calls.iter().find_map(|call| {
                (call.assigned.effect_id == effect_id)
                    .then_some(call.status.clone())
                    .and_then(|status| match status {
                        ActiveToolCallStatus::Requested {
                            deferred: Some(deferred),
                            ..
                        } => Some(deferred),
                        _ => None,
                    })
            })
        });
    if let Some(existing) = existing {
        if existing.handle == deferral.handle {
            return Ok(ToolResumeAction::WaitExternal);
        }
        submit_tool_fail_closed(coordinator, effect_id, deferral.handle.handle(), sources).await?;
        return Ok(ToolResumeAction::SuspendUncertain);
    }
    let settled = ToolBatchSettled {
        tool_batch_id: seed.tool_batch_id,
        outcome: ToolSettlement::Deferred(EffectDeferred {
            effect_id,
            handle: deferral.handle.clone(),
            reconciliation: deferral.reconciliation,
            next_poll_at: deferral.next_poll_at,
            expires_at: deferral.expires_at,
            output_contract: seed.requested.output_contract().clone(),
        }),
    };
    submit_resume_input(coordinator, KernelInput::ToolBatchSettled(settled), sources).await?;
    Ok(ToolResumeAction::WaitExternal)
}

async fn submit_tool_fail_closed<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    command_id: &str,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let accepted = coordinator
        .state()
        .accepted
        .as_ref()
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_resume_accepted_missing",
        })?;
    let security = accepted.security();
    let locator = coordinator
        .pending_tool_seeds()
        .into_iter()
        .find(|seed| seed.requested.effect_id() == effect_id)
        .or_else(|| deferred_tool_seed(coordinator, effect_id))
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_resume_seed_missing",
        })?
        .locator;
    let accepted_digest = coordinator
        .state()
        .tool_settlements
        .get(&effect_id)
        .map(|fingerprint| fingerprint.digest);
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        command_id,
        ExternalCommandTarget::Effect(effect_id),
        accepted.security().principal().clone(),
        AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_resume_authorization_invalid",
        })?,
        "conflicting_or_invalid_completion",
        Digest::raw_json(command_id.as_bytes()),
        accepted_digest,
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_resume_rejection_invalid",
    })?;
    submit_resume_input(
        coordinator,
        KernelInput::RecordExternalCommandRejected(RecordExternalCommandRejected {
            locator,
            rejection,
        }),
        sources,
    )
    .await
}

async fn apply_model_reconcile_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    result: ModelReconcileResult,
    retry_allowed: bool,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    let action = map_model_reconcile_result(coordinator.state(), &result, retry_allowed);
    match result {
        ModelReconcileResult::Completed(response) => {
            settle_reconciled_completion(coordinator, model, seed, draft, response, sources).await
        }
        ModelReconcileResult::Deferred(deferral) | ModelReconcileResult::StillRunning(deferral) => {
            ensure_or_wait_deferred(coordinator, model, seed, draft, deferral, sources).await
        }
        ModelReconcileResult::NotStarted
        | ModelReconcileResult::RetrySafe
        | ModelReconcileResult::Unknown
        | ModelReconcileResult::NonRepeatable => Ok(action),
    }
}

async fn settle_reconciled_completion<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    response: ModelResponse,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    if coordinator
        .state()
        .model_settlements
        .contains_key(&seed.pending.requested.effect_id())
    {
        return Ok(ModelResumeAction::UseRecorded);
    }
    if coordinator.state().phase == Some(RunPhase::AwaitingExternal) {
        return settle_external_model(coordinator, model, seed, draft, response, sources).await;
    }
    let driver = ModelDriverResult {
        seed,
        draft,
        provider: model.descriptor().provider,
        result: Ok(ModelTerminal::Completed(response)),
    };
    process_model_result(coordinator, driver, sources).await?;
    Ok(ModelResumeAction::UseRecorded)
}

async fn ensure_or_wait_deferred<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    deferral: ModelDeferral,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    if let Some(existing) = seed.pending.deferred.as_ref() {
        if existing.handle == deferral.handle {
            return Ok(ModelResumeAction::WaitExternal);
        }
        return submit_fail_closed(
            coordinator,
            &seed,
            deferral.handle.handle(),
            "conflicting_or_invalid_completion",
            sources,
        )
        .await;
    }
    let driver = ModelDriverResult {
        seed,
        draft,
        provider: model.descriptor().provider,
        result: Ok(ModelTerminal::Deferred(deferral)),
    };
    process_model_result(coordinator, driver, sources).await?;
    Ok(ModelResumeAction::WaitExternal)
}

async fn settle_external_model<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    response: ModelResponse,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    let driver = ModelDriverResult {
        seed: seed.clone(),
        draft,
        provider: model.descriptor().provider,
        result: Ok(ModelTerminal::Completed(response.clone())),
    };
    let now = sources.now()?;
    let allocation = allocate_settlement(&driver, sources)?;
    let settled = build_settlement(driver, now, &allocation)?;
    let ModelSettlement::Completed {
        completion,
        assistant_message,
    } = settled.outcome
    else {
        return Err(RunHandleError::ModelSettlement {
            code: "model_external_completion_invalid",
        });
    };
    let completion_id = completion
        .completion_id()
        .ok_or(RunHandleError::ModelSettlement {
            code: "model_completion_id_missing",
        })?
        .to_owned();
    let input = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            seed.pending.requested.effect_id(),
            &completion_id,
            ExternalEffectOutcome::Completed {
                output: completion.output().clone(),
                usage: completion.usage().cloned(),
                artifacts: Arc::from(completion.artifacts()),
            },
        )
        .map_err(|_| RunHandleError::ModelSettlement {
            code: "model_external_completion_invalid",
        })?,
        assistant_message: Some(assistant_message),
    });
    match coordinator
        .submit(
            TransitionEnv {
                now,
                ids: allocation.ids,
            },
            input,
        )
        .await
    {
        Ok(outcome) => {
            if let Some(fault) = outcome.fault {
                return Err(RunHandleError::Faulted { code: fault.code });
            }
            Ok(ModelResumeAction::UseRecorded)
        }
        Err(CommitCoordinatorError::Decision {
            code:
                "conflicting_completion_id" | "conflicting_settlement" | "settlement_digest_mismatch",
        }) => {
            submit_fail_closed(
                coordinator,
                &seed,
                &completion_id,
                "conflicting_or_invalid_completion",
                sources,
            )
            .await
        }
        Err(error) => Err(RunHandleError::Coordinator(error)),
    }
}

async fn submit_fail_closed<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: &ModelDispatchSeed,
    command_id: &str,
    reason_code: &'static str,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    let accepted =
        coordinator
            .state()
            .accepted
            .as_ref()
            .ok_or(RunHandleError::ModelSettlement {
                code: "model_resume_accepted_missing",
            })?;
    let security = accepted.security();
    let effect_id = seed.pending.requested.effect_id();
    let accepted_digest = coordinator
        .state()
        .model_settlements
        .get(&effect_id)
        .map(|fingerprint| fingerprint.digest);
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        command_id,
        ExternalCommandTarget::Effect(effect_id),
        seed.authorization.principal.clone(),
        AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .map_err(|_| RunHandleError::ModelSettlement {
            code: "model_resume_authorization_invalid",
        })?,
        reason_code,
        Digest::raw_json(command_id.as_bytes()),
        accepted_digest,
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_resume_rejection_invalid",
    })?;
    let input = KernelInput::RecordExternalCommandRejected(RecordExternalCommandRejected {
        locator: seed.locator.clone(),
        rejection,
    });
    submit_resume_input(coordinator, input, sources).await?;
    Ok(ModelResumeAction::SuspendUncertain)
}

async fn submit_resume_input<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    input: KernelInput,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let now = sources.now()?;
    let ids = allocate_resume_input(coordinator, now, &input, sources)?;
    let outcome = coordinator
        .submit(TransitionEnv { now, ids }, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

fn allocate_resume_input<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    now: finstack_ai_kernel::Timestamp,
    input: &KernelInput,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let append_batch_id = sources.generate::<AppendBatchTag>()?;
    let mut allocation = RuntimeIdAllocation::default();
    for _ in 0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS {
        let ids = allocation.freeze(append_batch_id)?;
        match coordinator.classify(
            &TransitionEnv {
                now,
                ids: ids.clone(),
            },
            input.clone(),
        ) {
            Ok(_) => return Ok(ids),
            Err(KernelError::AllocatedIdsExhausted { kind }) => match kind {
                "record_ids" => allocation.records.push(sources.generate::<RecordTag>()?),
                "event_ids" => allocation.events.push(sources.generate::<EventTag>()?),
                "effect_ids" => allocation.effects.push(sources.generate::<EffectTag>()?),
                "interaction_ids" => allocation
                    .interactions
                    .push(sources.generate::<InteractionTag>()?),
                "message_ids" => allocation.messages.push(sources.generate::<MessageTag>()?),
                "turn_ids" => allocation.turns.push(sources.generate::<TurnTag>()?),
                "model_request_ids" => allocation
                    .model_requests
                    .push(sources.generate::<ModelRequestTag>()?),
                "tool_batch_ids" => allocation
                    .tool_batches
                    .push(sources.generate::<ToolBatchTag>()?),
                "tool_call_ids" => allocation
                    .tool_calls
                    .push(sources.generate::<ToolCallTag>()?),
                "cancellation_request_ids" => allocation
                    .cancellations
                    .push(sources.generate::<CancellationRequestTag>()?),
                _ => {
                    return Err(RunHandleError::ModelSettlement {
                        code: "runtime_input_id_kind_unknown",
                    });
                }
            },
            Err(error) => {
                return Err(RunHandleError::ModelSettlement { code: error.code() });
            }
        }
    }
    Err(RunHandleError::ModelSettlement {
        code: "runtime_input_allocation_exhausted",
    })
}

fn pending_draft(seed: &ModelDispatchSeed) -> Result<ModelRequestDraft, RunHandleError> {
    let EffectInput::Model { request } = seed.pending.requested.input() else {
        return Err(RunHandleError::ModelSettlement {
            code: "model_request_invalid",
        });
    };
    serde_json::from_slice(request.as_bytes()).map_err(|_| RunHandleError::ModelSettlement {
        code: "model_request_invalid",
    })
}

pub(crate) async fn process_model_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    mut driver_result: ModelDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let effect_id = driver_result.seed.pending.requested.effect_id();
    let state = coordinator.state();
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(());
    };
    if let Some(cancellation) = state.cancellation.as_ref()
        && cancellation.outstanding_effects.contains(&effect_id)
    {
        let cancelled = driver_result
            .result
            .as_ref()
            .is_err_and(|error| error.category() == ErrorCategory::Cancellation);
        return reconcile_cancelled_effect(coordinator, effect_id, cancelled, sources).await;
    }
    if pending.requested.effect_id() != effect_id
        || pending.model_request_id != driver_result.seed.pending.model_request_id
        || pending.deferred.is_some()
        || state.terminal.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    if pending
        .requested
        .deadline()
        .is_some_and(|deadline| deadline <= now)
    {
        driver_result.result = Err(ModelError::try_new(
            "model_deadline_exceeded",
            ErrorCategory::Deadline,
            false,
            "model result arrived after the committed deadline",
            Metadata::empty(),
        )
        .map_err(|error| model_handle_error(&error))?);
    }
    let allocation = allocate_settlement(&driver_result, sources)?;
    let settled = build_settlement(driver_result, now, &allocation)?;
    let outcome = coordinator
        .submit(
            TransitionEnv {
                now,
                ids: allocation.ids,
            },
            KernelInput::ModelSettled(settled),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

pub(crate) async fn process_model_progress<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    provider: &str,
    progress: ModelProgress,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let state = coordinator.state();
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(());
    };
    if pending.requested.effect_id() != effect_id
        || pending.deferred.is_some()
        || state.terminal.is_some()
        || state.cancellation.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    let event_id = sources.generate_progress_event()?;
    let event = coordinator
        .materialize_model_progress(&progress, event_id, provider, now)
        .map_err(|code| RunHandleError::ModelSettlement { code })?;
    coordinator
        .publish_events(Arc::from([event]))
        .await
        .map_err(RunHandleError::Coordinator)
}

struct SettlementAllocation {
    ids: AllocatedIds,
    message_id: Option<MessageId>,
    tool_call_ids: Vec<ToolCallId>,
}

fn allocate_settlement<C: Clock, R: RandomSource>(
    result: &ModelDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<SettlementAllocation, RunHandleError> {
    let completed = matches!(result.result, Ok(ModelTerminal::Completed(_)));
    let tool_count = match &result.result {
        Ok(value) => match value {
            ModelTerminal::Completed(response) => response.tool_calls.len(),
            ModelTerminal::Deferred(_) => 0,
        },
        Err(_) => 0,
    };
    let record_count = if completed { 2 } else { 1 };
    let event_count = if completed { 2 } else { 1 };
    let records = (0..record_count)
        .map(|_| sources.generate::<RecordTag>())
        .collect::<Result<Vec<RecordId>, _>>()?;
    let events = (0..event_count)
        .map(|_| sources.generate::<EventTag>())
        .collect::<Result<Vec<EventId>, _>>()?;
    let message_id = completed
        .then(|| sources.generate::<MessageTag>())
        .transpose()?;
    let tool_call_ids = (0..tool_count)
        .map(|_| sources.generate::<ToolCallTag>())
        .collect::<Result<Vec<ToolCallId>, _>>()?;
    let append_batch_id: AppendBatchId = sources.generate::<AppendBatchTag>()?;
    let ids = AllocatedIds::try_new(
        records,
        events,
        Vec::new(),
        Vec::new(),
        message_id.into_iter().collect(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        tool_call_ids.clone(),
        vec![append_batch_id],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_settlement_ids_invalid",
    })?;
    Ok(SettlementAllocation {
        ids,
        message_id,
        tool_call_ids,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "terminal conversion keeps response, deferral, and failure identity continuity visible"
)]
fn build_settlement(
    result: ModelDriverResult,
    now: finstack_ai_kernel::Timestamp,
    allocation: &SettlementAllocation,
) -> Result<ModelSettled, RunHandleError> {
    let requested = &result.seed.pending.requested;
    let effect_id = requested.effect_id();
    let outcome = match result.result {
        Ok(terminal) => match terminal {
            ModelTerminal::Completed(response) => {
                let output_bytes = serde_json_canonicalizer::to_vec(&response).map_err(|_| {
                    RunHandleError::ModelSettlement {
                        code: "model_response_serialize_failed",
                    }
                })?;
                let output =
                    RawJson::parse(output_bytes).map_err(|_| RunHandleError::ModelSettlement {
                        code: "model_response_output_invalid",
                    })?;
                let completion = EffectCompleted::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    output,
                    Some(response.usage.clone()),
                    Vec::new(),
                    response.provider_ids.clone(),
                    Some(response.completion_id.as_ref()),
                    None,
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_effect_completion_invalid",
                })?;
                let mut content = response.assistant_content.to_vec();
                if allocation.tool_call_ids.len() != response.tool_calls.len() {
                    return Err(RunHandleError::ModelSettlement {
                        code: "model_tool_call_id_cardinality",
                    });
                }
                for (call, tool_call_id) in
                    response.tool_calls.iter().zip(&allocation.tool_call_ids)
                {
                    content.push(ContentBlock::ToolCall(
                        ToolCallBlock::try_new(*tool_call_id, &call.name, call.arguments.clone())
                            .map_err(|_| RunHandleError::ModelSettlement {
                            code: "model_tool_call_invalid",
                        })?,
                    ));
                }
                let model_ref = ModelRef::try_new(&result.provider, result.draft.model.as_str())
                    .map_err(|_| RunHandleError::ModelSettlement {
                        code: "model_reference_invalid",
                    })?;
                let message_id = allocation
                    .message_id
                    .ok_or(RunHandleError::ModelSettlement {
                        code: "model_message_id_missing",
                    })?;
                let assistant_message = Message::try_new(
                    message_id,
                    MessageRole::Assistant,
                    content,
                    now,
                    Some(model_ref),
                    response.provider_ids,
                    Metadata::empty(),
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_assistant_message_invalid",
                })?;
                ModelSettlement::Completed {
                    completion,
                    assistant_message,
                }
            }
            ModelTerminal::Deferred(deferral) => ModelSettlement::Deferred(EffectDeferred {
                effect_id,
                handle: deferral.handle,
                reconciliation: deferral.reconciliation,
                next_poll_at: deferral.next_poll_at,
                expires_at: deferral.expires_at,
                output_contract: requested.output_contract().clone(),
            }),
        },
        Err(error) => {
            let descriptor = error
                .to_descriptor()
                .map_err(|error| model_handle_error(&error))?;
            ModelSettlement::Failed(
                EffectFailed::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    descriptor,
                    None,
                    None::<&str>,
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_effect_failure_invalid",
                })?,
            )
        }
    };
    Ok(ModelSettled {
        turn_id: result.seed.pending.turn_id,
        model_request_id: result.seed.pending.model_request_id,
        outcome,
    })
}

// --- extracted from task.rs 2043-2093 ---
pub(crate) fn model_handle_error(error: &ModelError) -> RunHandleError {
    RunHandleError::Model {
        code: Arc::from(error.code()),
    }
}

pub(crate) fn tool_handle_error(error: &ToolError) -> RunHandleError {
    RunHandleError::Tool {
        code: Arc::from(error.code()),
    }
}

pub(crate) fn validate_model_binding(
    model: &dyn Model,
    profile: &LockedModelContextProfile,
) -> Result<(), RunHandleError> {
    let descriptor = model.descriptor();
    descriptor
        .validate()
        .map_err(|error| model_handle_error(&error))?;
    if descriptor.provider != profile.profile.provider
        || !descriptor.models.contains(&profile.profile.model)
    {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::MODEL_PROFILE_INVALID),
        });
    }
    let provider = model.capabilities(&profile.profile.model).context_profile;
    let effective = &profile.profile;
    let overlay = ModelContextProfileOverride {
        hard_input_bytes: Some(effective.hard_input_bytes),
        context_window_tokens: Some(effective.context_window_tokens),
        max_output_tokens: Some(effective.max_output_tokens),
        reserved_output_tokens: Some(effective.reserved_output_tokens),
        provider_overhead_tokens: Some(effective.provider_overhead_tokens),
    };
    let relocked = resolve_model_context_profile(provider, Some(&overlay), None, false)
        .map_err(|error| model_handle_error(&error))?;
    if relocked != *profile {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::MODEL_PROFILE_INVALID),
        });
    }
    Ok(())
}

pub(crate) fn id_source_error(_error: IdGenerationError) -> RunHandleError {
    RunHandleError::ModelSettlement {
        code: "model_settlement_id_source_failed",
    }
}
