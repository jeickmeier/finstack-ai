//! Interaction resume classification and response-schema validation.

use finstack_ai_kernel::{KernelState, RunPhase, Timestamp};

use std::collections::BTreeMap;

use finstack_ai_kernel::{RawJson, ValidationOutcome};
use thiserror::Error;

use crate::ports::tool::{JsonSchemaToolValidatorCompiler, ToolValidatorCompiler};

/// Fail-closed interaction-response validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum InteractionResponseError {
    /// The parked request's response schema cannot be compiled.
    #[error("interaction response schema is invalid")]
    InvalidSchema,
    /// The resolution payload does not satisfy the parked schema.
    #[error("interaction response does not satisfy the parked schema")]
    InvalidResponse,
}

/// Validate one resolution payload against the parked request schema.
///
/// Compiles `schema` with [`JsonSchemaToolValidatorCompiler`] and does not
/// commit any kernel input. Callers must invoke this before
/// [`finstack_ai_kernel::KernelInput::InteractionSettled`].
///
/// # Errors
///
/// Returns [`InteractionResponseError::InvalidSchema`] when the parked schema
/// cannot be compiled, and [`InteractionResponseError::InvalidResponse`] when
/// `response` fails the compiled schema.
pub(crate) fn validate_interaction_response(
    schema: &RawJson,
    response: &RawJson,
) -> Result<(), InteractionResponseError> {
    let validator = JsonSchemaToolValidatorCompiler
        .compile(schema, &BTreeMap::new())
        .map_err(|_| InteractionResponseError::InvalidSchema)?;
    match validator.validate(response) {
        ValidationOutcome::Valid => Ok(()),
        ValidationOutcome::Invalid { .. } => Err(InteractionResponseError::InvalidResponse),
    }
}

/// First-pass classification of an outstanding typed interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionResumeAction {
    /// No pending interaction and the run is not waiting.
    NoOutstanding,
    /// A terminal resolution is already recorded.
    UseRecorded,
    /// Wait for an authenticated resolution.
    WaitResolution,
    /// The recorded deadline is due; expire after recover.
    ExpireIfDue,
    /// Phase and pending projection disagree; do not fabricate a resolution.
    SuspendUncertain,
}

