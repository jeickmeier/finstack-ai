//! Semantic validations shared by reducer decision paths.

use crate::Digest;
use crate::content::ContentBlock;
use crate::effects::EffectCompleted;
use crate::error::ErrorDescriptor;
use crate::ids::{EffectId, MessageId};
use crate::message::{Message, MessageRole};
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
    env: &TransitionEnv,
    message: &Message,
    completion: &EffectCompleted,
) -> Result<(), KernelError> {
    if message.created_at() != env.now
        || message.role() != MessageRole::Assistant
        || message
            .content()
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolCall(_)))
        || message.provider_ids() != completion.provider_ids()
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

pub(super) fn validate_completion_identity(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: EffectId,
    settlement_digest: Digest,
) -> Result<(), KernelError> {
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
        && (existing.effect_id != effect_id || existing.settlement_digest != settlement_digest)
    {
        return Err(KernelError::ConflictingCompletionId);
    }
    Ok(())
}
