//! Cancellation buffering and follow-up construction.

use std::sync::Arc;

use super::super::decide::required;
use super::super::decision::{KernelError, PostCommitAction};
use super::super::fingerprint::synthetic_tool_digest;
use crate::effects::EffectCancelled;
use crate::records::RecordBody;
use crate::records::tools::{ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus};
use crate::state::{KernelState, TransitionEnv};

use super::planning::ensure_record_batch_bound;
use super::records::synthetic_result;
use super::records::{
    BufferedToolResult, cancelled_error, close_record, outcome_for_continuation,
    tool_settled_record,
};

/// Records, post-commit actions, and growth accounting produced after a tool
/// settlement or a cancellation closes part of the active batch.
#[derive(Default)]
pub struct ToolFollowups {
    pub bodies: Vec<RecordBody>,
    pub actions: Vec<PostCommitAction>,
    pub settlements: Vec<crate::EffectId>,
    pub messages: usize,
}

pub(crate) fn buffer_cancelled_effect(
    batch: &mut ActiveToolBatch,
    effect_id: crate::EffectId,
) -> Result<bool, KernelError> {
    let error = cancelled_error()?;
    let batch_id = batch.opened.tool_batch_id;
    let Some(index) = batch.call_index(effect_id) else {
        return Ok(false);
    };
    if !matches!(
        batch.calls[index].status,
        ActiveToolCallStatus::Requested { .. }
    ) {
        return Ok(false);
    }
    let mut call = batch.calls[index].clone();
    buffer_cancellation_result(&mut call, batch_id, &error)?;
    batch.set_call_status(index, call.status);
    Ok(true)
}

pub(crate) fn buffer_reconciled_tool_closures(
    batch: &mut ActiveToolBatch,
    completed_effects: &[crate::EffectId],
) -> Result<(), KernelError> {
    let error = cancelled_error()?;
    let batch_id = batch.opened.tool_batch_id;
    let indexes = batch
        .calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            let completed = completed_effects
                .binary_search(&call.assigned.effect_id)
                .is_ok();
            (matches!(call.status, ActiveToolCallStatus::Undispatched)
                || (completed && matches!(call.status, ActiveToolCallStatus::Requested { .. })))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    for index in indexes {
        let mut call = batch.calls[index].clone();
        buffer_cancellation_result(&mut call, batch_id, &error)?;
        batch.set_call_status(index, call.status);
    }
    Ok(())
}

pub(super) fn buffer_cancellation_result(
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
pub(crate) fn cancellation_followups(
    state: &KernelState,
    env: &TransitionEnv,
    newly_completed: &[crate::EffectId],
    newly_cancelled: &[crate::EffectId],
) -> Result<ToolFollowups, KernelError> {
    let Some(mut batch) = state.active_tool_batch.clone() else {
        return Ok(ToolFollowups::default());
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
    Ok(ToolFollowups {
        bodies,
        actions: Vec::new(),
        settlements,
        messages: message_count,
    })
}
