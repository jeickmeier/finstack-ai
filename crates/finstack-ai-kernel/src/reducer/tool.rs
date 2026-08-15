//! PR-010 tool-batch decisions and deterministic normalization helpers.

use std::sync::Arc;

use super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::capacity::{self, StateGrowth};
use super::decide::{
    draft_for_state, duplicate_decision, next_sequence, reject_terminal, required,
};
use super::decision::{Decision, KernelError, PostCommitAction};
use super::fingerprint::{
    direct_tool_digest, external_tool_digest, synthetic_tool_digest, tool_batch_close_digest,
    tool_batch_plan_digest,
};
use super::input::{
    ExternalEffectCompletedInput, ExternalEffectOutcome, StageSettled, ToolBatchSettled,
    ToolSettlement,
};
use super::validation::{validate_completion_identity, validate_error_descriptor};
use crate::content::{ContentBlock, JsonBlock, ToolCallBlock, ToolResultBlock};
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectFailed, EffectInput, EffectKind, EffectOutputKind,
    EffectRequested,
};
use crate::entries::{StageDisposition, StageOutcomeRecorded};
use crate::message::{Message, MessageRole, ProviderIds};
use crate::raw_json::{Metadata, RawJson};
use crate::records::APPEND_BATCH_MAX_RECORDS;
use crate::records::RecordBody;
use crate::state::{KernelState, RunPhase, TransitionEnv};
use crate::tools::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, AssignedToolCall, ToolBatchClosed,
    ToolBatchContinuation, ToolBatchOpened, ToolBatchOutcome, ToolCallPlan, ToolCallSettled,
    ToolFailurePolicy,
};

#[expect(
    clippy::too_many_lines,
    reason = "batch opening keeps the frozen validation, record order, and ID preflight in one path"
)]
pub(super) fn decide_batch_prepared(
    state: &KernelState,
    env: &TransitionEnv,
    input: &StageSettled,
    settlement_digest: crate::Digest,
) -> Result<Decision, KernelError> {
    let super::input::ReducerStageOutcome::ToolBatchPrepared {
        calls,
        continuation,
    } = &input.outcome
    else {
        return Err(KernelError::InvariantViolation);
    };
    if state.phase != Some(RunPhase::BeforeToolBatch) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    let turn = state
        .current_turn
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let source_message = state
        .messages
        .last()
        .ok_or(KernelError::InvariantViolation)?;
    let source_calls = assistant_calls(source_message);
    validate_plans(state, &source_calls, calls)?;
    validate_record_batch_bounds(calls)?;
    validate_allocated_ids(&env.ids, opening_id_requirements(calls)?)?;

    let tool_batch_id = required(env.ids.tool_batch_ids(), 0, "tool_batch_ids")?;
    let assigned = assign_plans(env, calls)?;
    let plan_digest = tool_batch_plan_digest(
        state.cycle,
        turn.turn_id,
        tool_batch_id,
        *source_message.id(),
        &assigned,
        *continuation,
    )?;
    let opened = ToolBatchOpened {
        cycle: state.cycle,
        turn_id: turn.turn_id,
        tool_batch_id,
        source_message_id: *source_message.id(),
        calls: assigned.clone().into(),
        continuation: *continuation,
        plan_digest,
    };

    let first_group = first_executable_group(&assigned);
    let mut bodies = vec![
        RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
            cursor: input.cursor,
            disposition: StageDisposition::ToolBatchPrepared {
                tool_batch_id,
                plan_digest,
            },
            settlement_digest,
        }),
        RecordBody::ToolBatchOpened(opened.clone()),
    ];
    let mut actions = Vec::new();
    if let Some(group) = first_group {
        append_group_requests(&assigned, group, &mut bodies, &mut actions)?;
    }

    let mut result_ids = Vec::new();
    let mut synthetic_effects = Vec::new();
    for assigned_call in &assigned {
        let ToolCallPlan::SyntheticClosure(closure) = &assigned_call.plan else {
            break;
        };
        let result = synthetic_result(&closure.call, &closure.error)?;
        let digest = synthetic_tool_digest(
            tool_batch_id,
            *closure.call.tool_call_id(),
            assigned_call.effect_id,
            &result,
            &closure.error,
        )?;
        let message_id = required(env.ids.message_ids(), result_ids.len(), "message_ids")?;
        bodies.push(RecordBody::ToolCallSettled(tool_settled_record(
            &opened,
            assigned_call,
            message_id,
            env,
            BufferedToolResult {
                result,
                settlement_digest: digest,
                synthetic: true,
                error: Some(closure.error.clone()),
            },
        )?));
        result_ids.push(message_id);
        synthetic_effects.push(assigned_call.effect_id);
    }

    if result_ids.len() == assigned.len() {
        bodies.push(RecordBody::ToolBatchClosed(close_record(
            &opened,
            result_ids.clone(),
            outcome_for_continuation(*continuation),
        )?));
    }
    ensure_record_batch_bound(bodies.len())?;

    capacity::preflight_decision(
        state,
        StateGrowth {
            messages: result_ids.len(),
            stage: Some(input.cursor),
            tool_settlements: &synthetic_effects,
            ..StateGrowth::default()
        },
    )?;
    let requirements = requirements_for_bodies(&bodies, result_ids.len(), assigned.len(), 1)?;
    validate_allocated_ids(&env.ids, requirements)?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions,
        diagnostics: Vec::new(),
    })
}

