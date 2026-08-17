use std::sync::Arc;

use crate::content::ContentBlock;
use crate::conversation::MessageRole;
use crate::effects::EffectOutputKind;
use crate::primitives::Digest;
use crate::records::RecordEnvelope;
use crate::records::tools::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, ToolBatchOutcome, ToolCallPlan,
    ToolSettlementFingerprint, ToolSettlementKind,
};
use crate::state::{CompletionIdentity, KernelState, RunPhase, TerminalCandidate};

use super::super::decision::KernelError;
use super::super::fingerprint::{
    completed_tool_record_digest, failed_tool_record_digest, synthetic_tool_digest,
};
use super::super::tool::decode_tool_result;

#[expect(
    clippy::too_many_lines,
    reason = "batch opening validates source order, grouping, identities, and replay state atomically"
)]
pub(super) fn apply_tool_batch_opened(
    state: &mut KernelState,
    opened: &crate::ToolBatchOpened,
) -> Result<(), KernelError> {
    if state.active_tool_batch.is_some()
        || opened.cycle != state.cycle
        || state
            .current_turn
            .as_ref()
            .is_none_or(|turn| turn.turn_id != opened.turn_id)
        || state
            .messages
            .last()
            .is_none_or(|message| *message.id() != opened.source_message_id)
        || opened.calls.is_empty()
        || opened.calls.len() > crate::SEMANTIC_ARRAY_MAX_ITEMS
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    let source_calls = source_tool_calls_for_open(state, opened)?;
    let mut calls = Vec::with_capacity(opened.calls.len());
    let mut prior_group = 0_u32;
    for (index, assigned) in opened.calls.iter().enumerate() {
        let source_index = u32::try_from(index).map_err(|_| KernelError::InvalidRecordOrder)?;
        let identity = state
            .tool_calls
            .get_mut(assigned.plan.call().tool_call_id())
            .ok_or(KernelError::ToolBatchPlanMismatch)?;
        if assigned.source_index != source_index
            || assigned.plan.call() != &source_calls[index]
            || identity.call != *assigned.plan.call()
            || identity.source_message_id != opened.source_message_id
            || identity.effect_id.is_some()
        {
            return Err(KernelError::ToolBatchPlanMismatch);
        }
        if index == 0 {
            if assigned.group_index != 0 {
                return Err(KernelError::ToolBatchPlanMismatch);
            }
        } else {
            let prior_mode = opened.calls[index - 1].plan.execution();
            let expected = if prior_mode == crate::ToolExecutionMode::Parallel
                && assigned.plan.execution() == crate::ToolExecutionMode::Parallel
            {
                prior_group
            } else {
                prior_group
                    .checked_add(1)
                    .ok_or(KernelError::ToolBatchPlanMismatch)?
            };
            if assigned.group_index != expected {
                return Err(KernelError::ToolBatchPlanMismatch);
            }
        }
        prior_group = assigned.group_index;
        identity.tool_batch_id = Some(opened.tool_batch_id);
        identity.effect_id = Some(assigned.effect_id);
        let status = match &assigned.plan {
            ToolCallPlan::Execute(call) => {
                if call.output_contract.kind != EffectOutputKind::ToolResult {
                    return Err(KernelError::ToolEffectContractMismatch);
                }
                ActiveToolCallStatus::Undispatched
            }
            ToolCallPlan::SyntheticClosure(closure) => {
                let result = super::super::tool::synthetic_result(&closure.call, &closure.error)?;
                let digest = synthetic_tool_digest(
                    opened.tool_batch_id,
                    *closure.call.tool_call_id(),
                    assigned.effect_id,
                    &result,
                    &closure.error,
                )?;
                ActiveToolCallStatus::Buffered {
                    result,
                    settlement_digest: digest,
                    synthetic: true,
                    error: Some(closure.error.clone()),
                }
            }
        };
        calls.push(ActiveToolCall {
            assigned: assigned.clone(),
            status,
        });
    }
    let current_group = calls
        .iter()
        .find_map(|call| {
            matches!(call.assigned.plan, ToolCallPlan::Execute(_))
                .then_some(call.assigned.group_index)
        })
        .unwrap_or(0);
    state.active_tool_batch = Some(ActiveToolBatch {
        opened: opened.clone(),
        calls: calls.into(),
        current_group,
        next_source_index: 0,
        result_message_ids: Arc::from([]),
        fatal_error: None,
    });
    state.last_tool_batch = None;
    state.phase = Some(RunPhase::AwaitingTools);
    Ok(())
}

