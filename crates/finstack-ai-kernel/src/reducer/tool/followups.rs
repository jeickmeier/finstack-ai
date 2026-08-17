//! Post-settlement tool follow-up records.

use super::super::decide::required;
use super::super::decision::KernelError;
use super::super::fingerprint::synthetic_tool_digest;
use crate::primitives::EffectId;
use crate::records::RecordBody;
use crate::records::tools::{ActiveToolBatch, ActiveToolCallStatus, ToolBatchOutcome};
use crate::state::TransitionEnv;

use super::cancellation::ToolFollowups;
use super::records::synthetic_result;
use super::records::{
    BufferedToolResult, aborted_error, append_group_requests, close_record, group_is_terminal,
    next_executable_group, outcome_for_continuation, tool_settled_record,
};

pub fn followup_records(
    batch: &mut ActiveToolBatch,
    env: &TransitionEnv,
) -> Result<ToolFollowups, KernelError> {
    let mut bodies = Vec::new();
    let mut actions = Vec::new();
    let current_group_complete = group_is_terminal(batch, batch.current_group);
    if batch.fatal_error.is_some() && current_group_complete {
        abort_undispatched_calls(batch)?;
    }

    let (finalized_effects, message_index) = finalize_buffered_prefix(batch, env, &mut bodies)?;

    if batch.fatal_error.is_none()
        && current_group_complete
        && let Some(group) = next_executable_group(batch)
    {
        append_group_requests(&batch.opened.calls, group, &mut bodies, &mut actions)?;
        batch.set_current_group(group);
    }

    if batch.all_settled() {
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

fn abort_undispatched_calls(batch: &mut ActiveToolBatch) -> Result<(), KernelError> {
    let abort_error = aborted_error()?;
    let abort_indexes = batch
        .calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            matches!(call.status, ActiveToolCallStatus::Undispatched).then_some(index)
        })
        .collect::<Vec<_>>();
    for index in abort_indexes {
        let call = &batch.calls[index];
        let result = synthetic_result(call.assigned.plan.call(), &abort_error)?;
        let digest = synthetic_tool_digest(
            batch.opened.tool_batch_id,
            *call.assigned.plan.call().tool_call_id(),
            call.assigned.effect_id,
            &result,
            &abort_error,
        )?;
        batch.set_call_status(
            index,
            ActiveToolCallStatus::Buffered {
                result,
                settlement_digest: digest,
                synthetic: true,
                error: Some(abort_error.clone()),
            },
        );
    }
    Ok(())
}

fn finalize_buffered_prefix(
    batch: &mut ActiveToolBatch,
    env: &TransitionEnv,
    bodies: &mut Vec<RecordBody>,
) -> Result<(Vec<EffectId>, usize), KernelError> {
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
        batch.set_call_status(
            index,
            ActiveToolCallStatus::Settled {
                result_message_id: message_id,
                settlement_digest,
            },
        );
        result_ids.push(message_id);
        batch.next_source_index = batch
            .next_source_index
            .checked_add(1)
            .ok_or(KernelError::InvariantViolation)?;
        batch.note_source_advanced();
        finalized_effects.push(batch.calls[index].assigned.effect_id);
        message_index += 1;
    }
    batch.result_message_ids = result_ids.into();
    Ok((finalized_effects, message_index))
}