fn opening_id_requirements(plans: &[ToolCallPlan]) -> Result<IdRequirements, KernelError> {
    let groups = execution_groups(plans)?;
    let first_executable_group = plans
        .iter()
        .zip(&groups)
        .find_map(|(plan, group)| matches!(plan, ToolCallPlan::Execute(_)).then_some(*group));
    let requests = first_executable_group.map_or(0, |group| {
        plans
            .iter()
            .zip(&groups)
            .filter(|(plan, assigned_group)| {
                **assigned_group == group && matches!(plan, ToolCallPlan::Execute(_))
            })
            .count()
    });
    let synthetic = plans
        .iter()
        .take_while(|plan| matches!(plan, ToolCallPlan::SyntheticClosure(_)))
        .count();
    let records = 2 + requests + synthetic + usize::from(first_executable_group.is_none());
    let events = requests + 2 * synthetic;
    Ok(IdRequirements::new(records, events, plans.len(), 0, 0, synthetic).with_tools(1, 0))
}

pub(super) fn decide_tool_settled(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ToolBatchSettled,
) -> Result<Decision, KernelError> {
    if let ToolSettlement::Failed(failure) = &input.outcome {
        validate_error_descriptor(failure.error())?;
    }
    let digest = direct_tool_digest(input.tool_batch_id, &input.outcome)?;
    settle_normalized_tool(
        state,
        env,
        input.tool_batch_id,
        &input.outcome,
        digest,
        false,
    )
}

pub(super) fn is_known_tool_effect(state: &KernelState, effect_id: crate::EffectId) -> bool {
    state.active_tool_batch.as_ref().is_some_and(|batch| {
        batch
            .calls
            .iter()
            .any(|call| call.assigned.effect_id == effect_id)
    }) || state
        .tool_calls
        .values()
        .any(|identity| identity.effect_id == Some(effect_id))
}