pub(super) fn source_tool_calls_for_open(
    state: &KernelState,
    opened: &crate::ToolBatchOpened,
) -> Result<Vec<crate::ToolCallBlock>, KernelError> {
    let source = state
        .messages
        .last()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let calls = source
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if !crate::is_internal_tool_name(call.tool_name()) => {
                Some(call.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if calls.len() != opened.calls.len() {
        return Err(KernelError::ToolBatchPlanMismatch);
    }
    Ok(calls)
}

pub(super) fn apply_tool_effect_completed(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let index = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == completed.effect_id())
        .ok_or(KernelError::InvalidRecordOrder)?;
    let (requested, external, planned_call) = match &batch.calls[index].status {
        ActiveToolCallStatus::Requested {
            requested,
            deferred,
        } => (
            requested.clone(),
            deferred.is_some(),
            batch.calls[index].assigned.plan.call().clone(),
        ),
        _ => return Err(KernelError::InvalidRecordOrder),
    };
    completed
        .validate_against(&requested)
        .map_err(|_| KernelError::ToolSettlementMismatch)?;
    let result = decode_tool_result(completed, &planned_call)?;
    let digest = completed_tool_record_digest(batch.opened.tool_batch_id, external, completed)?;
    insert_tool_identity(
        state,
        completed.effect_id(),
        ToolSettlementKind::Completed,
        digest,
        completed.completion_id(),
    )?;
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut batch.calls)[index].status = ActiveToolCallStatus::Buffered {
        result,
        settlement_digest: digest,
        synthetic: false,
        error: None,
    };
    state.phase = Some(tool_wait_phase(batch));
    Ok(())
}

pub(super) fn apply_tool_effect_failed(
    state: &mut KernelState,
    failed: &crate::EffectFailed,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let index = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == failed.effect_id())
        .ok_or(KernelError::InvalidRecordOrder)?;
    let (requested, external, policy, planned_call) = match &batch.calls[index].status {
        ActiveToolCallStatus::Requested {
            requested,
            deferred,
        } => (
            requested.clone(),
            deferred.is_some(),
            batch.calls[index].assigned.plan.failure_policy(),
            batch.calls[index].assigned.plan.call().clone(),
        ),
        _ => return Err(KernelError::InvalidRecordOrder),
    };
    failed
        .validate_against(&requested)
        .map_err(|_| KernelError::ToolSettlementMismatch)?;
    let digest = failed_tool_record_digest(batch.opened.tool_batch_id, external, failed)?;
    let result = super::super::tool::synthetic_result(&planned_call, failed.error())?;
    insert_tool_identity(
        state,
        failed.effect_id(),
        ToolSettlementKind::Failed,
        digest,
        failed.completion_id(),
    )?;
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut batch.calls)[index].status = ActiveToolCallStatus::Buffered {
        result,
        settlement_digest: digest,
        synthetic: true,
        error: Some(failed.error().clone()),
    };
    if policy == crate::ToolFailurePolicy::FailRun && batch.fatal_error.is_none() {
        batch.fatal_error = Some(failed.error().clone());
    }
    state.phase = Some(tool_wait_phase(batch));
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "the frozen source-order, authorship, and digest checks are one atomic apply invariant"
)]
pub(super) fn apply_tool_call_settled(
    state: &mut KernelState,
    settled: &crate::ToolCallSettled,
    record: &RecordEnvelope,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let index =
        usize::try_from(batch.next_source_index).map_err(|_| KernelError::InvalidRecordOrder)?;
    let call = batch
        .calls
        .get(index)
        .ok_or(KernelError::InvalidRecordOrder)?;
    if settled.cycle != batch.opened.cycle
        || settled.turn_id != batch.opened.turn_id
        || settled.tool_batch_id != batch.opened.tool_batch_id
        || settled.tool_call_id != *call.assigned.plan.call().tool_call_id()
        || settled.effect_id != call.assigned.effect_id
        || settled.message.role() != MessageRole::Tool
        || settled.message.created_at() != record.timestamp()
        || settled.message.model().is_some()
        || settled.message.provider_ids() != &crate::ProviderIds::empty()
        || settled.message.metadata() != &crate::Metadata::empty()
        || settled.message.content().len() != 1
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    let ContentBlock::ToolResult(message_result) = &settled.message.content()[0] else {
        return Err(KernelError::InvalidRecordOrder);
    };
    if message_result.tool_call_id() != call.assigned.plan.call().tool_call_id() {
        return Err(KernelError::ToolResultMismatch);
    }
    match &call.status {
        ActiveToolCallStatus::Buffered {
            result,
            settlement_digest,
            synthetic,
            error,
        } if result == message_result
            && *settlement_digest == settled.settlement_digest
            && *synthetic == settled.synthetic
            && error == &settled.error => {}
        ActiveToolCallStatus::Undispatched
            if batch.fatal_error.is_some()
                && settled.synthetic
                && settled
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code.as_str() == "tool_batch_aborted") =>
        {
            let error = settled
                .error
                .as_ref()
                .ok_or(KernelError::ToolResultMismatch)?;
            let expected = super::super::tool::synthetic_result(call.assigned.plan.call(), error)?;
            if expected != *message_result
                || synthetic_tool_digest(
                    settled.tool_batch_id,
                    settled.tool_call_id,
                    settled.effect_id,
                    message_result,
                    error,
                )? != settled.settlement_digest
            {
                return Err(KernelError::SettlementDigestMismatch);
            }
        }
        _ => return Err(KernelError::InvalidRecordOrder),
    }
    if settled.synthetic {
        let error = settled
            .error
            .as_ref()
            .ok_or(KernelError::ToolResultMismatch)?;
        if !message_result.is_error() {
            return Err(KernelError::SettlementDigestMismatch);
        }
        let synthetic_digest = synthetic_tool_digest(
            settled.tool_batch_id,
            settled.tool_call_id,
            settled.effect_id,
            message_result,
            error,
        )?;
        if let Some(existing) = state.tool_settlements.get(&settled.effect_id) {
            if existing.digest != settled.settlement_digest
                || (existing.kind == ToolSettlementKind::Synthetic
                    && synthetic_digest != settled.settlement_digest)
                || !matches!(
                    existing.kind,
                    ToolSettlementKind::Failed | ToolSettlementKind::Synthetic
                )
            {
                return Err(KernelError::SettlementDigestMismatch);
            }
        } else {
            if synthetic_digest != settled.settlement_digest {
                return Err(KernelError::SettlementDigestMismatch);
            }
            insert_tool_identity(
                state,
                settled.effect_id,
                ToolSettlementKind::Synthetic,
                settled.settlement_digest,
                None,
            )?;
        }
    } else if settled.error.is_some() {
        return Err(KernelError::ToolResultMismatch);
    }
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut batch.calls)[index].status = ActiveToolCallStatus::Settled {
        result_message_id: *settled.message.id(),
        settlement_digest: settled.settlement_digest,
    };
    let mut result_ids = batch.result_message_ids.to_vec();
    result_ids.push(*settled.message.id());
    batch.result_message_ids = result_ids.into();
    batch.next_source_index = batch
        .next_source_index
        .checked_add(1)
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut state.messages).push(settled.message.clone());
    Ok(())
}