/// Classify the outstanding interaction without fabricating a resolution.
#[must_use]
pub fn interaction_resume_action(state: &KernelState, now: Timestamp) -> InteractionResumeAction {
    let awaiting = state.phase() == Some(RunPhase::AwaitingInteraction);
    match (awaiting, state.pending_interaction()) {
        (false, None) => {
            if state.last_interaction_terminal().is_some()
                || !state.resolution_identities().is_empty()
            {
                InteractionResumeAction::UseRecorded
            } else {
                InteractionResumeAction::NoOutstanding
            }
        }
        (true, Some(pending)) => {
            if state.cancellation().is_some() {
                return InteractionResumeAction::WaitResolution;
            }
            if pending.prior_phase == RunPhase::AwaitingInteraction {
                return InteractionResumeAction::SuspendUncertain;
            }
            match pending.request.expires_at() {
                Some(deadline) if now >= deadline => InteractionResumeAction::ExpireIfDue,
                _ => InteractionResumeAction::WaitResolution,
            }
        }
        (false, Some(_))
            if matches!(
                state.phase(),
                Some(RunPhase::Cancelling | RunPhase::Suspended)
            ) && state.cancellation().is_some() =>
        {
            InteractionResumeAction::WaitResolution
        }
        (true, None) | (false, Some(_)) => InteractionResumeAction::SuspendUncertain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{
        InteractionKind, InteractionRequest, InteractionTerminal, InteractionTerminalOutcome,
        PendingInteraction, RawJson, Stage, StageCursor,
    };

    #[test]
    fn free_text_schema_rejects_numeric_answer() {
        let schema = RawJson::parse(
            r#"{"additionalProperties":false,"properties":{"answer":{"type":"string"}},"required":["answer"],"type":"object"}"#,
        )
        .expect("schema");
        let response = RawJson::parse(r#"{"answer":1}"#).expect("response");
        assert_eq!(
            validate_interaction_response(&schema, &response),
            Err(InteractionResponseError::InvalidResponse)
        );
    }

    #[test]
    fn typed_object_schema_rejects_string_answer() {
        let schema = RawJson::parse(
            r#"{"additionalProperties":false,"properties":{"answer":{"properties":{"confirmed":{"type":"boolean"}},"required":["confirmed"],"type":"object"}},"required":["answer"],"type":"object"}"#,
        )
        .expect("schema");
        let response = RawJson::parse(r#"{"answer":"nope"}"#).expect("response");
        assert_eq!(
            validate_interaction_response(&schema, &response),
            Err(InteractionResponseError::InvalidResponse)
        );
    }

    #[test]
    fn matching_string_answer_is_accepted() {
        let schema = RawJson::parse(
            r#"{"additionalProperties":false,"properties":{"answer":{"type":"string"}},"required":["answer"],"type":"object"}"#,
        )
        .expect("schema");
        let response = RawJson::parse(r#"{"answer":"250k USD"}"#).expect("response");
        assert_eq!(validate_interaction_response(&schema, &response), Ok(()));
    }

    fn timestamp(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn id(ordinal: u64) -> finstack_ai_kernel::Id<finstack_ai_kernel::InteractionTag> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        finstack_ai_kernel::Id::from_bytes(bytes)
    }

    fn request(expires_at: Option<Timestamp>) -> InteractionRequest {
        InteractionRequest::try_new(
            1,
            id(1),
            finstack_ai_kernel::Id::from_bytes({
                let mut bytes = [0_u8; 16];
                bytes[6] = 0x70;
                bytes[8..].copy_from_slice(&2_u64.to_be_bytes());
                bytes[8] = (bytes[8] & 0x3f) | 0x80;
                bytes
            }),
            InteractionKind::Approval,
            Vec::new(),
            finstack_ai_kernel::RawJson::parse(r#"{"type":"object"}"#).expect("schema"),
            finstack_ai_kernel::ComponentRef::new(
                finstack_ai_kernel::ComponentId::parse("finstack.policy.approval")
                    .expect("component"),
                Some(finstack_ai_kernel::Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
            ),
            finstack_ai_kernel::Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            None,
            expires_at,
            false,
            finstack_ai_kernel::Metadata::empty(),
        )
        .expect("request")
    }

    fn pending(expires_at: Option<Timestamp>) -> PendingInteraction {
        PendingInteraction {
            request: request(expires_at),
            prior_phase: RunPhase::BeforeToolBatch,
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeToolBatch,
            },
        }
    }

    #[test]
    fn classifies_journal_only_interaction_states() {
        assert_eq!(
            interaction_resume_action(&KernelState::default(), timestamp(1_000)),
            InteractionResumeAction::NoOutstanding
        );

        let mut recorded = KernelState::default();
        recorded.set_last_interaction_terminal(Some(InteractionTerminal {
            interaction_id: id(1),
            kind: InteractionKind::Approval,
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeToolBatch,
            },
            outcome: InteractionTerminalOutcome::Granted,
        }));
        assert_eq!(
            interaction_resume_action(&recorded, timestamp(1_000)),
            InteractionResumeAction::UseRecorded
        );

        let mut waiting = KernelState::default();
        waiting.set_phase(Some(RunPhase::AwaitingInteraction));
        waiting.set_pending_interaction(Some(pending(Some(timestamp(2_000)))));
        assert_eq!(
            interaction_resume_action(&waiting, timestamp(1_000)),
            InteractionResumeAction::WaitResolution
        );
        assert_eq!(
            interaction_resume_action(&waiting, timestamp(2_000)),
            InteractionResumeAction::ExpireIfDue
        );

        let mut mismatched = waiting;
        mismatched.set_pending_interaction(None);
        assert_eq!(
            interaction_resume_action(&mismatched, timestamp(1_000)),
            InteractionResumeAction::SuspendUncertain
        );

        let mut cancelling = KernelState::default();
        cancelling.set_phase(Some(RunPhase::Cancelling));
        cancelling.set_pending_interaction(Some(pending(Some(timestamp(2_000)))));
        cancelling.set_cancellation(Some(finstack_ai_kernel::CancellationState {
            request: finstack_ai_kernel::CancellationRequest::try_new(
                finstack_ai_kernel::Id::from_bytes({
                    let mut bytes = [0_u8; 16];
                    bytes[6] = 0x70;
                    bytes[8..].copy_from_slice(&3_u64.to_be_bytes());
                    bytes[8] = (bytes[8] & 0x3f) | 0x80;
                    bytes
                }),
                finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
                Some("shutdown"),
            )
            .expect("request"),
            prior_phase: RunPhase::AwaitingInteraction,
            completed_effects: std::sync::Arc::from([]),
            cancelled_effects: std::sync::Arc::from([]),
            uncertain_effects: std::sync::Arc::from([]),
            outstanding_effects: std::sync::Arc::from([]),
        }));
        assert_eq!(
            interaction_resume_action(&cancelling, timestamp(3_000)),
            InteractionResumeAction::WaitResolution
        );
    }
}