pub(super) fn decide_external_tool(
    state: &KernelState,
    env: &TransitionEnv,
    input: ExternalEffectCompletedInput,
) -> Result<Decision, KernelError> {
    let indexed_completion = state
        .completion_identities
        .get(input.completion.completion_id.as_ref());
    if indexed_completion.is_some_and(|existing| existing.effect_id != input.completion.effect_id) {
        return Err(KernelError::ConflictingCompletionId);
    }
    let tool_batch_id = state
        .active_tool_batch
        .as_ref()
        .and_then(|batch| {
            batch
                .calls
                .iter()
                .any(|call| call.assigned.effect_id == input.completion.effect_id)
                .then_some(batch.opened.tool_batch_id)
        })
        .or_else(|| {
            state
                .tool_calls
                .values()
                .find(|identity| identity.effect_id == Some(input.completion.effect_id))
                .and_then(|identity| identity.tool_batch_id)
        })
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    let digest = external_tool_digest(tool_batch_id, &input)?;
    if let Some(existing) = indexed_completion {
        return if existing.effect_id == input.completion.effect_id
            && existing.settlement_digest == digest
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.tool_settlements.get(&input.completion.effect_id) {
        return if existing.digest == digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if input.assistant_message.is_some() {
        return Err(KernelError::AssistantMessagePresenceMismatch);
    }
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    let active = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == input.completion.effect_id)
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    let ActiveToolCallStatus::Requested {
        requested,
        deferred: Some(_),
    } = &active.status
    else {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    };
    let completion_id = input.completion.completion_id;
    let outcome = match input.completion.outcome {
        ExternalEffectOutcome::Completed {
            output,
            usage,
            artifacts,
        } => ToolSettlement::Completed(
            EffectCompleted::try_new(
                requested.effect_id(),
                requested.output_contract().clone(),
                output,
                usage,
                artifacts.to_vec(),
                ProviderIds::empty(),
                Some(completion_id.as_ref()),
                None,
            )
            .map_err(|_| KernelError::ToolSettlementMismatch)?,
        ),
        ExternalEffectOutcome::Failed { error } => ToolSettlement::Failed(
            EffectFailed::try_new(
                requested.effect_id(),
                requested.output_contract().clone(),
                error,
                None,
                Some(completion_id.as_ref()),
            )
            .map_err(|_| KernelError::ToolSettlementMismatch)?,
        ),
    };
    settle_normalized_tool(state, env, tool_batch_id, &outcome, digest, true)
}

#[expect(
    clippy::too_many_lines,
    reason = "direct and external settlements share one ordered fail-closed validation pipeline"
)]
fn settle_normalized_tool(
    state: &KernelState,
    env: &TransitionEnv,
    tool_batch_id: crate::ToolBatchId,
    outcome: &ToolSettlement,
    settlement_digest: crate::Digest,
    external: bool,
) -> Result<Decision, KernelError> {
    let effect_id = settlement_effect_id(outcome);
    let completion_id = settlement_completion_id(outcome);
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
    {
        return if existing.effect_id == effect_id && existing.settlement_digest == settlement_digest
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.tool_settlements.get(&effect_id) {
        return if existing.digest == settlement_digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    reject_terminal(state)?;
    let valid_phase = if external {
        state.phase == Some(RunPhase::AwaitingExternal)
    } else {
        matches!(
            state.phase,
            Some(RunPhase::AwaitingTools | RunPhase::AwaitingExternal)
        )
    };
    if !valid_phase {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: if external {
                "external_effect_completed"
            } else {
                "tool_batch_settled"
            },
        });
    }
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(KernelError::ToolSettlementMismatch)?;
    if batch.opened.tool_batch_id != tool_batch_id {
        return Err(KernelError::ToolSettlementMismatch);
    }
    let call_index = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == effect_id)
        .ok_or(KernelError::ToolSettlementMismatch)?;
    let active_call = &batch.calls[call_index];
    let ActiveToolCallStatus::Requested {
        requested,
        deferred,
    } = &active_call.status
    else {
        return Err(KernelError::ToolSettlementMismatch);
    };
    if !external && let (Some(existing), ToolSettlement::Deferred(_)) = (deferred, outcome) {
        let prior = ToolSettlement::Deferred(existing.clone());
        return if direct_tool_digest(tool_batch_id, &prior)? == settlement_digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if external != deferred.is_some() {
        return Err(KernelError::ToolSettlementMismatch);
    }
    if requested.kind() != EffectKind::Tool
        || requested.output_contract().kind != EffectOutputKind::ToolResult
    {
        return Err(KernelError::ToolEffectContractMismatch);
    }

    let mut prospective = batch.clone();
    let mut bodies = Vec::new();
    match outcome {
        ToolSettlement::Completed(completion) => {
            completion
                .validate_against(requested)
                .map_err(|_| KernelError::ToolSettlementMismatch)?;
            if completion.output_contract().kind != EffectOutputKind::ToolResult
                || completion.provider_ids() != &ProviderIds::empty()
                || completion.reservation_id().is_some()
            {
                return Err(KernelError::ToolEffectContractMismatch);
            }
            validate_completion_identity(
                state,
                completion.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            let result = decode_tool_result(completion, active_call.assigned.plan.call())?;
            Arc::make_mut(&mut prospective.calls)[call_index].status =
                ActiveToolCallStatus::Buffered {
                    result,
                    settlement_digest,
                    synthetic: false,
                    error: None,
                };
            bodies.push(RecordBody::EffectCompleted(completion.clone()));
        }
        ToolSettlement::Failed(failure) => {
            failure
                .validate_against(requested)
                .map_err(|_| KernelError::ToolSettlementMismatch)?;
            validate_completion_identity(
                state,
                failure.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            let error = failure.error().clone();
            let result = synthetic_result(active_call.assigned.plan.call(), &error)?;
            Arc::make_mut(&mut prospective.calls)[call_index].status =
                ActiveToolCallStatus::Buffered {
                    result,
                    settlement_digest,
                    synthetic: true,
                    error: Some(error.clone()),
                };
            if active_call.assigned.plan.failure_policy() == ToolFailurePolicy::FailRun
                && prospective.fatal_error.is_none()
            {
                prospective.fatal_error = Some(error);
            }
            bodies.push(RecordBody::EffectFailed(failure.clone()));
        }
        ToolSettlement::Deferred(value) => {
            value
                .validate_against(requested)
                .map_err(|_| KernelError::ToolSettlementMismatch)?;
            if external {
                return Err(KernelError::ToolSettlementMismatch);
            }
            Arc::make_mut(&mut prospective.calls)[call_index].status =
                ActiveToolCallStatus::Requested {
                    requested: requested.clone(),
                    deferred: Some(value.clone()),
                };
            bodies.push(RecordBody::EffectDeferred(value.clone()));
            let requirements = requirements_for_bodies(&bodies, 0, 0, 0)?;
            validate_allocated_ids(&env.ids, requirements)?;
            return Ok(Decision {
                expected_sequence: next_sequence(state)?,
                records: draft_for_state(state, env, bodies)?,
                actions: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
    }

    let mut followups = followup_records(&mut prospective, env)?;
    followups.settlements.push(effect_id);
    bodies.append(&mut followups.bodies);
    ensure_record_batch_bound(bodies.len())?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            messages: followups.messages,
            tool_settlements: &followups.settlements,
            completion: completion_id,
            ..StateGrowth::default()
        },
    )?;
    let requirements = requirements_for_bodies(&bodies, followups.messages, 0, 0)?;
    validate_allocated_ids(&env.ids, requirements)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(state, env, bodies)?,
        actions: followups.actions,
        diagnostics: Vec::new(),
    })
}

struct ToolFollowups {
    bodies: Vec<RecordBody>,
    actions: Vec<PostCommitAction>,
    settlements: Vec<crate::EffectId>,
    messages: usize,
}

pub(super) struct CancellationToolFollowups {
    pub bodies: Vec<RecordBody>,
    pub settlements: Vec<crate::EffectId>,
    pub messages: usize,
}

pub(super) fn buffer_cancelled_effect(
    batch: &mut ActiveToolBatch,
    effect_id: crate::EffectId,
) -> Result<bool, KernelError> {
    let error = cancelled_error()?;
    let batch_id = batch.opened.tool_batch_id;
    let Some(call) = Arc::make_mut(&mut batch.calls)
        .iter_mut()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
        return Ok(false);
    };
    if !matches!(call.status, ActiveToolCallStatus::Requested { .. }) {
        return Ok(false);
    }
    buffer_cancellation_result(call, batch_id, &error)?;
    Ok(true)
}

