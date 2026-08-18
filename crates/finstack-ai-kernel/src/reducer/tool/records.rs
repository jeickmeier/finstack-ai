//! Tool-batch record construction helpers.

use super::super::allocated_ids::IdRequirements;
use super::super::decision::{KernelError, PostCommitAction};
use super::super::fingerprint::tool_batch_close_digest;
use crate::content::{ContentBlock, JsonBlock, ToolCallBlock, ToolResultBlock};
use crate::conversation::{Message, MessageRole, ProviderIds};
use crate::effects::{EffectCompleted, EffectInput, EffectKind, EffectRequested};
use crate::primitives::{Metadata, RawJson};
use crate::records::RecordBody;
use crate::records::tools::{
    AssignedToolCall, ToolBatchClosed, ToolBatchContinuation, ToolBatchOpened, ToolBatchOutcome,
    ToolCallPlan, ToolCallSettled,
};
use crate::state::TransitionEnv;

pub(super) fn append_group_requests(
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

pub(super) fn tool_settled_record(
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

pub struct BufferedToolResult {
    pub(super) result: ToolResultBlock,
    pub(super) settlement_digest: crate::Digest,
    pub(super) synthetic: bool,
    pub(super) error: Option<crate::ErrorDescriptor>,
}

pub(super) fn close_record(
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

pub(crate) fn synthetic_result(
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

pub(crate) fn decode_tool_result(
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

pub(super) fn first_executable_group(assigned: &[AssignedToolCall]) -> Option<u32> {
    assigned
        .iter()
        .find_map(|call| matches!(call.plan, ToolCallPlan::Execute(_)).then_some(call.group_index))
}

pub(super) fn outcome_for_continuation(continuation: ToolBatchContinuation) -> ToolBatchOutcome {
    match continuation {
        ToolBatchContinuation::ContinueModel => ToolBatchOutcome::ContinueModel,
        ToolBatchContinuation::Finalize => ToolBatchOutcome::Finalize,
    }
}

pub(super) fn aborted_error() -> Result<crate::ErrorDescriptor, KernelError> {
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

pub(super) fn requirements_for_bodies(
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
