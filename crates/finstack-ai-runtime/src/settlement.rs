//! Shared commit-settlement helpers for native and host-driven run owners.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use finstack_ai_kernel::{
    ActiveToolCallStatus, AllocatedIds, AppendBatchId, AppendBatchTag, AuthorizationEvidence,
    CancellationReconciledInput, CancellationRequestTag, ComponentId, ComponentRef, ContentBlock,
    Digest, EffectCompleted, EffectDeferred, EffectFailed, EffectId, EffectInput, EffectOutputKind,
    EffectTag, ErrorCategory, ErrorDescriptor, EventId, EventTag, ExternalCommandKind,
    ExternalCommandRejected, ExternalCommandTarget, ExternalEffectCompletedInput,
    ExternalEffectCompletion, ExternalEffectOutcome, Id, IdTag, InteractionExpired,
    InteractionKind, InteractionRequest, InteractionSettled, InteractionTag,
    InteractionTerminalOutcome, KernelError, KernelInput, Message, MessageId, MessageRole,
    MessageTag, Metadata, ModelRef, ModelRequestTag, ModelSettled, ModelSettlement,
    OutputConfiguration, OutputSpec, ProviderIds, RawJson, RecordExternalCommandRejected, RecordId,
    RecordTag, ReducerStageOutcome, RequestInteraction, RetrySafety, RunPhase, Stage, StageCursor,
    StageSettled, TerminalCandidate, TextBlock, ToolBatchContinuation, ToolBatchSettled,
    ToolBatchTag, ToolCallBlock, ToolCallId, ToolCallPlan, ToolCallTag, ToolFailurePolicy, ToolId,
    ToolSettlement, TransitionEnv, TurnTag, Version,
};

use crate::coordinator::{
    CommitCoordinator, CommitCoordinatorError, ModelDispatchSeed, ToolDispatchSeed,
};
use crate::middleware_driver::StageDriver;
use crate::run_types::RunHandleError;
use crate::stage_settlement::{ToolBatchPolicy, run_tool_batch_chain, submit_folded};
use crate::tool::AssembledToolTerminal;
use crate::{
    CancellationSignal, Clock, IdGenerationError, InteractionResumeAction,
    LockedModelContextProfile, Model, ModelContextProfileOverride, ModelDeferral, ModelError,
    ModelProgress, ModelReconcileResult, ModelRequestDraft, ModelResponse, ModelResumeAction,
    ModelTerminal, PendingToolEffect, RandomSource, ReconcileContext, ResolvedToolCatalog,
    RunCallContext, ToolCatalogPlan, ToolDeferral, ToolError, ToolPolicyDecision, ToolProgress,
    ToolReconcileResult, ToolResult, ToolResumeAction, UuidV7Generator, interaction_resume_action,
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
/// Settle `Stage::BeforeToolBatch` when the run is parked at that cursor.
///
/// The one facade stage the facade itself never settles, and therefore the one
/// stage whose middleware chain does not run at the `submit_command` choke
/// point. Its chain runs here instead, between collecting the source
/// `ToolCallBlock`s and deciding their plans, so the folded `FilterTools` set
/// feeds `ResolvedToolCatalog::decide_plan`'s existing `middleware` parameter.
/// The batch still settles exactly once, as the `ToolBatchPrepared` built from
/// the resulting plans.
///
/// # Fail closed
///
/// The cancellation and run-deadline guards precede the chain and return
/// before it: a run that is already cancelled or already out of budget must
/// not spend more of either on middleware.
///
/// `now` is read once, before the chain runs, and is the `now` every
/// settlement below uses — so a slow chain cannot move the run's semantic
/// transition instant, and a chain that runs past the deadline still settles
/// against the instant at which the batch was found ready. This mirrors
/// `stage_settlement::settle_facade_stage` keeping the facade's `env.now`
/// across its own fold.
///
/// # Coverage
///
/// The plan array is checked against the source calls by
/// [`assert_plan_coverage`] before submission. A denied call is a
/// [`ToolCallPlan::SyntheticClosure`] in place, never a shorter batch.
pub(crate) async fn prepare_tool_batch_if_ready<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<C, R>,
    driver: Option<&StageDriver>,
) -> Result<bool, RunHandleError> {
    if coordinator.state().phase != Some(RunPhase::BeforeToolBatch) {
        return Ok(false);
    }
    let now = sources.now()?;
    if coordinator.state().cancellation.is_some() {
        return Ok(false);
    }
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
    let continuation = if state.final_result.is_some() {
        ToolBatchContinuation::Finalize
    } else {
        ToolBatchContinuation::ContinueModel
    };
    let cursor = StageCursor {
        cycle: state.cycle,
        stage: Stage::BeforeToolBatch,
    };
    let retained = match run_tool_batch_chain(coordinator, driver, cursor, &calls).await? {
        ToolBatchPolicy::Unchanged => None,
        ToolBatchPolicy::Retain(retained) => Some(retained),
        ToolBatchPolicy::Fail(descriptor) => {
            settle_tool_batch_failure(coordinator, sources, now, cursor, *descriptor).await?;
            return Ok(false);
        }
    };
    let plans = match plan_source_calls(
        catalog,
        calls,
        deadline,
        retained.as_ref(),
        granted,
        refused,
    )? {
        PlannedBatch::Ready(plans) => plans,
        PlannedBatch::ApprovalRequired => {
            request_approval_interaction(coordinator, sources).await?;
            return Ok(false);
        }
    };
    submit_tool_batch_opening(coordinator, sources, now, cursor, plans, continuation).await
}

/// The planning loop's two outcomes for one batch.
enum PlannedBatch {
    /// One plan per source call, in source order.
    Ready(Vec<ToolCallPlan>),
    /// A call needs durable approval evidence the run does not have yet, so
    /// no batch is planned at this cursor.
    ApprovalRequired,
}

/// Decide one plan per source call, then check the coverage invariant.
///
/// `retained` is the folded `BeforeToolBatch` `FilterTools` narrowing, or
/// `None` when the chain did not narrow anything — see
/// [`middleware_tool_policy`].
///
/// # Errors
///
/// Returns [`TOOL_PLAN_COVERAGE_MISMATCH`] when the plans do not cover the
/// source calls exactly once each, in source order.
fn plan_source_calls(
    catalog: &ResolvedToolCatalog,
    calls: Vec<ToolCallBlock>,
    deadline: Option<finstack_ai_kernel::Timestamp>,
    retained: Option<&BTreeSet<ToolId>>,
    granted: bool,
    refused: bool,
) -> Result<PlannedBatch, RunHandleError> {
    let source_call_ids = calls
        .iter()
        .map(|call| *call.tool_call_id())
        .collect::<Vec<_>>();
    let mut plans = Vec::with_capacity(calls.len());
    for call in calls {
        let policy = middleware_tool_policy(catalog, &call, retained);
        match catalog.decide_plan(call, deadline, policy, granted, refused) {
            ToolCatalogPlan::Ready(plan) => plans.push(plan),
            ToolCatalogPlan::RequireApproval => return Ok(PlannedBatch::ApprovalRequired),
        }
    }
    assert_plan_coverage(&plans, &source_call_ids)?;
    Ok(PlannedBatch::Ready(plans))
}