pub(super) fn buffer_reconciled_tool_closures(
    batch: &mut ActiveToolBatch,
    completed_effects: &[crate::EffectId],
) -> Result<(), KernelError> {
    let error = cancelled_error()?;
    let batch_id = batch.opened.tool_batch_id;
    for call in Arc::make_mut(&mut batch.calls) {
        let completed = completed_effects
            .binary_search(&call.assigned.effect_id)
            .is_ok();
        if matches!(call.status, ActiveToolCallStatus::Undispatched)
            || (completed && matches!(call.status, ActiveToolCallStatus::Requested { .. }))
        {
            buffer_cancellation_result(call, batch_id, &error)?;
        }
    }
    Ok(())
}

fn buffer_cancellation_result(
    call: &mut ActiveToolCall,
    tool_batch_id: crate::ToolBatchId,
    error: &crate::ErrorDescriptor,
) -> Result<(), KernelError> {
    let result = synthetic_result(call.assigned.plan.call(), error)?;
    let digest = synthetic_tool_digest(
        tool_batch_id,
        *call.assigned.plan.call().tool_call_id(),
        call.assigned.effect_id,
        &result,
        error,
    )?;
    call.status = ActiveToolCallStatus::Buffered {
        result,
        settlement_digest: digest,
        synthetic: true,
        error: Some(error.clone()),
    };
    Ok(())
}

