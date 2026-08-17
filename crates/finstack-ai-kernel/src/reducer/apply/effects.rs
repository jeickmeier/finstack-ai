use std::sync::Arc;

use crate::content::ContentBlock;
use crate::effects::{EffectKind, EffectOutputKind};
use crate::primitives::Digest;
use crate::records::RecordBody;
use crate::records::lifecycle::EntryAppended;
use crate::records::tools::{ActiveToolCallStatus, ToolCallIdentity};
use crate::state::{
    CompletionIdentity, KernelState, ModelSettlementFingerprint, ModelSettlementKind,
    PendingModelEffect, RunPhase, TerminalCandidate,
};

use super::super::decision::KernelError;
use super::super::fingerprint::{completed_record_digest, failed_record_digest};
use super::interactions::{apply_interaction_effect_requested, apply_interaction_effect_terminal};
use super::tools::{apply_tool_effect_completed, apply_tool_effect_failed};

pub(super) fn apply_limit_reached(
    state: &mut KernelState,
    reached: &crate::LimitReached,
) -> Result<(), KernelError> {
    reached
        .validate()
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    state.limit_usage = reached.usage.clone();
    Ok(())
}

pub(super) fn apply_effect_requested(
    state: &mut KernelState,
    requested: &crate::EffectRequested,
    next: Option<&RecordBody>,
) -> Result<(), KernelError> {
    if requested.kind() == EffectKind::Interaction {
        return apply_interaction_effect_requested(state, requested, next);
    }
    if requested.kind() == EffectKind::Timer {
        let pending = state
            .retry
            .pending
            .as_ref()
            .ok_or(KernelError::InvalidRecordOrder)?;
        if pending.timer_effect_id != requested.effect_id()
            || requested.output_contract().kind != EffectOutputKind::TimerFiring
            || !matches!(requested.input(), crate::EffectInput::Timer { due_at } if *due_at == pending.due_at)
        {
            return Err(KernelError::InvalidRecordOrder);
        }
        state.phase = Some(RunPhase::Sleeping);
        return Ok(());
    }
    if requested.kind() == EffectKind::Model {
        state.limit_usage.model_requests = state
            .limit_usage
            .model_requests
            .checked_add(1)
            .ok_or(KernelError::InvalidRecordOrder)?;
    }
    if requested.kind() == EffectKind::Tool {
        let batch = state
            .active_tool_batch
            .as_mut()
            .ok_or(KernelError::InvalidRecordOrder)?;
        let call_index = batch
            .calls
            .iter()
            .position(|call| call.assigned.effect_id == requested.effect_id())
            .ok_or(KernelError::InvalidRecordOrder)?;
        let active_call = &batch.calls[call_index];
        if !matches!(active_call.status, ActiveToolCallStatus::Undispatched)
            || active_call.assigned.group_index < batch.current_group
            || requested.output_contract().kind != EffectOutputKind::ToolResult
            || !matches!(requested.input(), crate::EffectInput::Tool { call: input } if input == active_call.assigned.plan.call())
        {
            return Err(KernelError::InvalidRecordOrder);
        }
        let requested_group = active_call.assigned.group_index;
        if requested_group > batch.current_group
            && batch.calls.iter().any(|call| {
                call.assigned.group_index < requested_group
                    && !matches!(call.status, ActiveToolCallStatus::Settled { .. })
            })
        {
            return Err(KernelError::InvalidRecordOrder);
        }
        let active_call = &mut Arc::make_mut(&mut batch.calls)[call_index];
        batch.current_group = requested_group;
        active_call.status = ActiveToolCallStatus::Requested {
            requested: requested.clone(),
            deferred: None,
        };
        state.phase = Some(RunPhase::AwaitingTools);
        return Ok(());
    }
    let turn = state
        .current_turn
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let model_request_id = turn
        .model_request_id
        .ok_or(KernelError::InvariantViolation)?;
    state.pending_model_effect = Some(PendingModelEffect {
        cycle: state.cycle,
        turn_id: turn.turn_id,
        model_request_id,
        requested: requested.clone(),
        deferred: None,
    });
    state.phase = Some(RunPhase::AwaitingModel);
    Ok(())
}

pub(super) fn update_wall_usage(
    state: &mut KernelState,
    now: crate::Timestamp,
) -> Result<(), KernelError> {
    let Some(accepted_at) = state.accepted_at else {
        return Ok(());
    };
    let elapsed = now
        .as_unix_ms()
        .checked_sub(accepted_at.as_unix_ms())
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(KernelError::InvalidRecordOrder)?;
    state.limit_usage.wall_time = crate::Duration::from_millis(elapsed);
    Ok(())
}