/// Settle the cursor as `ToolBatchPrepared`, opening the batch.
async fn submit_tool_batch_opening<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    now: finstack_ai_kernel::Timestamp,
    cursor: StageCursor,
    plans: Vec<ToolCallPlan>,
    continuation: ToolBatchContinuation,
) -> Result<bool, RunHandleError> {
    let input = KernelInput::StageSettled(StageSettled {
        cursor,
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

/// Settle the cursor as `Fail` when the `BeforeToolBatch` chain fails the
/// stage, so no batch opens.
///
/// `Fail` is admissible at `BeforeToolBatch` (`decide.rs:1165-1177`) and
/// normalizes the stage as failed, driving the run to `BeforeFinalize` with
/// the component's own descriptor as the terminal candidate. Allocation goes
/// through [`submit_folded`], not [`stage_allocation`] directly, because a
/// middleware-authored outcome can itself be the input that crosses a run
/// limit and flips the kernel to `decide_limit`'s fixed id requirement.
async fn settle_tool_batch_failure<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    now: finstack_ai_kernel::Timestamp,
    cursor: StageCursor,
    descriptor: ErrorDescriptor,
) -> Result<(), RunHandleError> {
    let outcome = ReducerStageOutcome::Fail(descriptor);
    let committed = submit_folded(coordinator, sources, now, cursor, outcome).await?;
    if let Some(fault) = committed.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

/// The planned batch does not cover every source `ToolCallBlock` exactly once,
/// in source order.
const TOOL_PLAN_COVERAGE_MISMATCH: &str = "tool_plan_coverage_mismatch";

/// Translate the folded `BeforeToolBatch` `FilterTools` set into
/// `ResolvedToolCatalog::decide_plan`'s per-call `middleware` argument.
///
/// Returns `None` — literally the argument `decide_plan` received before this
/// hook existed — both when no component narrowed the tool set and when this
/// call's tool survived the narrowing, so a run whose chain retains everything
/// plans byte for byte like a run with no chain at all. (`Some(Allow)` would
/// be equivalent under `decide_plan`'s `max`, but `None` is identical.)
///
/// A call whose tool is absent from the retained set is denied. An
/// unregistered tool name is denied too, since it has no [`ToolId`] that could
/// be retained — a distinction without a difference in practice, because
/// `decide_plan` closes an unknown tool synthetically before it ever consults
/// this argument.
fn middleware_tool_policy(
    catalog: &ResolvedToolCatalog,
    call: &ToolCallBlock,
    retained: Option<&BTreeSet<ToolId>>,
) -> Option<ToolPolicyDecision> {
    let retained = retained?;
    let survives = catalog
        .by_name(call.tool_name())
        .is_some_and(|tool| retained.contains(&tool.spec.id));
    (!survives).then_some(ToolPolicyDecision::Deny)
}

/// Enforce the plan-coverage invariant: the `ToolBatchPrepared` plan array
/// covers every source `ToolCallBlock` exactly once, in source order.
///
/// A middleware-denied call is present as a
/// [`ToolCallPlan::SyntheticClosure`], never absent — dropping it would settle
/// a batch the model never asked for and leave the denied call unanswered.
/// The check is positional on [`ToolCallId`] rather than a length comparison,
/// because a length check alone would accept a reordered or duplicated plan
/// array just as happily as the correct one.
///
/// # Errors
///
/// Returns [`TOOL_PLAN_COVERAGE_MISMATCH`] rather than submitting a batch that
/// does not answer every source call.
fn assert_plan_coverage(
    plans: &[ToolCallPlan],
    source_call_ids: &[ToolCallId],
) -> Result<(), RunHandleError> {
    if plans.len() == source_call_ids.len()
        && plans
            .iter()
            .zip(source_call_ids)
            .all(|(plan, source)| plan.call().tool_call_id() == source)
    {
        return Ok(());
    }
    Err(RunHandleError::ToolSettlement {
        code: TOOL_PLAN_COVERAGE_MISMATCH,
    })
}

/// The outcome `fail_closed_on_run_deadline` submits at `Stage::BeforeToolBatch`.
///
/// The kernel admits `ReducerStageOutcome::Continue` only at
/// `BeforeRun`/`AfterModel`/`AfterToolBatch`
/// (`finstack-ai-kernel/src/reducer/decide.rs:1107-1111`); `BeforeToolBatch` is
/// not one of them. `Fail(_)`, by contrast, is admitted at every stage except
/// `BeforeFinalize`'s own dedicated arm (`decide.rs:1165-1177`), which is
/// exactly the fail-closed semantics this path needs: normalize the aggregate
/// stage as failed and let the reducer drive the run toward termination,
/// rather than pretending the (unopened) tool batch may continue.
/// `ToolBatchPrepared` was not a candidate: it requires real `ToolCallPlan`s
/// for calls that were never decided, and is validated by a wholly different
/// kernel path (`tool::decide_batch_prepared`) that a deadline breach cannot
/// satisfy.
///
/// In practice the kernel never even reaches the `stage_id_requirements` table
/// for this submission (see `fail_closed_on_run_deadline`'s own comment on
/// `decide_limit` precedence), so this choice is belt-and-suspenders rather
/// than load-bearing today — but it keeps the submitted outcome honest and
/// independently admissible if that precedence ever narrows.
fn run_deadline_outcome() -> Result<ReducerStageOutcome, RunHandleError> {
    Ok(ReducerStageOutcome::Fail(
        ErrorDescriptor::new(
            "deadline_exceeded",
            "run deadline exceeded before the tool batch could open",
            ErrorCategory::Deadline,
            false,
        )
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "run_deadline_descriptor_invalid",
        })?,
    ))
}

async fn fail_closed_on_run_deadline<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    now: finstack_ai_kernel::Timestamp,
) -> Result<(), RunHandleError> {
    let cursor = StageCursor {
        cycle: coordinator.state().cycle,
        stage: Stage::BeforeToolBatch,
    };
    let stage_outcome = run_deadline_outcome()?;
    let input = KernelInput::StageSettled(StageSettled {
        cursor,
        outcome: stage_outcome,
    });
    // Deliberately NOT `stage_allocation`: this function's only caller
    // (`prepare_tool_batch_if_ready`) invokes it exactly when
    // `now >= accepted.effective_deadline()`, using the same `now`. Under that
    // exact condition, `decide_limit` (`decide.rs:469-497`, guarded on
    // `env.now >= deadline` at `decide.rs:660-661`) intercepts *every*
    // non-`AcceptRun`/`InteractionSettled` input before `decide_stage` — and
    // therefore `stage_id_requirements` — ever runs, and it always demands
    // `IdRequirements::new(2, 2, 0, 0, 0, 0)` (`decide.rs:684`: one
    // `LimitReached` record + one `RunFailed` record, one event each, whatever
    // the submitted stage outcome is). Allocating via `stage_allocation`
    // instead (which computes `(1, 0, 0, 0, 0, 0)` for `Fail` at
    // `BeforeToolBatch`, per `stage_id_requirements`) under-allocates and the
    // submission is rejected — confirmed by
    // `run_deadline_fail_closed_is_admitted_by_the_kernel_at_before_tool_batch`
    // below reproducing exactly this mismatch.
    let ids = stage_ids(2, 2, 0, 0, 0, 0, sources)?;
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

/// Allocate the exact [`AllocatedIds`] the kernel requires for `outcome` at
/// `cursor`.
///
/// Mirrors `stage_id_requirements`
/// (`finstack-ai-kernel/src/reducer/decide.rs:1101-1183`) arm for arm, so an
/// inadmissible `(stage, outcome)` pair is rejected here — at allocation time —
/// instead of later at `coordinator.classify`/`submit` with a less specific
/// kernel error. `ReducerStageOutcome::ToolBatchPrepared` bypasses the tuple
/// table entirely in the kernel (`decide_stage` routes it to
/// `tool::decide_batch_prepared` before `stage_id_requirements` ever runs), so
/// it is handled here the same way: delegated whole to
/// [`allocate_tool_opening`], which already derives the exact counts from
/// `tool_opening_counts`.
///
/// **This function mirrors `stage_id_requirements` only.**
/// `stage_id_requirements` — and therefore this function's whole table,
/// `ToolBatchPrepared` included — is unreachable for *any* `StageSettled`
/// submission whenever `decide_limit` (`decide.rs:469-709`) returns a
/// decision instead of `None`. That is not specific to a deadline breach:
/// `decide_limit` first *increments* run-level usage from the very outcome
/// being submitted (`decide.rs:547-609` — `ContextPrepared` grows
/// `context_bytes`, `ModelRequestPrepared` grows `model_requests`,
/// `ToolBatchPrepared` grows `tool_calls`/`max_parallel_tools`, `Retry` grows
/// `retries`, and any effect/external completion grows token/cost usage), then
/// tests `deadline_crossing.or(first_limit_crossing(accepted, &usage))`
/// (`decide.rs:680`) — i.e. the run's wall-clock deadline *or* any configured
/// ceiling on turns, model requests, tool calls, parallel tools, retries,
/// wall time, or context bytes. Whichever fires, the kernel requires exactly
/// `IdRequirements::new(2, 2, 0, 0, 0, 0)` (`decide.rs:684`) regardless of the
/// submitted stage or outcome — the same fixed shape
/// `fail_closed_on_run_deadline` allocates directly (see its own comment),
/// not through this function. A caller that always allocates from this table
/// unconditionally will reproduce that exact class of mismatch on any run
/// that crosses a configured limit, not only on a deadline breach.
#[allow(
    dead_code,
    reason = "consumed by the middleware driver's stage fold (Tasks 6-7); exercised directly by this module's own tests until then"
)]
pub(crate) fn stage_allocation<C: Clock, R: RandomSource>(
    state: &finstack_ai_kernel::KernelState,
    cursor: StageCursor,
    outcome: &ReducerStageOutcome,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    if let ReducerStageOutcome::ToolBatchPrepared { calls, .. } = outcome {
        return allocate_tool_opening(calls, sources);
    }
    match outcome {
        // decide.rs:1107-1130 (plus the AfterModel JSON-schema guard).
        ReducerStageOutcome::Continue
            if matches!(
                cursor.stage,
                Stage::BeforeRun | Stage::AfterModel | Stage::AfterToolBatch
            ) =>
        {
            if cursor.stage == Stage::AfterModel
                && matches!(
                    state.output_configuration,
                    Some(OutputConfiguration {
                        output: OutputSpec::JsonSchema { .. },
                        ..
                    })
                )
                && state.final_result.is_none()
                && state.validation_failure.is_none()
            {
                return Err(RunHandleError::ToolSettlement {
                    code: "stage_allocation_output_contract_pending",
                });
            }
            stage_ids(1, 0, 0, 0, 0, 0, sources)
        }
        // decide.rs:1131-1133.
        ReducerStageOutcome::ContextPrepared { .. } if cursor.stage == Stage::PrepareContext => {
            stage_ids(2, 0, 0, 1, 0, 0, sources)
        }
        // decide.rs:1134-1141.
        ReducerStageOutcome::ModelRequestPrepared {
            output_contract, ..
        } if cursor.stage == Stage::BeforeModel => {
            if output_contract.kind != EffectOutputKind::ModelResponse {
                return Err(RunHandleError::ToolSettlement {
                    code: "stage_allocation_model_request_contract_mismatch",
                });
            }
            stage_ids(2, 1, 1, 0, 1, 0, sources)
        }
        // decide.rs:1142-1145 (terminal_body_from_candidate's own precondition:
        // a terminal candidate must exist).
        ReducerStageOutcome::FinalizeAccepted if cursor.stage == Stage::BeforeFinalize => {
            if state.terminal_candidate.is_none() {
                return Err(RunHandleError::ToolSettlement {
                    code: "stage_allocation_terminal_candidate_missing",
                });
            }
            stage_ids(2, 1, 0, 0, 0, 0, sources)
        }
        // decide.rs:1146-1158.
        ReducerStageOutcome::ContinueModel { .. }
            if cursor.stage == Stage::BeforeFinalize
                && matches!(
                    state.terminal_candidate,
                    Some(TerminalCandidate::Completed { .. })
                ) =>
        {
            state
                .cycle
                .checked_add(1)
                .ok_or(RunHandleError::ToolSettlement {
                    code: "stage_allocation_cycle_overflow",
                })?;
            stage_ids(1, 0, 0, 0, 0, 0, sources)
        }
        // decide.rs:1159-1161.
        ReducerStageOutcome::Fail(_) if cursor.stage == Stage::BeforeFinalize => {
            stage_ids(2, 1, 0, 0, 0, 0, sources)
        }
        // decide.rs:1162-1164.
        ReducerStageOutcome::Retry(_) if cursor.stage == Stage::BeforeFinalize => {
            stage_ids(3, 1, 1, 0, 0, 0, sources)
        }
        // decide.rs:1165-1177.
        ReducerStageOutcome::Fail(_)
            if matches!(
                cursor.stage,
                Stage::BeforeRun
                    | Stage::PrepareContext
                    | Stage::BeforeModel
                    | Stage::AfterModel
                    | Stage::BeforeToolBatch
                    | Stage::AfterToolBatch
            ) =>
        {
            stage_ids(1, 0, 0, 0, 0, 0, sources)
        }
        // decide.rs:1178-1181.
        _ => Err(RunHandleError::ToolSettlement {
            code: "stage_allocation_outcome_not_admitted",
        }),
    }
}