/// Build cancellation records against a prospective tool batch.
///
/// Requested calls classified as cancelled receive `EffectCancelled`; calls
/// classified as completed are conservatively closed with the same
/// framework-authored cancelled result because reconciliation carries no tool
/// output. Undispatched calls receive the identical closure. Buffered real
/// results remain real. Canonical result records are emitted only for the
/// contiguous source prefix, so bounded reconciliation chunks are replay-safe.
pub(super) fn cancellation_followups(
    state: &KernelState,
    env: &TransitionEnv,
    newly_completed: &[crate::EffectId],
    newly_cancelled: &[crate::EffectId],
) -> Result<CancellationToolFollowups, KernelError> {
    let Some(mut batch) = state.active_tool_batch.clone() else {
        return Ok(CancellationToolFollowups {
            bodies: Vec::new(),
            settlements: Vec::new(),
            messages: 0,
        });
    };
    let error = cancelled_error()?;
    let batch_id = batch.opened.tool_batch_id;
    let mut bodies = Vec::new();
    for call in Arc::make_mut(&mut batch.calls) {
        let effect_id = call.assigned.effect_id;
        let classified_cancelled = newly_cancelled.binary_search(&effect_id).is_ok();
        let classified_completed = newly_completed.binary_search(&effect_id).is_ok();
        let should_close = classified_cancelled
            || classified_completed
            || matches!(call.status, ActiveToolCallStatus::Undispatched);
        if !should_close {
            continue;
        }
        if classified_cancelled
            && let ActiveToolCallStatus::Requested { requested, .. } = &call.status
        {
            bodies.push(RecordBody::EffectCancelled(
                EffectCancelled::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    Some("cancelled"),
                    Option::<&str>::None,
                )
                .map_err(|_| KernelError::InvariantViolation)?,
            ));
        }
        if matches!(
            call.status,
            ActiveToolCallStatus::Requested { .. } | ActiveToolCallStatus::Undispatched
        ) {
            buffer_cancellation_result(call, batch_id, &error)?;
        }
    }

    let mut result_ids = batch.result_message_ids.to_vec();
    let start =
        usize::try_from(batch.next_source_index).map_err(|_| KernelError::InvariantViolation)?;
    let mut message_count = 0_usize;
    let mut settlements = Vec::new();
    for index in start..batch.calls.len() {
        let ActiveToolCallStatus::Buffered {
            result,
            settlement_digest,
            synthetic,
            error,
        } = batch.calls[index].status.clone()
        else {
            break;
        };
        let message_id = required(env.ids.message_ids(), message_count, "message_ids")?;
        bodies.push(RecordBody::ToolCallSettled(tool_settled_record(
            &batch.opened,
            &batch.calls[index].assigned,
            message_id,
            env,
            BufferedToolResult {
                result,
                settlement_digest,
                synthetic,
                error,
            },
        )?));
        result_ids.push(message_id);
        settlements.push(batch.calls[index].assigned.effect_id);
        message_count += 1;
    }
    if result_ids.len() == batch.calls.len() {
        bodies.push(RecordBody::ToolBatchClosed(close_record(
            &batch.opened,
            result_ids,
            outcome_for_continuation(batch.opened.continuation),
        )?));
    }
    ensure_record_batch_bound(bodies.len())?;
    Ok(CancellationToolFollowups {
        bodies,
        settlements,
        messages: message_count,
    })
}

fn followup_records(
    batch: &mut ActiveToolBatch,
    env: &TransitionEnv,
) -> Result<ToolFollowups, KernelError> {
    let mut bodies = Vec::new();
    let mut actions = Vec::new();
    let current_group_complete = group_is_terminal(batch, batch.current_group);
    if batch.fatal_error.is_some() && current_group_complete {
        let abort_error = aborted_error()?;
        for call in Arc::make_mut(&mut batch.calls) {
            if matches!(call.status, ActiveToolCallStatus::Undispatched) {
                let result = synthetic_result(call.assigned.plan.call(), &abort_error)?;
                let digest = synthetic_tool_digest(
                    batch.opened.tool_batch_id,
                    *call.assigned.plan.call().tool_call_id(),
                    call.assigned.effect_id,
                    &result,
                    &abort_error,
                )?;
                call.status = ActiveToolCallStatus::Buffered {
                    result,
                    settlement_digest: digest,
                    synthetic: true,
                    error: Some(abort_error.clone()),
                };
            }
        }
    }

    let mut finalized_effects = Vec::new();
    let start =
        usize::try_from(batch.next_source_index).map_err(|_| KernelError::InvariantViolation)?;
    let mut message_index = 0_usize;
    // Accumulated across the loop and written back once. Rebuilding the shared
    // slice per iteration made finalizing a k-call batch O(k^2).
    let mut result_ids = batch.result_message_ids.to_vec();
    for index in start..batch.calls.len() {
        let ActiveToolCallStatus::Buffered {
            result,
            settlement_digest,
            synthetic,
            error,
        } = batch.calls[index].status.clone()
        else {
            break;
        };
        let message_id = required(env.ids.message_ids(), message_index, "message_ids")?;
        let settled = tool_settled_record(
            &batch.opened,
            &batch.calls[index].assigned,
            message_id,
            env,
            BufferedToolResult {
                result,
                settlement_digest,
                synthetic,
                error,
            },
        )?;
        bodies.push(RecordBody::ToolCallSettled(settled));
        Arc::make_mut(&mut batch.calls)[index].status = ActiveToolCallStatus::Settled {
            result_message_id: message_id,
            settlement_digest,
        };
        result_ids.push(message_id);
        batch.next_source_index = batch
            .next_source_index
            .checked_add(1)
            .ok_or(KernelError::InvariantViolation)?;
        finalized_effects.push(batch.calls[index].assigned.effect_id);
        message_index += 1;
    }
    batch.result_message_ids = result_ids.into();

    if batch.fatal_error.is_none()
        && current_group_complete
        && let Some(group) = next_executable_group(batch)
    {
        append_group_requests(&batch.opened.calls, group, &mut bodies, &mut actions)?;
        batch.current_group = group;
    }

    if batch
        .calls
        .iter()
        .all(|call| matches!(call.status, ActiveToolCallStatus::Settled { .. }))
    {
        let outcome = match &batch.fatal_error {
            Some(error) => ToolBatchOutcome::Failed {
                error: error.clone(),
            },
            None => outcome_for_continuation(batch.opened.continuation),
        };
        bodies.push(RecordBody::ToolBatchClosed(close_record(
            &batch.opened,
            batch.result_message_ids.to_vec(),
            outcome,
        )?));
    }
    Ok(ToolFollowups {
        bodies,
        actions,
        settlements: finalized_effects,
        messages: message_index,
    })
}

