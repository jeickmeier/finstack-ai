//! Semantic validations shared by reducer decision paths.

use crate::content::ContentBlock;
use crate::conversation::{Message, MessageRole};
use crate::effects::EffectCompleted;
use crate::primitives::ErrorDescriptor;
use crate::primitives::{MessageId, ToolCallId};
use crate::state::{KernelState, TransitionEnv};

use super::KernelError;

pub(super) fn validate_error_descriptor(error: &ErrorDescriptor) -> Result<(), KernelError> {
    error
        .validate()
        .map_err(|_| KernelError::InvalidInputPayload {
            field: "error.message",
            reason_code: "invalid_text",
        })
}

pub(super) fn validate_assistant_semantics(
    state: &KernelState,
    env: &TransitionEnv,
    message: &Message,
    completion: &EffectCompleted,
) -> Result<(), KernelError> {
    if message.created_at() != env.now
        || message.role() != MessageRole::Assistant
        || message.provider_ids() != completion.provider_ids()
    {
        return Err(KernelError::AssistantMessageMismatch);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut final_output_calls = 0_usize;
    for call in assistant_tool_calls(message, false) {
        if !seen.insert(*call.tool_call_id()) || state.tool_calls.contains_key(call.tool_call_id())
        {
            return Err(KernelError::DuplicateToolCall);
        }
        if crate::is_internal_tool_name(call.tool_name()) {
            if call.tool_name() != crate::SUBMIT_FINAL_OUTPUT_TOOL
                || !matches!(
                    state.output_configuration,
                    Some(crate::OutputConfiguration {
                        output: crate::OutputSpec::JsonSchema { .. },
                        ..
                    })
                )
            {
                return Err(KernelError::AssistantMessageMismatch);
            }
            final_output_calls += 1;
            if final_output_calls > 1 {
                return Err(KernelError::AssistantMessageMismatch);
            }
        }
    }
    Ok(())
}
pub(super) fn assistant_tool_calls(
    message: &Message,
    skip_internal: bool,
) -> Vec<&crate::ToolCallBlock> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call)
                if !skip_internal || !crate::is_internal_tool_name(call.tool_name()) =>
            {
                Some(call)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn validate_assistant_tool_call_ids(
    allocated: &[ToolCallId],
    message: &Message,
) -> Result<(), KernelError> {
    if assistant_tool_calls(message, false)
        .iter()
        .map(|call| *call.tool_call_id())
        .ne(allocated.iter().copied())
    {
        return Err(KernelError::AssistantMessageMismatch);
    }
    Ok(())
}
pub(super) fn validate_assistant_message_id(
    message_id: MessageId,
    message: &Message,
) -> Result<(), KernelError> {
    if *message.id() != message_id {
        return Err(KernelError::AssistantMessageMismatch);
    }
    Ok(())
}