/// Generate an [`AllocatedIds`] bag of the given cardinalities, in the six-tuple
/// order `(records, events, effects, turns, model_requests, messages)` used by
/// `IdRequirements::new` (`finstack-ai-kernel/src/reducer/allocated_ids.rs:22-29`).
fn stage_ids<C: Clock, R: RandomSource>(
    records: usize,
    events: usize,
    effects: usize,
    turns: usize,
    model_requests: usize,
    messages: usize,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(records, sources)?,
        generate_tool_ids::<EventTag, _, _>(events, sources)?,
        generate_tool_ids::<EffectTag, _, _>(effects, sources)?,
        Vec::new(),
        generate_tool_ids::<MessageTag, _, _>(messages, sources)?,
        generate_tool_ids::<TurnTag, _, _>(turns, sources)?,
        generate_tool_ids::<ModelRequestTag, _, _>(model_requests, sources)?,
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "stage_allocation_ids_invalid",
    })
}

// --- extracted from task.rs 1225-1353 ---
enum IdleCancelClass {
    Cancelled,
    Completed,
    Uncertain,
}

pub(crate) async fn reconcile_cancelled_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    cancelled: bool,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    reconcile_classified_effect(
        coordinator,
        effect_id,
        if cancelled {
            IdleCancelClass::Cancelled
        } else {
            IdleCancelClass::Completed
        },
        sources,
    )
    .await
}