fn validate_plans(
    state: &KernelState,
    source: &[&ToolCallBlock],
    plans: &[ToolCallPlan],
) -> Result<(), KernelError> {
    if source.is_empty() || source.len() != plans.len() {
        return Err(KernelError::ToolBatchPlanMismatch);
    }
    for (source_call, plan) in source.iter().zip(plans) {
        let planned = plan.call();
        if source_call.tool_call_id() != planned.tool_call_id()
            || source_call.tool_name() != planned.tool_name()
            || source_call.arguments() != planned.arguments()
        {
            return Err(KernelError::ToolBatchPlanMismatch);
        }
        let identity = state
            .tool_calls
            .get(source_call.tool_call_id())
            .ok_or(KernelError::ToolBatchPlanMismatch)?;
        if identity.call != **source_call || identity.effect_id.is_some() {
            return Err(KernelError::ToolBatchPlanMismatch);
        }
        match plan {
            ToolCallPlan::Execute(call)
                if call.output_contract.kind != EffectOutputKind::ToolResult =>
            {
                return Err(KernelError::ToolEffectContractMismatch);
            }
            ToolCallPlan::SyntheticClosure(closure) => {
                validate_error_descriptor(&closure.error)?;
            }
            ToolCallPlan::Execute(_) => {}
        }
    }
    Ok(())
}

fn assign_plans(
    env: &TransitionEnv,
    plans: &[ToolCallPlan],
) -> Result<Vec<AssignedToolCall>, KernelError> {
    let mut assigned = Vec::with_capacity(plans.len());
    let groups = execution_groups(plans)?;
    for (index, (plan, group)) in plans.iter().zip(groups).enumerate() {
        assigned.push(AssignedToolCall {
            source_index: u32::try_from(index).map_err(|_| KernelError::InvalidInputPayload {
                field: "calls",
                reason_code: "too_many_items",
            })?,
            group_index: group,
            effect_id: required(env.ids.effect_ids(), index, "effect_ids")?,
            plan: plan.clone(),
        });
    }
    Ok(assigned)
}

fn execution_groups(plans: &[ToolCallPlan]) -> Result<Vec<u32>, KernelError> {
    let mut groups = Vec::with_capacity(plans.len());
    let mut group = 0_u32;
    for (index, plan) in plans.iter().enumerate() {
        if index > 0
            && !(plans[index - 1].execution() == crate::ToolExecutionMode::Parallel
                && plan.execution() == crate::ToolExecutionMode::Parallel)
        {
            group = group
                .checked_add(1)
                .ok_or(KernelError::InvalidInputPayload {
                    field: "calls",
                    reason_code: "too_many_items",
                })?;
        }
        groups.push(group);
    }
    Ok(groups)
}