pub(super) fn apply_completed_usage(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
) -> Result<(), KernelError> {
    state.limit_usage.output_bytes = state
        .limit_usage
        .output_bytes
        .checked_add(
            u64::try_from(completed.output().as_bytes().len())
                .map_err(|_| KernelError::InvalidRecordOrder)?,
        )
        .ok_or(KernelError::InvalidRecordOrder)?;
    if completed.usage().and_then(crate::Usage::cost).is_none() {
        reserve_unknown_cost(state)?;
    }
    let Some(usage) = completed.usage() else {
        return Ok(());
    };
    if let Some(value) = usage.input_tokens() {
        state.limit_usage.input_tokens = state
            .limit_usage
            .input_tokens
            .checked_add(value)
            .ok_or(KernelError::InvalidRecordOrder)?;
    }
    if let Some(value) = usage.output_tokens() {
        state.limit_usage.output_tokens = state
            .limit_usage
            .output_tokens
            .checked_add(value)
            .ok_or(KernelError::InvalidRecordOrder)?;
    }
    if let Some(value) = usage.cost() {
        let current = state
            .limit_usage
            .cost
            .as_ref()
            .map_or(0, crate::CostAmount::micros);
        let micros = current
            .checked_add(value.micros())
            .ok_or(KernelError::InvalidRecordOrder)?;
        state.limit_usage.cost = Some(
            crate::CostAmount::try_new(value.unit(), micros, value.pricing_policy_version())
                .map_err(|_| KernelError::InvalidRecordOrder)?,
        );
    }
    for (key, delta) in usage.extension_counters() {
        let current = state
            .limit_usage
            .extension_counters
            .get(key)
            .copied()
            .unwrap_or(0);
        state.limit_usage.extension_counters.insert(
            key.clone(),
            current
                .checked_add(*delta)
                .ok_or(KernelError::InvalidRecordOrder)?,
        );
    }
    Ok(())
}

fn reserve_unknown_cost(state: &mut KernelState) -> Result<(), KernelError> {
    let Some(maximum) = state
        .accepted
        .as_ref()
        .and_then(|accepted| accepted.limits().max_cost.as_ref())
    else {
        return Ok(());
    };
    if maximum.unknown_usage() != crate::UnknownUsagePolicy::AllowWithinReservedMaximum {
        return Ok(());
    }
    state.limit_usage.cost = Some(
        crate::CostAmount::try_new(
            maximum.unit(),
            maximum.micros(),
            maximum.pricing_policy_version(),
        )
        .map_err(|_| KernelError::InvalidRecordOrder)?,
    );
    Ok(())
}

pub(super) fn apply_effect_deferred(
    state: &mut KernelState,
    deferred: &crate::EffectDeferred,
) -> Result<(), KernelError> {
    if deferred.output_contract.kind == EffectOutputKind::ToolResult {
        let batch = state
            .active_tool_batch
            .as_mut()
            .ok_or(KernelError::InvalidRecordOrder)?;
        let call = Arc::make_mut(&mut batch.calls)
            .iter_mut()
            .find(|call| call.assigned.effect_id == deferred.effect_id)
            .ok_or(KernelError::InvalidRecordOrder)?;
        let ActiveToolCallStatus::Requested {
            requested,
            deferred: existing,
        } = &mut call.status
        else {
            return Err(KernelError::InvalidRecordOrder);
        };
        deferred
            .validate_against(requested)
            .map_err(|_| KernelError::ToolSettlementMismatch)?;
        if existing.is_some() {
            return Err(KernelError::InvalidRecordOrder);
        }
        *existing = Some(deferred.clone());
        state.phase = Some(RunPhase::AwaitingExternal);
        return Ok(());
    }
    let pending = state
        .pending_model_effect
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    pending.deferred = Some(deferred.clone());
    state.phase = Some(RunPhase::AwaitingExternal);
    Ok(())
}

