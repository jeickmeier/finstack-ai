use finstack_ai_kernel::{
    Digest, EntryId, KernelState, Message, MessageRole, ReducerStageOutcome, Sensitivity, Stage,
};

use crate::context_driver::structural_protected;
use crate::middleware::{BeforeModelInput, CompactionSourceEntry, StageInput};
use crate::model::LockedModelContextProfile;
use crate::run_types::RunHandleError;

use super::codec::{
    canonical_message, canonical_messages, canonical_terminal_candidate, parse_draft,
};
use super::{MIDDLEWARE_STAGE_INPUT_INVALID, stage_error};

/// Build the [`StageInput`] one stage's chain observes.
///
/// Every payload is JCS-canonical JSON over live `coordinator.state()`, not a
/// recovered copy:
///
/// - `BeforeRun` — the run's message array (empty at that cursor).
/// - `PrepareContext` — the base outcome's own prepared message array, i.e.
///   exactly what would land if no component contributed.
/// - `AfterModel` — the single most recent message, which at that cursor is the
///   assistant message the model just produced.
/// - `AfterToolBatch` — the trailing run of `MessageRole::Tool` messages, i.e.
///   the results the batch just appended.
/// - `BeforeFinalize` — the terminal candidate, live and O(1) here.
///
/// `BeforeModel` is the exception: it is the typed
/// `StageInput::BeforeModel(Box<BeforeModelInput>)`, built by
/// [`before_model_input`] from the base outcome plus the run's locked profile.
pub(super) fn stage_input(
    state: &KernelState,
    cursor_stage: Stage,
    outcome: &ReducerStageOutcome,
    profile: &LockedModelContextProfile,
    projection: Option<&std::collections::BTreeMap<EntryId, (bool, Sensitivity)>>,
) -> Result<StageInput, RunHandleError> {
    match cursor_stage {
        Stage::BeforeRun => Ok(StageInput::BeforeRun {
            value: canonical_messages(state.messages().as_slice())?,
        }),
        Stage::PrepareContext => {
            let value = match outcome {
                ReducerStageOutcome::ContextPrepared { messages } => canonical_messages(messages)?,
                _ => canonical_messages(state.messages().as_slice())?,
            };
            Ok(StageInput::PrepareContext { value })
        }
        Stage::AfterModel => {
            let message = state
                .messages()
                .last()
                .ok_or_else(|| stage_error(MIDDLEWARE_STAGE_INPUT_INVALID))?;
            Ok(StageInput::AfterModel {
                value: canonical_message(message)?,
            })
        }
        Stage::AfterToolBatch => Ok(StageInput::AfterToolBatch {
            value: canonical_messages(trailing_role_run(
                state.messages().as_slice(),
                MessageRole::Tool,
            ))?,
        }),
        Stage::BeforeFinalize => {
            let result_message = match state.terminal_candidate() {
                Some(finstack_ai_kernel::TerminalCandidate::Completed { message_id, .. }) => state
                    .messages()
                    .iter()
                    .find(|message| message.id() == message_id)
                    .map(canonical_message)
                    .transpose()?,
                _ => None,
            };
            Ok(StageInput::BeforeFinalize {
                candidate: canonical_terminal_candidate(state)?,
                result_message,
            })
        }
        Stage::BeforeModel => Ok(StageInput::BeforeModel(Box::new(before_model_input(
            outcome, profile, projection,
        )?))),
        Stage::BeforeToolBatch => Err(stage_error(MIDDLEWARE_STAGE_INPUT_INVALID)),
    }
}