fn validate_record_batch_bounds(plans: &[ToolCallPlan]) -> Result<(), KernelError> {
    let groups = execution_groups(plans)?;
    let executable_groups = plans
        .iter()
        .zip(&groups)
        .filter(|(plan, _)| matches!(plan, ToolCallPlan::Execute(_)))
        .map(|(_, group)| *group)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let leading_synthetic = plans
        .iter()
        .take_while(|plan| matches!(plan, ToolCallPlan::SyntheticClosure(_)))
        .count();
    let opening_requests = executable_groups.first().map_or(0, |group| {
        plans
            .iter()
            .zip(&groups)
            .filter(|(plan, assigned_group)| {
                **assigned_group == *group && matches!(plan, ToolCallPlan::Execute(_))
            })
            .count()
    });
    let opening_close = usize::from(executable_groups.is_empty());
    ensure_record_batch_bound(2 + leading_synthetic + opening_requests + opening_close)?;

    for (group_position, group) in executable_groups.iter().enumerate() {
        let start = plans
            .iter()
            .zip(&groups)
            .position(|(plan, assigned_group)| {
                *assigned_group == *group && matches!(plan, ToolCallPlan::Execute(_))
            })
            .ok_or(KernelError::InvariantViolation)?;
        let next_group = executable_groups.get(group_position + 1).copied();
        let end = next_group
            .and_then(|next| {
                plans
                    .iter()
                    .zip(&groups)
                    .position(|(plan, assigned_group)| {
                        *assigned_group == next && matches!(plan, ToolCallPlan::Execute(_))
                    })
            })
            .unwrap_or(plans.len());
        let next_requests = next_group.map_or(0, |next| {
            plans
                .iter()
                .zip(&groups)
                .filter(|(plan, assigned_group)| {
                    **assigned_group == next && matches!(plan, ToolCallPlan::Execute(_))
                })
                .count()
        });
        let close = usize::from(next_group.is_none());
        ensure_record_batch_bound(1 + (end - start) + next_requests + close)?;

        let group_can_fail_run = plans.iter().zip(&groups).any(|(plan, assigned_group)| {
            *assigned_group == *group
                && matches!(plan, ToolCallPlan::Execute(_))
                && plan.failure_policy() == ToolFailurePolicy::FailRun
        });
        if group_can_fail_run {
            ensure_record_batch_bound(1 + (plans.len() - start) + 1)?;
        }
    }
    Ok(())
}

fn ensure_record_batch_bound(count: usize) -> Result<(), KernelError> {
    if count > APPEND_BATCH_MAX_RECORDS {
        return Err(KernelError::InvalidInputPayload {
            field: "records",
            reason_code: "too_many_items",
        });
    }
    Ok(())
}

fn append_group_requests(
    assigned: &[AssignedToolCall],
    group: u32,
    bodies: &mut Vec<RecordBody>,
    actions: &mut Vec<PostCommitAction>,
) -> Result<(), KernelError> {
    for call in assigned.iter().filter(|call| call.group_index == group) {
        let ToolCallPlan::Execute(validated) = &call.plan else {
            continue;
        };
        let requested = EffectRequested::try_new(
            call.effect_id,
            EffectKind::Tool,
            None,
            validated.component.clone(),
            None,
            validated.output_contract.clone(),
            EffectInput::Tool {
                call: validated.call.clone(),
            },
            validated.retry_safety,
            validated.deadline,
        )
        .map_err(|_| KernelError::ToolEffectContractMismatch)?;
        bodies.push(RecordBody::EffectRequested(requested));
        actions.push(PostCommitAction::ExecuteEffect {
            effect_id: call.effect_id,
        });
    }
    Ok(())
}

fn tool_settled_record(
    opened: &ToolBatchOpened,
    assigned: &AssignedToolCall,
    message_id: crate::MessageId,
    env: &TransitionEnv,
    normalized: BufferedToolResult,
) -> Result<ToolCallSettled, KernelError> {
    let message = Message::try_new(
        message_id,
        MessageRole::Tool,
        vec![ContentBlock::ToolResult(normalized.result)],
        env.now,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .map_err(|_| KernelError::ToolResultMismatch)?;
    Ok(ToolCallSettled {
        cycle: opened.cycle,
        turn_id: opened.turn_id,
        tool_batch_id: opened.tool_batch_id,
        tool_call_id: *assigned.plan.call().tool_call_id(),
        effect_id: assigned.effect_id,
        message,
        settlement_digest: normalized.settlement_digest,
        synthetic: normalized.synthetic,
        error: normalized.error,
    })
}

struct BufferedToolResult {
    result: ToolResultBlock,
    settlement_digest: crate::Digest,
    synthetic: bool,
    error: Option<crate::ErrorDescriptor>,
}

fn close_record(
    opened: &ToolBatchOpened,
    result_message_ids: Vec<crate::MessageId>,
    outcome: ToolBatchOutcome,
) -> Result<ToolBatchClosed, KernelError> {
    let close_digest = tool_batch_close_digest(
        opened.cycle,
        opened.turn_id,
        opened.tool_batch_id,
        opened.source_message_id,
        &result_message_ids,
        &outcome,
    )?;
    Ok(ToolBatchClosed {
        cycle: opened.cycle,
        turn_id: opened.turn_id,
        tool_batch_id: opened.tool_batch_id,
        source_message_id: opened.source_message_id,
        result_message_ids: result_message_ids.into(),
        outcome,
        close_digest,
    })
}

pub(super) fn synthetic_result(
    call: &ToolCallBlock,
    error: &crate::ErrorDescriptor,
) -> Result<ToolResultBlock, KernelError> {
    let encoded = serde_json::to_string(error).map_err(|_| KernelError::InvariantViolation)?;
    let json = RawJson::parse(encoded).map_err(|_| KernelError::InvariantViolation)?;
    ToolResultBlock::try_new(
        *call.tool_call_id(),
        vec![ContentBlock::Json(JsonBlock::new(json))],
        true,
    )
    .map_err(|_| KernelError::InvariantViolation)
}

fn decode_tool_result(
    completion: &EffectCompleted,
    call: &ToolCallBlock,
) -> Result<ToolResultBlock, KernelError> {
    let result = serde_json::from_str::<ToolResultBlock>(completion.output().as_str())
        .map_err(|_| KernelError::ToolResultMismatch)?;
    if result.tool_call_id() != call.tool_call_id() {
        return Err(KernelError::ToolResultMismatch);
    }
    Ok(result)
}

fn assistant_calls(message: &Message) -> Vec<&ToolCallBlock> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if !crate::is_internal_tool_name(call.tool_name()) => {
                Some(call)
            }
            _ => None,
        })
        .collect()
}

