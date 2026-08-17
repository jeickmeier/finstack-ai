use finstack_ai_kernel::{KernelState, Message, RawJson};

use crate::model::ModelRequestDraft;
use crate::run_types::RunHandleError;

use super::{MIDDLEWARE_STAGE_INPUT_INVALID, MIDDLEWARE_STAGE_PAYLOAD_INVALID, stage_error};

// --- codec helpers, on live kernel state rather than a recovered copy -------

/// JCS-canonical bytes for an ordered message array.
pub(super) fn canonical_messages(messages: &[Message]) -> Result<RawJson, RunHandleError> {
    canonical(&messages)
}

/// JCS-canonical bytes for one message.
pub(super) fn canonical_message(message: &Message) -> Result<RawJson, RunHandleError> {
    canonical(message)
}

/// Parse a `Replace` payload back into an ordered message array.
pub(super) fn parse_messages(value: &RawJson) -> Result<Vec<Message>, RunHandleError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}

/// Parse a committed `ModelRequestPrepared` request, or a `BeforeModel`
/// `Replace` payload, back into a [`ModelRequestDraft`].
pub(super) fn parse_draft(value: &RawJson) -> Result<ModelRequestDraft, RunHandleError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}

/// Re-canonicalize a folded draft into the `RawJson` the outcome carries.
pub(super) fn canonical_draft(draft: &ModelRequestDraft) -> Result<RawJson, RunHandleError> {
    let bytes = draft
        .canonical_bytes()
        .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))?;
    RawJson::parse(&bytes).map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}

/// JCS-canonical bytes for the terminal candidate gated by `BeforeFinalize`,
/// read live rather than through a `CommitCoordinator::recover`.
pub(super) fn canonical_terminal_candidate(state: &KernelState) -> Result<RawJson, RunHandleError> {
    let candidate = state
        .terminal_candidate
        .as_ref()
        .ok_or_else(|| stage_error(MIDDLEWARE_STAGE_INPUT_INVALID))?;
    canonical(candidate)
}

pub(super) fn canonical<T: serde::Serialize>(value: &T) -> Result<RawJson, RunHandleError> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))?;
    RawJson::parse(&bytes).map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}