async fn reconcile_classified_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    class: IdleCancelClass,
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
    let (completed_effects, cancelled_effects, uncertain_effects) = match class {
        IdleCancelClass::Cancelled => (Arc::from([]), Arc::from([effect_id]), Arc::from([])),
        IdleCancelClass::Completed => (Arc::from([effect_id]), Arc::from([]), Arc::from([])),
        IdleCancelClass::Uncertain => (Arc::from([]), Arc::from([]), Arc::from([effect_id])),
    };
    let input = KernelInput::CancellationReconciled(CancellationReconciledInput {
        request_id,
        completed_effects,
        cancelled_effects,
        uncertain_effects,
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

pub(crate) async fn drain_idle_cancellation<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    force_all: bool,
) -> Result<(), RunHandleError> {
    loop {
        let Some(cancellation) = coordinator.state().cancellation.clone() else {
            return Ok(());
        };
        if coordinator.state().terminal.is_some()
            || coordinator.state().phase == Some(RunPhase::Suspended)
        {
            return Ok(());
        }
        let Some(effect_id) = cancellation
            .outstanding_effects
            .iter()
            .copied()
            .find(|id| idle_cancellation_class(coordinator.state(), *id, force_all).is_some())
        else {
            return Ok(());
        };
        let class = idle_cancellation_class(coordinator.state(), effect_id, force_all).ok_or(
            RunHandleError::CancellationSettlement {
                code: "idle_cancellation_class_missing",
            },
        )?;
        reconcile_classified_effect(coordinator, effect_id, class, sources).await?;
    }
}

fn idle_cancellation_class(
    state: &finstack_ai_kernel::KernelState,
    effect_id: EffectId,
    force_all: bool,
) -> Option<IdleCancelClass> {
    if state
        .pending_interaction
        .as_ref()
        .is_some_and(|pending| pending.request.effect_id() == effect_id)
    {
        return Some(IdleCancelClass::Cancelled);
    }
    if state
        .retry
        .pending
        .as_ref()
        .is_some_and(|pending| pending.timer_effect_id == effect_id)
    {
        return Some(IdleCancelClass::Cancelled);
    }
    if let Some(pending) = state.pending_model_effect.as_ref()
        && pending.requested.effect_id() == effect_id
    {
        return effect_idle_class(
            pending.requested.retry_safety(),
            pending.deferred.is_some(),
            force_all,
        );
    }
    state.active_tool_batch.as_ref().and_then(|batch| {
        batch.calls.iter().find_map(|call| {
            if call.assigned.effect_id != effect_id {
                return None;
            }
            match &call.status {
                ActiveToolCallStatus::Requested {
                    requested,
                    deferred,
                } => effect_idle_class(requested.retry_safety(), deferred.is_some(), force_all),
                _ => None,
            }
        })
    })
}

fn effect_idle_class(
    safety: RetrySafety,
    deferred: bool,
    force_all: bool,
) -> Option<IdleCancelClass> {
    if !deferred && !force_all {
        return None;
    }
    Some(match safety {
        RetrySafety::AtMostOnce | RetrySafety::Unknown => IdleCancelClass::Uncertain,
        RetrySafety::SafeToRetry | RetrySafety::IdempotentWithKey => IdleCancelClass::Cancelled,
    })
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
    if coordinator.state().cancellation.is_some() {
        return Ok(InteractionResumeAction::WaitResolution);
    }
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
        .map_err(|error| tool_handle_error(&ToolError::from(error)))?);
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
        .map_err(|error| model_handle_error(&ModelError::from(error)))?);
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use finstack_ai_kernel::{
        BudgetPropagation, CancellationPropagation, DeadlinePropagation, Kernel, KernelState,
        LaneTag, PrincipalPropagation, PrincipalRef, RunAccepted, RunPropagationPolicy,
        RunRelation, RunSecurityContext, RunTag, SessionTag, Timestamp,
    };

    use super::*;
    use crate::ExternalClock;

    fn fixed_id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn fixed_timestamp(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    /// Mirrors `coordinator.rs`'s own `tests::acceptance()` fixture (same field
    /// values), parameterized on `effective_deadline` so the deadline path can
    /// exercise a run whose deadline has already passed.
    fn acceptance(effective_deadline: Option<Timestamp>) -> RunAccepted {
        let run_id = fixed_id::<RunTag>(3);
        RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            effective_deadline,
            finstack_ai_kernel::RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"agent":"fixture"}"#),
            None,
        )
        .expect("acceptance")
    }

    /// Base fixture reused by every test below: a minimally valid accepted,
    /// running `KernelState`. Individual tests override `phase`/`cycle`/
    /// `accepted` via struct-update syntax where the scenario needs it.
    fn accepted_state() -> KernelState {
        KernelState {
            session_id: Some(fixed_id::<SessionTag>(1)),
            lane_id: Some(fixed_id::<LaneTag>(2)),
            accepted: Some(acceptance(None)),
            accepted_at: Some(fixed_timestamp(1_000)),
            phase: Some(RunPhase::BeforeRun),
            cycle: 0,
            ..KernelState::default()
        }
    }

    /// Deterministic, collision-free random source: each `fill_bytes` call
    /// tiles the buffer with the bytes of a monotonic counter, so repeated
    /// allocations inside one test never collide the way two calls against a
    /// truly fixed byte pattern would.
    #[derive(Default)]
    struct CountingRandom(AtomicU64);

    impl RandomSource for CountingRandom {
        fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
            let counter = self.0.fetch_add(1, Ordering::Relaxed);
            let bytes = counter.to_be_bytes();
            for (index, slot) in buf.iter_mut().enumerate() {
                *slot = bytes[index % bytes.len()];
            }
            Ok(())
        }
    }

    fn test_sources() -> SettlementSources<ExternalClock, CountingRandom> {
        sources_at(1_000)
    }

    fn sources_at(now_ms: i64) -> SettlementSources<ExternalClock, CountingRandom> {
        SettlementSources::try_new(
            ExternalClock::new(fixed_timestamp(now_ms)),
            CountingRandom::default(),
        )
        .expect("sources")
    }

    /// The test named in the task-3 brief: allocation for the same outcome
    /// must differ by stage. This is exactly the property `StageIds::for_outcome`
    /// (reverted by this task) got wrong by keying allocation on the outcome
    /// alone.
    #[test]
    fn fail_allocation_differs_between_before_finalize_and_other_stages() {
        let state = accepted_state();
        let sources = test_sources();
        let fail = ReducerStageOutcome::Fail(
            ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
                .expect("descriptor"),
        );

        let at_finalize = stage_allocation(
            &state,
            StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            &fail,
            &sources,
        )
        .expect("finalize allocation");

        let at_before_run = stage_allocation(
            &state,
            StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            &fail,
            &sources,
        )
        .expect("before_run allocation");

        assert_eq!(
            at_finalize.record_ids().len(),
            2,
            "BeforeFinalize Fail needs 2 records"
        );
        assert_eq!(
            at_finalize.event_ids().len(),
            1,
            "BeforeFinalize Fail needs 1 event"
        );
        assert_eq!(
            at_before_run.record_ids().len(),
            1,
            "BeforeRun Fail needs 1 record"
        );
        assert_eq!(
            at_before_run.event_ids().len(),
            0,
            "BeforeRun Fail needs 0 events"
        );
    }

    #[test]
    fn run_deadline_fail_closed_uses_a_stage_legal_outcome() {
        // fail_closed_on_run_deadline settles Stage::BeforeToolBatch. The kernel
        // rejects Continue there (decide.rs:1107-1111), so the fail-closed path
        // must not emit Continue.
        let outcome = run_deadline_outcome().expect("run deadline outcome");
        assert!(
            !matches!(outcome, ReducerStageOutcome::Continue),
            "BeforeToolBatch cannot accept Continue; got {outcome:?}"
        );
    }

    /// `stage_allocation` in isolation (no run deadline configured, so the
    /// kernel's own `decide_limit` deadline-crossing precedence — see the two
    /// tests below — cannot confound the result): it must mirror
    /// `stage_id_requirements` and refuse `Continue` at `BeforeToolBatch`,
    /// exactly the outcome the pre-existing bug submitted.
    #[test]
    fn stage_allocation_rejects_continue_at_before_tool_batch() {
        let state = KernelState {
            phase: Some(RunPhase::BeforeToolBatch),
            ..accepted_state()
        };
        let sources = test_sources();
        let cursor = StageCursor {
            cycle: state.cycle,
            stage: Stage::BeforeToolBatch,
        };
        assert!(
            stage_allocation(&state, cursor, &ReducerStageOutcome::Continue, &sources).is_err(),
            "stage_allocation must reject Continue at BeforeToolBatch, matching stage_id_requirements"
        );
    }

    /// End-to-end, with the run's deadline actually in the past — the exact
    /// precondition `fail_closed_on_run_deadline`'s only caller
    /// (`prepare_tool_batch_if_ready`) guarantees before invoking it.
    ///
    /// This is the test that caught a second, deeper issue than the one in
    /// the brief: once `now >= effective_deadline`, `decide_limit`
    /// (`decide.rs:469-497`) intercepts *before* `decide_stage` — and
    /// therefore `stage_id_requirements` — ever runs, and always demands
    /// `IdRequirements::new(2, 2, 0, 0, 0, 0)` (`decide.rs:684`), not whatever
    /// `stage_allocation` computes for the submitted outcome. Allocating via
    /// `stage_allocation` here (as an earlier version of this fix did)
    /// compiles and passes every other unit test in this module, but fails an
    /// actual deadline-expiry run end to end
    /// (`finstack-ai-test/tests/interaction.rs::expire_if_due_on_restore_never_dispatches`,
    /// confirmed by reproducing the failure locally before writing this
    /// assertion). This test pins both halves of that discovery so a future
    /// change cannot silently reintroduce it.
    #[test]
    fn run_deadline_fail_closed_is_admitted_by_the_kernel_at_before_tool_batch() {
        let deadline = fixed_timestamp(1_500);
        let now = fixed_timestamp(2_000);
        let state = KernelState {
            phase: Some(RunPhase::BeforeToolBatch),
            accepted: Some(acceptance(Some(deadline))),
            ..accepted_state()
        };
        let sources = test_sources();
        let cursor = StageCursor {
            cycle: state.cycle,
            stage: Stage::BeforeToolBatch,
        };
        let outcome = run_deadline_outcome().expect("run deadline outcome");

        // The wrong shape: stage_allocation's Fail@BeforeToolBatch tuple
        // (1, 0, 0, 0, 0, 0) is the stage_id_requirements answer, but
        // decide_limit never lets stage_id_requirements run here.
        let wrong_ids =
            stage_allocation(&state, cursor, &outcome, &sources).expect("stage allocation");
        let rejected = Kernel::try_restore(state.clone())
            .expect("restore state")
            .decide(
                &TransitionEnv {
                    now,
                    ids: wrong_ids,
                },
                KernelInput::StageSettled(StageSettled {
                    cursor,
                    outcome: outcome.clone(),
                }),
            );
        assert!(
            rejected.is_err(),
            "stage_allocation's ids must NOT satisfy decide_limit's deadline-crossing \
             requirement once the deadline has passed: {rejected:?}"
        );

        // The real fix's shape: matches decide_limit's own requirement.
        let ids = stage_ids(2, 2, 0, 0, 0, 0, &sources).expect("deadline crossing ids");
        let decision = Kernel::try_restore(state).expect("restore state").decide(
            &TransitionEnv { now, ids },
            KernelInput::StageSettled(StageSettled { cursor, outcome }),
        );
        assert!(
            decision.is_ok(),
            "kernel rejected the fail-closed submission at BeforeToolBatch: {decision:?}"
        );
    }

    /// The deadline itself: `fail_closed_on_run_deadline` is reached only when
    /// `now >= effective_deadline`. This pins that the fixture used above
    /// actually represents an expired run, not merely a state the kernel
    /// happens to accept.
    #[test]
    fn accepted_run_with_past_deadline_reports_expired() {
        let deadline = fixed_timestamp(1_500);
        let accepted = acceptance(Some(deadline));
        let now = fixed_timestamp(2_000);
        assert!(
            accepted
                .effective_deadline()
                .is_some_and(|value| now >= value)
        );
    }

    fn completed_candidate(cycle: u64) -> TerminalCandidate {
        TerminalCandidate::Completed {
            cycle,
            turn_id: fixed_id(20),
            model_request_id: fixed_id(21),
            effect_id: fixed_id(22),
            message_id: fixed_id(23),
            result_digest: Digest::raw_json(b"{}"),
        }
    }

    fn failed_candidate(cycle: u64) -> TerminalCandidate {
        TerminalCandidate::Failed {
            cycle,
            turn_id: None,
            model_request_id: None,
            effect_id: None,
            error: ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
                .expect("descriptor"),
        }
    }

    fn model_request_prepared(kind: EffectOutputKind) -> ReducerStageOutcome {
        ReducerStageOutcome::ModelRequestPrepared {
            request: RawJson::parse(b"{}").expect("request"),
            component: None,
            output_contract: finstack_ai_kernel::EffectOutputContract {
                kind,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"{}"),
            },
            retry_safety: RetrySafety::SafeToRetry,
            deadline: None,
        }
    }

    /// Ids expected out of a successful `stage_allocation` call, in the same
    /// `(records, events, effects, turns, model_requests, messages)` order as
    /// `IdRequirements::new`.
    struct Ids {
        records: usize,
        events: usize,
        effects: usize,
        turns: usize,
        model_requests: usize,
        messages: usize,
    }

    enum Expect {
        Ok(Ids),
        Err(&'static str),
    }

    /// One `(state, cursor, outcome)` case and the tuple or error code
    /// `decide.rs` demands for it, checked against `stage_allocation`'s own
    /// answer.
    struct Case {
        label: &'static str,
        state: KernelState,
        cursor: StageCursor,
        outcome: ReducerStageOutcome,
        expect: Expect,
    }

    /// Table-driven: every `stage_allocation` match arm, both the `Ok` tuple
    /// and, where the arm has one, the guard's `Err` code — checked against
    /// one shared assertion so the table is the single place a future edit to
    /// `stage_allocation` (or a drift against `decide.rs`) has to be updated,
    /// rather than seven separate ad hoc tests.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one flat case table keeps every stage_allocation arm's tuple/error next to its decide.rs citation"
    )]
    fn stage_allocation_matches_decide_rs_arm_for_arm() {
        let base = accepted_state();
        let sources = test_sources();

        let cases = vec![
            // decide.rs:1107-1130 — Continue is admitted at BeforeRun.
            Case {
                label: "Continue @ BeforeRun",
                state: KernelState {
                    phase: Some(RunPhase::BeforeRun),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeRun,
                },
                outcome: ReducerStageOutcome::Continue,
                expect: Expect::Ok(Ids {
                    records: 1,
                    events: 0,
                    effects: 0,
                    turns: 0,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide.rs:1113-1128 — the AfterModel + JsonSchema + no-final-
            // result-yet guard rejects Continue even though AfterModel is
            // otherwise an admitted stage for it.
            Case {
                label: "Continue @ AfterModel (pending JsonSchema output)",
                state: KernelState {
                    phase: Some(RunPhase::AfterModel),
                    output_configuration: Some(OutputConfiguration {
                        output: OutputSpec::JsonSchema {
                            schema: finstack_ai_kernel::SchemaRef {
                                draft: finstack_ai_kernel::JsonSchemaDraft::Draft202012,
                                schema_version: 1,
                                schema_digest: Digest::raw_json(b"{}"),
                            },
                        },
                        ..OutputConfiguration::default()
                    }),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::AfterModel,
                },
                outcome: ReducerStageOutcome::Continue,
                expect: Expect::Err("stage_allocation_output_contract_pending"),
            },
            // decide.rs:1131-1133.
            Case {
                label: "ContextPrepared @ PrepareContext",
                state: KernelState {
                    phase: Some(RunPhase::PreparingContext),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::PrepareContext,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([]),
                },
                expect: Expect::Ok(Ids {
                    records: 2,
                    events: 0,
                    effects: 0,
                    turns: 1,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide.rs:1134-1141.
            Case {
                label: "ModelRequestPrepared @ BeforeModel (ModelResponse contract)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeModel),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeModel,
                },
                outcome: model_request_prepared(EffectOutputKind::ModelResponse),
                expect: Expect::Ok(Ids {
                    records: 2,
                    events: 1,
                    effects: 1,
                    turns: 0,
                    model_requests: 1,
                    messages: 0,
                }),
            },
            // decide.rs:1137-1139 — the contract-kind guard.
            Case {
                label: "ModelRequestPrepared @ BeforeModel (ToolResult contract)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeModel),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeModel,
                },
                outcome: model_request_prepared(EffectOutputKind::ToolResult),
                expect: Expect::Err("stage_allocation_model_request_contract_mismatch"),
            },
            // decide.rs:1142-1145.
            Case {
                label: "FinalizeAccepted @ BeforeFinalize (candidate present)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeFinalize),
                    terminal_candidate: Some(completed_candidate(0)),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
                outcome: ReducerStageOutcome::FinalizeAccepted,
                expect: Expect::Ok(Ids {
                    records: 2,
                    events: 1,
                    effects: 0,
                    turns: 0,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide.rs:1142-1145 — terminal_body_from_candidate's own
            // precondition (private to the kernel crate; mirrored here via the
            // public `terminal_candidate` field) that a candidate must exist.
            Case {
                label: "FinalizeAccepted @ BeforeFinalize (no candidate)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeFinalize),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
                outcome: ReducerStageOutcome::FinalizeAccepted,
                expect: Expect::Err("stage_allocation_terminal_candidate_missing"),
            },
            // decide.rs:1146-1157.
            Case {
                label: "ContinueModel @ BeforeFinalize (Completed candidate)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeFinalize),
                    terminal_candidate: Some(completed_candidate(0)),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
                outcome: ReducerStageOutcome::ContinueModel { reason: None },
                expect: Expect::Ok(Ids {
                    records: 1,
                    events: 0,
                    effects: 0,
                    turns: 0,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide.rs:1146-1151 — the guard requires a *Completed* candidate;
            // a Failed one is not admitted for ContinueModel and falls through
            // to the catch-all.
            Case {
                label: "ContinueModel @ BeforeFinalize (Failed candidate)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeFinalize),
                    terminal_candidate: Some(failed_candidate(0)),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
                outcome: ReducerStageOutcome::ContinueModel { reason: None },
                expect: Expect::Err("stage_allocation_outcome_not_admitted"),
            },
            // decide.rs:1153-1156 — the checked cycle-overflow guard.
            Case {
                label: "ContinueModel @ BeforeFinalize (cycle overflow)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeFinalize),
                    cycle: u64::MAX,
                    terminal_candidate: Some(completed_candidate(u64::MAX)),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: u64::MAX,
                    stage: Stage::BeforeFinalize,
                },
                outcome: ReducerStageOutcome::ContinueModel { reason: None },
                expect: Expect::Err("stage_allocation_cycle_overflow"),
            },
            // decide.rs:1159-1161.
            Case {
                label: "Fail @ BeforeFinalize",
                state: KernelState {
                    phase: Some(RunPhase::BeforeFinalize),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
                outcome: ReducerStageOutcome::Fail(
                    ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
                        .expect("descriptor"),
                ),
                expect: Expect::Ok(Ids {
                    records: 2,
                    events: 1,
                    effects: 0,
                    turns: 0,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide.rs:1162-1164.
            Case {
                label: "Retry @ BeforeFinalize",
                state: KernelState {
                    phase: Some(RunPhase::BeforeFinalize),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
                outcome: ReducerStageOutcome::Retry(
                    finstack_ai_kernel::RetryDirective::try_new(
                        finstack_ai_kernel::RetryClassification::Validation,
                        finstack_ai_kernel::Duration::ZERO,
                        "probe-policy",
                    )
                    .expect("directive"),
                ),
                expect: Expect::Ok(Ids {
                    records: 3,
                    events: 1,
                    effects: 1,
                    turns: 0,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide.rs:1165-1177.
            Case {
                label: "Fail @ BeforeRun",
                state: KernelState {
                    phase: Some(RunPhase::BeforeRun),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeRun,
                },
                outcome: ReducerStageOutcome::Fail(
                    ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
                        .expect("descriptor"),
                ),
                expect: Expect::Ok(Ids {
                    records: 1,
                    events: 0,
                    effects: 0,
                    turns: 0,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide_stage:1078-1080 — ToolBatchPrepared bypasses the table
            // entirely and is delegated whole to `allocate_tool_opening`.
            // `tool_opening_counts` on an empty call list yields
            // records = 2 + 0 (requests) + 0 (messages) + 1 (no executable
            // group) = 3, events = 0, effects = 0, messages = 0.
            Case {
                label: "ToolBatchPrepared @ BeforeToolBatch (delegates to allocate_tool_opening)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeToolBatch),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeToolBatch,
                },
                outcome: ReducerStageOutcome::ToolBatchPrepared {
                    calls: Arc::from([]),
                    continuation: ToolBatchContinuation::ContinueModel,
                },
                expect: Expect::Ok(Ids {
                    records: 3,
                    events: 0,
                    effects: 0,
                    turns: 0,
                    model_requests: 0,
                    messages: 0,
                }),
            },
            // decide.rs:1178-1181 — the catch-all: an outcome that is never
            // admitted at the given stage under any arm.
            Case {
                label: "ContextPrepared @ BeforeRun (wrong stage, catch-all)",
                state: KernelState {
                    phase: Some(RunPhase::BeforeRun),
                    ..base.clone()
                },
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeRun,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([]),
                },
                expect: Expect::Err("stage_allocation_outcome_not_admitted"),
            },
        ];

        for case in cases {
            let result = stage_allocation(&case.state, case.cursor, &case.outcome, &sources);
            match case.expect {
                Expect::Ok(ids) => {
                    let allocated = result.unwrap_or_else(|error| {
                        panic!("{}: expected Ok, got {error:?}", case.label)
                    });
                    assert_eq!(
                        allocated.record_ids().len(),
                        ids.records,
                        "{}: record_ids",
                        case.label
                    );
                    assert_eq!(
                        allocated.event_ids().len(),
                        ids.events,
                        "{}: event_ids",
                        case.label
                    );
                    assert_eq!(
                        allocated.effect_ids().len(),
                        ids.effects,
                        "{}: effect_ids",
                        case.label
                    );
                    assert_eq!(
                        allocated.turn_ids().len(),
                        ids.turns,
                        "{}: turn_ids",
                        case.label
                    );
                    assert_eq!(
                        allocated.model_request_ids().len(),
                        ids.model_requests,
                        "{}: model_request_ids",
                        case.label
                    );
                    assert_eq!(
                        allocated.message_ids().len(),
                        ids.messages,
                        "{}: message_ids",
                        case.label
                    );
                    assert_eq!(
                        allocated.append_batch_ids().len(),
                        1,
                        "{}: append_batch_ids",
                        case.label
                    );
                }
                Expect::Err(code) => match result {
                    Err(RunHandleError::ToolSettlement { code: actual }) => {
                        assert_eq!(actual, code, "{}: error code", case.label);
                    }
                    other => panic!(
                        "{}: expected ToolSettlement {{ code: {code:?} }}, got {other:?}",
                        case.label
                    ),
                },
            }
        }
    }

    // ---- BeforeToolBatch middleware --------------------------------------
    //
    // The one facade stage that never settles through `submit_command`:
    // `prepare_tool_batch_if_ready` settles it, so its chain runs here and
    // lands through `decide_plan`'s `middleware` parameter rather than as an
    // aggregate `ReducerStageOutcome`.

    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::task::{Context as TaskContext, Poll, Waker};

    use finstack_ai_kernel::{
        AcceptRun, AppendRequest, AssignedToolCall, CommittedBatch, ComponentInvocation,
        EffectOutputContract, InvocationRecovery, RecordBody, RecordDraft, RecordEnvelope,
        ToolBatchOpened, ToolExecutionMode, ToolId,
    };

    use crate::middleware::{
        Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
        MiddlewareRegistration, MiddlewareRole, OrderTier, ResolvedMiddlewareChain, StageInput,
        StageMask, StageOutcome,
    };
    use crate::middleware_driver::StageDriver;
    use crate::{
        ApprovalMetadata, ApprovalRequirement, JournalStore, JsonSchemaToolValidatorCompiler,
        LoadRequest, LoadedSession, PortFuture, SideEffectClass, SnapshotReceipt, SnapshotRequest,
        StoreError, StoreHealth, ToolCallContext, ToolEventStream, ToolExecutionPolicy,
        ToolPolicyDecision, ToolSpec, ToolsetRegistration,
    };

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut context = TaskContext::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[expect(clippy::too_many_arguments, reason = "mirrors AllocatedIds' own bags")]
    fn env(
        now: i64,
        records: &[u64],
        events: &[u64],
        effects: &[u64],
        turns: &[u64],
        model_requests: &[u64],
        messages: &[u64],
        append_batch: u64,
    ) -> TransitionEnv {
        TransitionEnv {
            now: fixed_timestamp(now),
            ids: AllocatedIds::try_new(
                records.iter().copied().map(fixed_id).collect(),
                events.iter().copied().map(fixed_id).collect(),
                effects.iter().copied().map(fixed_id).collect(),
                Vec::new(),
                messages.iter().copied().map(fixed_id).collect(),
                turns.iter().copied().map(fixed_id).collect(),
                model_requests.iter().copied().map(fixed_id).collect(),
                Vec::new(),
                Vec::new(),
                vec![fixed_id(append_batch)],
                Vec::new(),
            )
            .expect("allocated ids"),
        }
    }

    fn model_settled_env(now: i64, message: u64, tool_calls: &[u64]) -> TransitionEnv {
        TransitionEnv {
            now: fixed_timestamp(now),
            ids: AllocatedIds::try_new(
                vec![fixed_id(607), fixed_id(608)],
                vec![fixed_id(603), fixed_id(604)],
                Vec::new(),
                Vec::new(),
                vec![fixed_id(message)],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                tool_calls.iter().copied().map(fixed_id).collect(),
                vec![fixed_id(605)],
                Vec::new(),
            )
            .expect("model settled ids"),
        }
    }

    // -- in-memory journal --------------------------------------------------

    struct MemoryStore {
        inner: Mutex<MemoryInner>,
    }

    #[derive(Default)]
    struct MemoryInner {
        batches: Vec<CommittedBatch>,
        drafts: Vec<RecordDraft>,
    }

    impl MemoryStore {
        fn new() -> Self {
            Self {
                inner: Mutex::new(MemoryInner::default()),
            }
        }

        /// The single durable `ToolBatchOpened`, i.e. the kernel's own record
        /// of the complete source-ordered assigned plan. Read from the journal
        /// rather than from `state.active_tool_batch` because a batch of
        /// nothing but synthetic closures opens and closes in one transition,
        /// leaving no active batch behind to inspect.
        fn opened_tool_batch(&self) -> Option<ToolBatchOpened> {
            self.inner
                .lock()
                .expect("lock")
                .drafts
                .iter()
                .find_map(|draft| match draft.body() {
                    RecordBody::ToolBatchOpened(opened) => Some(opened.clone()),
                    _ => None,
                })
        }
    }

    fn commit_request(request: &AppendRequest) -> CommittedBatch {
        let records = request
            .records()
            .iter()
            .enumerate()
            .map(|(offset, draft)| {
                let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
                RecordEnvelope::try_new(
                    draft.format_version(),
                    draft.kind_version(),
                    draft.record_id(),
                    draft.session_id(),
                    draft.lane_id(),
                    draft.run_id(),
                    sequence,
                    draft.timestamp(),
                    None,
                    Digest::raw_json(format!("payload-{sequence}").as_bytes()),
                    None,
                    Digest::raw_json(format!("checksum-{sequence}").as_bytes()),
                    draft.derived_event_ids().to_vec(),
                    draft.body().clone(),
                )
                .expect("envelope")
            })
            .collect::<Vec<_>>();
        CommittedBatch::try_new(
            request.batch_id(),
            request.expected_sequence(),
            request.expected_sequence() + u64::try_from(records.len()).expect("count") - 1,
            records,
        )
        .expect("committed batch")
    }

    impl JournalStore for MemoryStore {
        fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
            let committed = commit_request(&request);
            let mut inner = self.inner.lock().expect("lock");
            inner.drafts.extend(request.records().iter().cloned());
            inner.batches.push(committed.clone());
            Box::pin(async move { Ok(committed) })
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            let inner = self.inner.lock().expect("lock");
            let batches = inner.batches.clone();
            let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
            Box::pin(async move {
                Ok(LoadedSession {
                    session_id: request.session_id,
                    head_sequence,
                    head_checksum: batches
                        .last()
                        .and_then(|batch| batch.records.last().map(RecordEnvelope::checksum)),
                    metadata: Metadata::empty(),
                    committed_batches: batches.into(),
                    snapshot: None,
                    accelerated: None,
                })
            })
        }

        fn write_snapshot(
            &self,
            _request: SnapshotRequest,
        ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "not_used",
                })
            })
        }

        fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
            Box::pin(async {
                Ok(StoreHealth {
                    ready: true,
                    durable: false,
                    detail: Arc::from("test"),
                })
            })
        }
    }

    /// These tests stop at batch opening, so nothing is ever executed — but a
    /// coordinator with no dispatcher at all faults the moment the kernel
    /// commits an effect, which would mask the behaviour under test.
    struct NoopDispatcher;

    impl crate::coordinator::PostCommitDispatcher for NoopDispatcher {
        fn dispatch(
            &self,
            _dispatch: crate::coordinator::RuntimeDispatch,
        ) -> PortFuture<Result<(), crate::coordinator::DispatchError>> {
            Box::pin(async { Ok(()) })
        }
    }

    // -- tool catalog -------------------------------------------------------

    const TOOL_NAMES: [&str; 3] = ["alpha", "beta", "gamma"];

    fn tool_id(name: &str) -> ToolId {
        ToolId::parse(format!("finstack.tools.{name}")).expect("tool id")
    }

    fn tool_spec(name: &str) -> ToolSpec {
        ToolSpec {
            id: tool_id(name),
            model_name: Arc::from(name),
            title: Arc::from(name),
            description: Arc::from("fixture tool"),
            input_schema: RawJson::parse(br#"{"type":"object"}"#).expect("input schema"),
            output_schema: None,
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 4_096,
            metadata: Metadata::empty(),
        }
    }

    /// A registered but never-dispatched toolset: these tests stop at batch
    /// opening, which is where the `BeforeToolBatch` decision lands.
    struct FixtureToolset {
        specs: Arc<[ToolSpec]>,
    }

    impl crate::Toolset for FixtureToolset {
        fn descriptor(&self) -> crate::ToolsetDescriptor {
            crate::ToolsetDescriptor {
                name: Arc::from("fixture.toolset"),
                metadata: Metadata::empty(),
            }
        }

        fn tools(&self) -> Arc<[ToolSpec]> {
            Arc::clone(&self.specs)
        }

        fn call(
            &self,
            _ctx: ToolCallContext,
            _call: finstack_ai_kernel::ValidatedToolCall,
        ) -> PortFuture<Result<ToolEventStream, ToolError>> {
            Box::pin(async {
                Err(ToolError::stable(
                    "fixture_tool_never_dispatched",
                    "the fixture stops at batch opening",
                ))
            })
        }
    }

    fn catalog() -> ResolvedToolCatalog {
        let specs: Arc<[ToolSpec]> = TOOL_NAMES.iter().copied().map(tool_spec).collect();
        let policies = specs
            .iter()
            .map(|spec| {
                (
                    spec.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::Allow,
                        max_concurrency: 1,
                    },
                )
            })
            .collect();
        ResolvedToolCatalog::try_new(
            [ToolsetRegistration {
                toolset: Arc::new(FixtureToolset { specs }),
                policies,
                components: BTreeMap::new(),
            }],
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .expect("catalog")
    }

    // -- middleware ---------------------------------------------------------

    fn descriptor(component: &str, stage: Stage) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse(component).expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            stages: StageMask::from_stages([stage]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        }
    }

    /// A component that returns one fixed outcome and counts its invocations,
    /// so a test can tell "ran and contributed nothing" from "never ran".
    struct Fixed {
        descriptor: MiddlewareDescriptor,
        outcome: StageOutcome,
        calls: Arc<AtomicUsize>,
    }

    impl Middleware for Fixed {
        fn descriptor(&self) -> MiddlewareDescriptor {
            self.descriptor.clone()
        }

        fn invoke(
            &self,
            _ctx: MiddlewareContext,
            _input: StageInput,
        ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let outcome = self.outcome.clone();
            Box::pin(async move { Ok(outcome) })
        }
    }

    fn driver_for(stage: Stage, outcome: StageOutcome, calls: &Arc<AtomicUsize>) -> StageDriver {
        let middleware: Arc<dyn Middleware> = Arc::new(Fixed {
            descriptor: descriptor("fixture.tool-policy", stage),
            outcome,
            calls: Arc::clone(calls),
        });
        StageDriver::new(
            Arc::new(
                ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                    .expect("chain"),
            ),
            CancellationSignal::new(),
        )
    }

    fn retain(names: &[&str]) -> StageOutcome {
        StageOutcome::FilterTools(names.iter().copied().map(tool_id).collect())
    }

    // -- driving a coordinator to BeforeToolBatch ---------------------------

    fn tool_call(ordinal: u64, name: &str) -> ToolCallBlock {
        ToolCallBlock::try_new(
            fixed_id(ordinal),
            name,
            RawJson::parse(b"{}").expect("arguments"),
        )
        .expect("tool call")
    }

    fn model_output_contract() -> EffectOutputContract {
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
        }
    }

    /// Drive a fresh coordinator all the way to `RunPhase::BeforeToolBatch`
    /// with an assistant message carrying one tool call per name in `names`.
    fn coordinator_at_before_tool_batch(
        store: &Arc<MemoryStore>,
        names: &[&str],
        deadline: Option<Timestamp>,
    ) -> CommitCoordinator {
        let mut coordinator = CommitCoordinator::new(Arc::clone(store) as Arc<dyn JournalStore>);
        coordinator.install_dispatcher(Arc::new(NoopDispatcher));
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: fixed_id::<SessionTag>(1),
                lane_id: fixed_id::<LaneTag>(2),
                accepted: acceptance(deadline),
            }),
        ))
        .expect("accept");
        block_on(coordinator.submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeRun,
                },
                outcome: ReducerStageOutcome::Continue,
            }),
        ))
        .expect("before run");
        let user = Message::try_new(
            fixed_id(4),
            MessageRole::User,
            vec![ContentBlock::Text(
                TextBlock::try_new("call the tools").expect("text"),
            )],
            fixed_timestamp(900),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("user message");
        block_on(coordinator.submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::PrepareContext,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([user]),
                },
            }),
        ))
        .expect("context");
        block_on(coordinator.submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeModel,
                },
                outcome: ReducerStageOutcome::ModelRequestPrepared {
                    request: RawJson::parse(br#"{"messages":[]}"#).expect("request"),
                    component: None,
                    output_contract: model_output_contract(),
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: None,
                },
            }),
        ))
        .expect("model request");
        settle_model_with_tool_calls(&mut coordinator, names);
        block_on(coordinator.submit(
            env(1_500, &[609], &[], &[], &[], &[], &[], 606),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::AfterModel,
                },
                outcome: ReducerStageOutcome::Continue,
            }),
        ))
        .expect("after model");
        assert_eq!(
            coordinator.state().phase,
            Some(RunPhase::BeforeToolBatch),
            "the fixture must park the run exactly at the BeforeToolBatch cursor"
        );
        coordinator
    }

    /// Settle the outstanding model effect with an assistant message carrying
    /// one tool call per name, ordinals 301, 302, ... in source order.
    fn settle_model_with_tool_calls(coordinator: &mut CommitCoordinator, names: &[&str]) {
        let pending = coordinator
            .state()
            .pending_model_effect
            .as_ref()
            .expect("pending model effect")
            .clone();
        let call_ordinals = (0..names.len())
            .map(|index| 301 + u64::try_from(index).expect("index"))
            .collect::<Vec<_>>();
        let mut content = vec![ContentBlock::Text(
            TextBlock::try_new("calling").expect("text"),
        )];
        content.extend(
            names
                .iter()
                .zip(&call_ordinals)
                .map(|(name, ordinal)| ContentBlock::ToolCall(tool_call(*ordinal, name))),
        );
        let assistant = Message::try_new(
            fixed_id(617),
            MessageRole::Assistant,
            content,
            fixed_timestamp(1_400),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("assistant message");
        let completion = EffectCompleted::try_new(
            pending.requested.effect_id(),
            model_output_contract(),
            RawJson::parse(br#"{"text":"calling"}"#).expect("output"),
            None,
            Vec::new(),
            ProviderIds::empty(),
            Some("cmpl-tool"),
            None,
        )
        .expect("completion");
        block_on(coordinator.submit(
            model_settled_env(1_400, 617, &call_ordinals),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message: assistant,
                },
            }),
        ))
        .expect("model settled");
    }

    /// Drive to `BeforeToolBatch` and run `prepare_tool_batch_if_ready` with
    /// `driver`, returning the durable source-ordered plan the kernel opened.
    fn prepare_with(
        names: &[&str],
        driver: Option<&StageDriver>,
    ) -> (Vec<AssignedToolCall>, CommitCoordinator, Arc<MemoryStore>) {
        let store = Arc::new(MemoryStore::new());
        let mut coordinator = coordinator_at_before_tool_batch(&store, names, None);
        let sources = sources_at(1_600);
        block_on(prepare_tool_batch_if_ready(
            &mut coordinator,
            &catalog(),
            &sources,
            driver,
        ))
        .expect("tool batch preparation");
        let plans = store
            .opened_tool_batch()
            .expect("a tool batch must have been opened")
            .calls
            .to_vec();
        (plans, coordinator, store)
    }

    fn planned_call_ids(plans: &[AssignedToolCall]) -> Vec<ToolCallId> {
        plans
            .iter()
            .map(|assigned| *assigned.plan.call().tool_call_id())
            .collect()
    }

    fn source_call_ids(count: usize) -> Vec<ToolCallId> {
        (0..count)
            .map(|index| fixed_id(301 + u64::try_from(index).expect("index")))
            .collect()
    }

    // -- passthrough --------------------------------------------------------

    #[test]
    fn no_driver_plans_every_source_call_for_execution() {
        let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, None);
        assert_eq!(plans.len(), 3);
        assert!(
            plans
                .iter()
                .all(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_))),
            "with no chain every registered, schema-valid call must execute: {plans:?}"
        );
    }

    #[test]
    fn a_chain_with_no_before_tool_batch_component_is_passthrough() {
        // The facade installs its chain unconditionally, so the driver is
        // almost always `Some`. Passthrough must key off `is_active`, and the
        // resulting plan must be byte-identical to the no-driver one.
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(Stage::AfterModel, retain(&[]), &calls);
        let (with_driver, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));
        let (without_driver, _c, _s) = prepare_with(&TOOL_NAMES, None);

        assert_eq!(
            with_driver, without_driver,
            "an inactive stage must not perturb the opened plan"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "no BeforeToolBatch component is registered, so none may run"
        );
    }

    #[test]
    fn an_identity_fold_leaves_the_plan_unchanged() {
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(Stage::BeforeToolBatch, StageOutcome::Continue, &calls);
        let (with_driver, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));
        let (without_driver, _c, _s) = prepare_with(&TOOL_NAMES, None);

        assert_eq!(calls.load(Ordering::Relaxed), 1, "the component must run");
        assert_eq!(
            with_driver, without_driver,
            "an all-Continue chain must not perturb the opened plan"
        );
    }

    // -- filtering ----------------------------------------------------------

    #[test]
    fn filtered_tool_call_becomes_a_synthetic_closure() {
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
        let (plans, _coordinator, _store) = prepare_with(&["alpha"], Some(&driver));

        assert_eq!(plans.len(), 1, "every source call must appear exactly once");
        assert!(
            matches!(plans[0].plan, ToolCallPlan::SyntheticClosure(_)),
            "a denied call must be a synthetic closure, got {:?}",
            plans[0].plan
        );
        assert_eq!(planned_call_ids(&plans), source_call_ids(1));
    }

    #[test]
    fn fully_filtered_batch_still_commits_every_source_call() {
        // `prepare_tool_batch_if_ready` guards on the source `calls` being
        // empty, never on the plans, so denying every call still commits a
        // batch of all-SyntheticClosure plans rather than short-circuiting.
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
        let (plans, coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

        assert_eq!(plans.len(), 3);
        assert!(
            plans
                .iter()
                .all(|assigned| matches!(assigned.plan, ToolCallPlan::SyntheticClosure(_))),
            "every denied call must still be planned: {plans:?}"
        );
        assert_eq!(
            planned_call_ids(&plans),
            source_call_ids(3),
            "coverage is positional: same calls, same order, no duplicates"
        );
        assert!(
            coordinator.state().terminal.is_none(),
            "a fully filtered batch is not a run failure: {:?}",
            coordinator.state().terminal
        );
    }

    #[test]
    fn partial_filter_denies_only_the_calls_outside_the_retained_set() {
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(Stage::BeforeToolBatch, retain(&["beta"]), &calls);
        let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

        let shapes = plans
            .iter()
            .map(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_)))
            .collect::<Vec<_>>();
        assert_eq!(
            shapes,
            vec![false, true, false],
            "only the retained tool may execute, and source order is preserved: {plans:?}"
        );
        assert_eq!(planned_call_ids(&plans), source_call_ids(3));
    }

    #[test]
    fn source_order_survives_a_filter_that_denies_the_first_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(Stage::BeforeToolBatch, retain(&["gamma"]), &calls);
        let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

        let shapes = plans
            .iter()
            .map(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_)))
            .collect::<Vec<_>>();
        assert_eq!(
            shapes,
            vec![false, false, true],
            "denying the leading calls must not shift the surviving one forward: {plans:?}"
        );
        assert_eq!(
            plans
                .iter()
                .map(|assigned| assigned.source_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(planned_call_ids(&plans), source_call_ids(3));
    }

    // -- the coverage invariant itself --------------------------------------

    #[test]
    fn plan_coverage_rejects_a_short_reordered_or_duplicated_plan_array() {
        let alpha = tool_call(301, "alpha");
        let beta = tool_call(302, "beta");
        let plan = |call: &ToolCallBlock| {
            ToolCallPlan::SyntheticClosure(finstack_ai_kernel::SyntheticToolClosure {
                call: call.clone(),
                execution: ToolExecutionMode::Sequential,
                failure_policy: ToolFailurePolicy::ReturnToModel,
                error: ErrorDescriptor::new(
                    "tool_policy_denied",
                    "denied",
                    ErrorCategory::Validation,
                    false,
                )
                .expect("descriptor"),
            })
        };
        let source = vec![*alpha.tool_call_id(), *beta.tool_call_id()];

        assert_plan_coverage(&[plan(&alpha), plan(&beta)], &source).expect("exact cover");
        for (label, plans) in [
            ("short", vec![plan(&alpha)]),
            ("reordered", vec![plan(&beta), plan(&alpha)]),
            ("duplicated", vec![plan(&alpha), plan(&alpha)]),
            ("long", vec![plan(&alpha), plan(&beta), plan(&beta)]),
        ] {
            let Err(error) = assert_plan_coverage(&plans, &source) else {
                panic!("a {label} plan array must be rejected");
            };
            assert!(
                matches!(&error, RunHandleError::ToolSettlement { code }
                    if *code == TOOL_PLAN_COVERAGE_MISMATCH),
                "{label}: expected the reserved coverage code, got {error:?}"
            );
        }
    }

    // -- terminal folds and the deadline bypass -----------------------------

    #[test]
    fn a_middleware_failure_settles_the_stage_as_failed_instead_of_opening_a_batch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(
            Stage::BeforeToolBatch,
            StageOutcome::Fail(Box::new(
                ErrorDescriptor::new(
                    "tool_batch_rejected",
                    "fixture",
                    ErrorCategory::Middleware,
                    false,
                )
                .expect("descriptor"),
            )),
            &calls,
        );
        let store = Arc::new(MemoryStore::new());
        let mut coordinator = coordinator_at_before_tool_batch(&store, &TOOL_NAMES, None);
        let sources = sources_at(1_600);

        let opened = block_on(prepare_tool_batch_if_ready(
            &mut coordinator,
            &catalog(),
            &sources,
            Some(&driver),
        ))
        .expect("the failed stage still settles");

        assert!(!opened, "no batch may open when the chain fails the stage");
        assert!(
            store.opened_tool_batch().is_none(),
            "no ToolBatchOpened record may exist"
        );
        // `Fail` at a non-`BeforeFinalize` stage (`decide.rs:1165-1177`)
        // normalizes the stage as failed and drives the run to
        // `BeforeFinalize` carrying the component's own descriptor; the
        // facade's finalize settlement is what commits the terminal.
        assert_eq!(coordinator.state().phase, Some(RunPhase::BeforeFinalize));
        let Some(TerminalCandidate::Failed { error, .. }) =
            coordinator.state().terminal_candidate.as_ref()
        else {
            panic!(
                "the middleware Fail must become the terminal candidate: {:?}",
                coordinator.state().terminal_candidate
            );
        };
        assert_eq!(error.code.as_str(), "tool_batch_rejected");
    }

    #[test]
    fn the_run_deadline_path_bypasses_the_chain_entirely() {
        // Fail closed: a run already out of budget must not spend more of it
        // on middleware.
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
        let store = Arc::new(MemoryStore::new());
        let mut coordinator =
            coordinator_at_before_tool_batch(&store, &TOOL_NAMES, Some(fixed_timestamp(1_550)));
        let sources = sources_at(1_600);

        let opened = block_on(prepare_tool_batch_if_ready(
            &mut coordinator,
            &catalog(),
            &sources,
            Some(&driver),
        ))
        .expect("the deadline path settles");

        assert!(!opened);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "the deadline path must not invoke any middleware component"
        );
        assert!(store.opened_tool_batch().is_none());
    }
}