pub(super) fn apply_tool_batch_closed(
    state: &mut KernelState,
    closed: &crate::ToolBatchClosed,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if closed.cycle != batch.opened.cycle
        || closed.turn_id != batch.opened.turn_id
        || closed.tool_batch_id != batch.opened.tool_batch_id
        || closed.source_message_id != batch.opened.source_message_id
        || closed.result_message_ids != batch.result_message_ids
        || !batch
            .calls
            .iter()
            .all(|call| matches!(call.status, ActiveToolCallStatus::Settled { .. }))
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    match (
        &batch.fatal_error,
        &batch.opened.continuation,
        &closed.outcome,
    ) {
        (Some(expected), _, ToolBatchOutcome::Failed { error }) if expected == error => {}
        (None, crate::ToolBatchContinuation::ContinueModel, ToolBatchOutcome::ContinueModel)
        | (None, crate::ToolBatchContinuation::Finalize, ToolBatchOutcome::Finalize) => {}
        _ => return Err(KernelError::InvalidRecordOrder),
    }
    if let ToolBatchOutcome::Failed { error } = &closed.outcome {
        state.terminal_candidate = Some(match state.terminal_candidate.as_ref() {
            Some(TerminalCandidate::Completed {
                cycle,
                turn_id,
                model_request_id,
                effect_id,
                ..
            }) => TerminalCandidate::Failed {
                cycle: *cycle,
                turn_id: Some(*turn_id),
                model_request_id: Some(*model_request_id),
                effect_id: Some(*effect_id),
                error: error.clone(),
            },
            _ => return Err(KernelError::InvariantViolation),
        });
    }
    state.last_tool_batch = Some(closed.clone());
    state.active_tool_batch = None;
    state.phase = Some(RunPhase::AfterToolBatch);
    Ok(())
}

pub(super) fn insert_tool_identity(
    state: &mut KernelState,
    effect_id: crate::EffectId,
    kind: ToolSettlementKind,
    digest: Digest,
    completion_id: Option<&str>,
) -> Result<(), KernelError> {
    if let Some(existing) = state.tool_settlements.get(&effect_id)
        && (existing.kind != kind || existing.digest != digest)
    {
        return Err(KernelError::ConflictingSettlement);
    }
    state
        .tool_settlements
        .insert(effect_id, ToolSettlementFingerprint { kind, digest });
    if let Some(completion_id) = completion_id {
        if let Some(existing) = state.completion_identities.get(completion_id)
            && (existing.effect_id != effect_id || existing.settlement_digest != digest)
        {
            return Err(KernelError::ConflictingCompletionId);
        }
        state.completion_identities.insert(
            Arc::from(completion_id),
            CompletionIdentity {
                effect_id,
                settlement_digest: digest,
            },
        );
    }
    Ok(())
}

pub(super) fn tool_wait_phase(batch: &ActiveToolBatch) -> RunPhase {
    if batch.calls.iter().any(|call| {
        matches!(
            call.status,
            ActiveToolCallStatus::Requested {
                deferred: Some(_),
                ..
            }
        )
    }) {
        RunPhase::AwaitingExternal
    } else {
        RunPhase::AwaitingTools
    }
}
