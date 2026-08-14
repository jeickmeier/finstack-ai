//! Interaction resume classification for outstanding typed requests.

use finstack_ai_kernel::{KernelState, RunPhase, Timestamp};

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
    let awaiting = state.phase == Some(RunPhase::AwaitingInteraction);
    match (awaiting, state.pending_interaction.as_ref()) {
        (false, None) => {
            if state.last_interaction_terminal.is_some() || !state.resolution_identities.is_empty()
            {
                InteractionResumeAction::UseRecorded
            } else {
                InteractionResumeAction::NoOutstanding
            }
        }
        (true, Some(pending)) => {
            if pending.prior_phase == RunPhase::AwaitingInteraction {
                return InteractionResumeAction::SuspendUncertain;
            }
            match pending.request.expires_at() {
                Some(deadline) if now >= deadline => InteractionResumeAction::ExpireIfDue,
                _ => InteractionResumeAction::WaitResolution,
            }
        }
        (true, None) | (false, Some(_)) => InteractionResumeAction::SuspendUncertain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{
        InteractionKind, InteractionRequest, InteractionTerminal, InteractionTerminalOutcome,
        PendingInteraction, Stage, StageCursor,
    };

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

        let recorded = KernelState {
            last_interaction_terminal: Some(InteractionTerminal {
                interaction_id: id(1),
                kind: InteractionKind::Approval,
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeToolBatch,
                },
                outcome: InteractionTerminalOutcome::Granted,
            }),
            ..KernelState::default()
        };
        assert_eq!(
            interaction_resume_action(&recorded, timestamp(1_000)),
            InteractionResumeAction::UseRecorded
        );

        let waiting = KernelState {
            phase: Some(RunPhase::AwaitingInteraction),
            pending_interaction: Some(pending(Some(timestamp(2_000)))),
            ..KernelState::default()
        };
        assert_eq!(
            interaction_resume_action(&waiting, timestamp(1_000)),
            InteractionResumeAction::WaitResolution
        );
        assert_eq!(
            interaction_resume_action(&waiting, timestamp(2_000)),
            InteractionResumeAction::ExpireIfDue
        );

        let mut mismatched = waiting;
        mismatched.pending_interaction = None;
        assert_eq!(
            interaction_resume_action(&mismatched, timestamp(1_000)),
            InteractionResumeAction::SuspendUncertain
        );
    }
}
