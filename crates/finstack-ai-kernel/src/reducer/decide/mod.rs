//! Pure transition decisions.

mod bodies;
mod cancel;
mod external;
mod limit;
mod model;
mod output;
mod settlement;
mod shared;
mod stage;

use super::decision::{Decision, KernelError};
use super::input::{KernelInput, ReducerStageOutcome, StageSettled};
use crate::state::{KernelState, TransitionEnv};

use cancel::{
    decide_cancel, decide_reconciliation, decide_timer_fired, validate_cancel_authorization,
};
use external::decide_external_command_rejected;
use limit::decide_limit;
use model::{decide_external, decide_model, decide_request_compaction_model};
use output::{decide_capabilities_activated, decide_configure_output, decide_output_validated};
use settlement::{SettlementReview, review_settlement};
use stage::{decide_accept, decide_stage};

pub(super) use shared::{
    draft_for_state, duplicate_decision, expected_stage_cursor, next_sequence,
    outstanding_requested_effects, reject_terminal, required,
};

pub(super) fn decide(
    state: &KernelState,
    env: &TransitionEnv,
    input: KernelInput,
) -> Result<Decision, KernelError> {
    if let KernelInput::RecordExternalCommandRejected(input) = &input {
        return decide_external_command_rejected(state, env, input);
    }
    if let KernelInput::CancelRequested(cancel) = &input
        && state.cancellation.is_none()
        && state.terminal.is_none()
        && let Some(accepted) = state.accepted.as_ref()
    {
        validate_cancel_authorization(accepted, env, cancel)?;
    }
    // The prepared context is the largest payload the kernel canonicalizes and
    // it grows with every turn, so it is canonicalized once here and reused by
    // limit accounting and by the record it produces.
    let context_canonical = match &input {
        KernelInput::StageSettled(StageSettled {
            outcome: ReducerStageOutcome::ContextPrepared { messages },
            ..
        }) => Some(
            crate::records::lifecycle::context_digest_and_len(messages)
                .map_err(|_| KernelError::ContextDigestMismatch)?,
        ),
        _ => None,
    };
    let review = match review_settlement(state, &input)? {
        SettlementReview::Duplicate => return duplicate_decision(state),
        review => review,
    };
    if let Some(decision) = decide_limit(state, env, &input, context_canonical)? {
        return Ok(decision);
    }
    match (input, review) {
        (KernelInput::AcceptRun(input), SettlementReview::Unreviewed) => {
            decide_accept(state, env, &input)
        }
        (KernelInput::StageSettled(input), SettlementReview::Fresh(digest)) => {
            decide_stage(state, env, &input, context_canonical, digest)
        }
        (KernelInput::ModelSettled(input), SettlementReview::Fresh(_)) => {
            decide_model(state, env, &input)
        }
        (KernelInput::ExternalEffectCompleted(input), SettlementReview::Fresh(digest)) => {
            decide_external(state, env, input, digest)
        }
        (KernelInput::ToolBatchSettled(input), SettlementReview::Fresh(digest)) => {
            super::tool::decide_tool_settled(state, env, &input, digest)
        }
        (KernelInput::CancelRequested(input), SettlementReview::Unreviewed) => {
            decide_cancel(state, env, &input)
        }
        (KernelInput::CancellationReconciled(input), SettlementReview::Unreviewed) => {
            decide_reconciliation(state, env, &input)
        }
        (KernelInput::TimerFired(input), SettlementReview::Unreviewed) => {
            decide_timer_fired(state, env, &input)
        }
        (KernelInput::ConfigureOutput(input), SettlementReview::Unreviewed) => {
            decide_configure_output(state, env, input)
        }
        (KernelInput::CapabilitiesActivated(input), SettlementReview::Unreviewed) => {
            decide_capabilities_activated(state, env, input)
        }
        (KernelInput::OutputValidated(input), SettlementReview::Unreviewed) => {
            decide_output_validated(state, env, input)
        }
        (KernelInput::RequestInteraction(input), SettlementReview::Unreviewed) => {
            super::interaction::decide_request(state, env, &input)
        }
        (
            KernelInput::InteractionSettled(input),
            SettlementReview::Fresh(_) | SettlementReview::Unreviewed,
        ) => super::interaction::decide_settled(state, env, &input),
        (KernelInput::RequestCompactionModel(input), SettlementReview::Unreviewed) => {
            decide_request_compaction_model(state, env, &input)
        }
        _ => Err(KernelError::InvariantViolation),
    }
}