pub(super) fn apply_effect_completed(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
    next: Option<&RecordBody>,
) -> Result<(), KernelError> {
    if completed.output_contract().kind == EffectOutputKind::InteractionResolution {
        return apply_interaction_effect_terminal(
            state,
            completed.effect_id(),
            completed.completion_id(),
            completed.output_digest(),
        );
    }
    if completed.output_contract().kind == EffectOutputKind::ToolResult {
        return apply_tool_effect_completed(state, completed);
    }
    let RecordBody::EntryAppended(entry) = next.ok_or(KernelError::InvalidRecordOrder)? else {
        return Err(KernelError::InvalidRecordOrder);
    };
    index_completed_settlement(state, completed, entry)?;
    state.terminal_candidate = Some(TerminalCandidate::Completed {
        cycle: entry.cycle,
        turn_id: entry.turn_id,
        model_request_id: entry.model_request_id,
        effect_id: entry.effect_id,
        message_id: *entry.message.id(),
        result_digest: completed.output_digest(),
    });
    Ok(())
}

pub(super) fn apply_entry_appended(
    state: &mut KernelState,
    entry: &EntryAppended,
) -> Result<(), KernelError> {
    let message_id = *entry.message.id();
    let mut has_tool_calls = false;
    for block in entry.message.content() {
        if let ContentBlock::ToolCall(call) = block
            && !crate::is_internal_tool_name(call.tool_name())
        {
            has_tool_calls = true;
            if state.tool_calls.contains_key(call.tool_call_id()) {
                return Err(KernelError::DuplicateToolCall);
            }
            state.tool_calls.insert(
                *call.tool_call_id(),
                ToolCallIdentity {
                    cycle: entry.cycle,
                    turn_id: entry.turn_id,
                    source_message_id: message_id,
                    tool_batch_id: None,
                    effect_id: None,
                    call: call.clone(),
                },
            );
        }
    }
    if has_tool_calls {
        state.state_version = state.state_version.max(2);
    }
    Arc::make_mut(&mut state.messages).push(entry.message.clone());
    if let Some(turn) = state.current_turn.as_mut() {
        turn.final_message_id = Some(message_id);
    }
    if !matches!(
        state.terminal_candidate,
        Some(TerminalCandidate::Completed {
            effect_id,
            message_id: candidate_message_id,
            ..
        }) if effect_id == entry.effect_id && candidate_message_id == message_id
    ) {
        return Err(KernelError::InvariantViolation);
    }
    state.pending_model_effect = None;
    state.phase = Some(RunPhase::AfterModel);
    Ok(())
}

pub(super) fn apply_effect_failed(
    state: &mut KernelState,
    failed: &crate::EffectFailed,
) -> Result<(), KernelError> {
    if failed.output_contract().kind == EffectOutputKind::InteractionResolution {
        return apply_interaction_effect_terminal(
            state,
            failed.effect_id(),
            failed.completion_id(),
            crate::Digest::raw_json(b"interaction-effect-failed"),
        );
    }
    if failed.output_contract().kind == EffectOutputKind::ToolResult {
        return apply_tool_effect_failed(state, failed);
    }
    index_failed_settlement(state, failed)?;
    let pending = state
        .pending_model_effect
        .take()
        .ok_or(KernelError::InvariantViolation)?;
    state.terminal_candidate = Some(TerminalCandidate::Failed {
        cycle: pending.cycle,
        turn_id: Some(pending.turn_id),
        model_request_id: Some(pending.model_request_id),
        effect_id: Some(pending.requested.effect_id()),
        error: failed.error().clone(),
    });
    state.phase = Some(RunPhase::BeforeFinalize);
    Ok(())
}

pub(super) fn index_completed_settlement(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
    entry: &EntryAppended,
) -> Result<(), KernelError> {
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let digest = completed_record_digest(pending, completed, &entry.message)?;
    insert_model_identity(
        state,
        completed.effect_id(),
        ModelSettlementKind::Completed,
        digest,
        completed.completion_id(),
    )
}

pub(super) fn index_failed_settlement(
    state: &mut KernelState,
    failed: &crate::EffectFailed,
) -> Result<(), KernelError> {
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let digest = failed_record_digest(pending, failed)?;
    insert_model_identity(
        state,
        failed.effect_id(),
        ModelSettlementKind::Failed,
        digest,
        failed.completion_id(),
    )
}

pub(super) fn insert_model_identity(
    state: &mut KernelState,
    effect_id: crate::EffectId,
    kind: ModelSettlementKind,
    digest: Digest,
    completion_id: Option<&str>,
) -> Result<(), KernelError> {
    if let Some(existing) = state.model_settlements.get(&effect_id)
        && (existing.kind != kind || existing.digest != digest)
    {
        return Err(KernelError::ConflictingSettlement);
    }
    state
        .model_settlements
        .insert(effect_id, ModelSettlementFingerprint { kind, digest });
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
