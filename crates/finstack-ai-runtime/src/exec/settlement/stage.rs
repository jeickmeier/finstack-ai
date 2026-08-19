use std::collections::BTreeSet;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, ContentBlock, EffectOutputKind, EffectTag, ErrorCategory,
    ErrorDescriptor, EventTag, KernelInput, MessageTag, ModelRequestTag, OutputConfiguration,
    OutputSpec, RecordTag, ReducerStageOutcome, RunPhase, Stage, StageCursor, StageSettled,
    TerminalCandidate, ToolBatchContinuation, ToolCallBlock, ToolCallId, ToolCallPlan, ToolId,
    TransitionEnv, TurnTag,
};

use crate::coordinator::CommitCoordinator;
use crate::middleware_driver::StageDriver;
use crate::run_types::RunHandleError;
use crate::stage_settlement::{ToolBatchPolicy, run_tool_batch_chain, submit_folded};
use crate::{Clock, RandomSource, ResolvedToolCatalog, ToolCatalogPlan, ToolPolicyDecision};

use super::SettlementSources;
use super::interaction::{
    allocate_tool_opening, approval_refused_for_current_cursor,
    approval_released_for_current_cursor, request_approval_interaction,
};
use super::tool::{generate_tool_id, generate_tool_ids};

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
pub(super) const TOOL_PLAN_COVERAGE_MISMATCH: &str = "tool_plan_coverage_mismatch";

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
pub(super) fn assert_plan_coverage(
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
pub(super) fn run_deadline_outcome() -> Result<ReducerStageOutcome, RunHandleError> {
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
pub(super) fn stage_ids<C: Clock, R: RandomSource>(
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