fn first_executable_group(assigned: &[AssignedToolCall]) -> Option<u32> {
    assigned
        .iter()
        .find_map(|call| matches!(call.plan, ToolCallPlan::Execute(_)).then_some(call.group_index))
}

fn next_executable_group(batch: &ActiveToolBatch) -> Option<u32> {
    batch.calls.iter().find_map(|call| {
        (matches!(call.status, ActiveToolCallStatus::Undispatched)
            && matches!(call.assigned.plan, ToolCallPlan::Execute(_)))
        .then_some(call.assigned.group_index)
    })
}

fn group_is_terminal(batch: &ActiveToolBatch, group: u32) -> bool {
    batch.calls.iter().all(|call| {
        call.assigned.group_index != group
            || matches!(
                call.status,
                ActiveToolCallStatus::Buffered { .. } | ActiveToolCallStatus::Settled { .. }
            )
    })
}

fn outcome_for_continuation(continuation: ToolBatchContinuation) -> ToolBatchOutcome {
    match continuation {
        ToolBatchContinuation::ContinueModel => ToolBatchOutcome::ContinueModel,
        ToolBatchContinuation::Finalize => ToolBatchOutcome::Finalize,
    }
}

fn aborted_error() -> Result<crate::ErrorDescriptor, KernelError> {
    crate::ErrorDescriptor::new(
        "tool_batch_aborted",
        "tool call was not dispatched because the batch failed",
        crate::ErrorCategory::Tool,
        false,
    )
    .map_err(|_| KernelError::InvariantViolation)
}

pub(super) fn cancelled_error() -> Result<crate::ErrorDescriptor, KernelError> {
    crate::ErrorDescriptor::new(
        "cancelled",
        "tool call was cancelled before run termination",
        crate::ErrorCategory::Cancellation,
        false,
    )
    .map_err(|_| KernelError::InvariantViolation)
}

fn settlement_effect_id(outcome: &ToolSettlement) -> crate::EffectId {
    match outcome {
        ToolSettlement::Completed(value) => value.effect_id(),
        ToolSettlement::Deferred(value) => value.effect_id,
        ToolSettlement::Failed(value) => value.effect_id(),
    }
}

fn settlement_completion_id(outcome: &ToolSettlement) -> Option<&str> {
    match outcome {
        ToolSettlement::Completed(value) => value.completion_id(),
        ToolSettlement::Deferred(_) => None,
        ToolSettlement::Failed(value) => value.completion_id(),
    }
}

fn requirements_for_bodies(
    bodies: &[RecordBody],
    messages: usize,
    effects: usize,
    tool_batches: usize,
) -> Result<IdRequirements, KernelError> {
    let events = bodies.iter().try_fold(0_usize, |count, body| {
        let body_count = body
            .derived_event_count(crate::RECORD_KIND_VERSION)
            .map_err(|_| KernelError::InvariantViolation)?;
        count
            .checked_add(body_count)
            .ok_or(KernelError::InvariantViolation)
    })?;
    Ok(
        IdRequirements::new(bodies.len(), events, effects, 0, 0, messages)
            .with_tools(tool_batches, 0),
    )
}
