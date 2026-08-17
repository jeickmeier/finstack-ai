//! Post-settlement tool follow-up records.

use std::sync::Arc;

use super::super::decide::required;
use super::super::decision::KernelError;
use super::super::fingerprint::synthetic_tool_digest;
use crate::records::RecordBody;
use crate::state::TransitionEnv;
use crate::tools::{ActiveToolBatch, ActiveToolCallStatus, ToolBatchOutcome};

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