/// Assemble the typed `BeforeModel` stage input.
///
/// # `request`
///
/// Round-tripped from the base outcome's `request: RawJson`, which the facade
/// produced with `ModelRequestDraft::canonical_bytes` (`agent.rs:813-815`), so
/// the trip is exact. `parse_committed_model_request` already relies on the
/// same round trip.
///
/// # `source_entries`
///
/// One entry per message of **the draft's own array**, not of
/// `state.current_turn.context.messages`. In the facade path the two are the
/// same array — `model_draft` is handed `turn.context.messages`
/// (`agent.rs:797-812`) — but sourcing them from the draft is the choice that
/// stays correct if they ever diverge: a `CompactContext` result is validated
/// against `source_entries` (`middleware.rs:1069-1083`) and then *replaces*
/// `draft.messages`, so entries drawn from anywhere else would let a compactor
/// inject a message the draft never had, or lose one it did.
///
/// Each field is derived, never invented:
///
/// - `entry_id` — `EntryId::from_bytes(message.id().to_bytes())`, the kernel's
///   own message-to-entry mapping (`conversation.rs:128` and `:584`).
/// - `sensitivity` — [`Sensitivity::Internal`], the classification the kernel
///   itself requires of every message-bearing event (`events.rs:864-867`).
/// - `provenance_digest` — over the message's canonical bytes. In a
///   journal-derived history the message *is* its own provenance. It is never
///   compared field-wise; it only feeds `compaction_source_digest`, so any
///   deterministic derivation is self-consistent.
/// - `protected` — from the context-port projection when providers ran, else
///   the structural rule: system/developer messages and the trailing current
///   user are protected. A compactor still cannot set this bit.
///
/// # `hard_input_tokens`
///
/// `context_window_tokens - (reserved_output_tokens + provider_overhead_tokens)`,
/// which is exactly `validate_model_request`'s `available` (`model.rs:1289-1302`)
/// and matches the field's own definition, "maximum permitted model-input tokens
/// after output/overhead reservation".
///
/// # Errors
///
/// Returns `middleware_stage_input_invalid` when the base outcome is not a
/// `ModelRequestPrepared` (nothing else can carry a draft at this cursor) or the
/// locked profile's margins leave no input budget, and
/// `middleware_stage_payload_invalid` when the committed request is not a
/// decodable draft.
pub(super) fn before_model_input(
    outcome: &ReducerStageOutcome,
    profile: &LockedModelContextProfile,
    projection: Option<&std::collections::BTreeMap<EntryId, (bool, Sensitivity)>>,
) -> Result<BeforeModelInput, RunHandleError> {
    let ReducerStageOutcome::ModelRequestPrepared { request, .. } = outcome else {
        return Err(stage_error(MIDDLEWARE_STAGE_INPUT_INVALID));
    };
    let draft = parse_draft(request)?;
    let last = draft.messages.len().saturating_sub(1);
    let mut source_entries = Vec::with_capacity(draft.messages.len());
    for (index, message) in draft.messages.iter().enumerate() {
        let entry_id = EntryId::from_bytes(message.id().to_bytes());
        let (protected, sensitivity) = projection
            .and_then(|map| map.get(&entry_id).copied())
            .unwrap_or_else(|| {
                (
                    structural_protected(message, index == last),
                    Sensitivity::Internal,
                )
            });
        source_entries.push(CompactionSourceEntry {
            entry_id,
            message: message.clone(),
            sensitivity,
            provenance_digest: Digest::raw_json(canonical_message(message)?.as_bytes()),
            protected,
        });
    }
    Ok(BeforeModelInput {
        request: draft,
        source_entries: source_entries.into(),
        model_context_profile_digest: profile.digest,
        hard_input_tokens: hard_input_tokens(profile)?,
        checkpoint: None,
    })
}

/// The locked profile's input-token budget after output and overhead margins.
fn hard_input_tokens(profile: &LockedModelContextProfile) -> Result<u64, RunHandleError> {
    profile
        .profile
        .reserved_output_tokens
        .checked_add(profile.profile.provider_overhead_tokens)
        .and_then(|margins| profile.profile.context_window_tokens.checked_sub(margins))
        .ok_or_else(|| stage_error(MIDDLEWARE_STAGE_INPUT_INVALID))
}

/// The maximal trailing slice of `messages` whose role is `role`.
pub(super) fn trailing_role_run(messages: &[Message], role: MessageRole) -> &[Message] {
    let start = messages
        .iter()
        .rposition(|message| message.role() != role)
        .map_or(0, |index| index + 1);
    &messages[start..]
}
