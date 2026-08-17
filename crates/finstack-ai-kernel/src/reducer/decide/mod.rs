//! Pure PR-009 transition decisions.

mod bodies;
mod cancel;
mod external;
mod limit;
mod model;
mod output;
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
use model::{decide_external, decide_model};
use output::{decide_capabilities_activated, decide_configure_output, decide_output_validated};
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
            crate::entries::context_digest_and_len(messages)
                .map_err(|_| KernelError::ContextDigestMismatch)?,
        ),
        _ => None,
    };
    if let Some(decision) = decide_limit(state, env, &input, context_canonical)? {
        return Ok(decision);
    }
    match input {
        KernelInput::AcceptRun(input) => decide_accept(state, env, &input),
        KernelInput::StageSettled(input) => decide_stage(state, env, &input, context_canonical),
        KernelInput::ModelSettled(input) => decide_model(state, env, &input),
        KernelInput::ExternalEffectCompleted(input) => decide_external(state, env, input),
        KernelInput::ToolBatchSettled(input) => {
            super::tool::decide_tool_settled(state, env, &input)
        }
        KernelInput::CancelRequested(input) => decide_cancel(state, env, &input),
        KernelInput::CancellationReconciled(input) => decide_reconciliation(state, env, &input),
        KernelInput::TimerFired(input) => decide_timer_fired(state, env, &input),
        KernelInput::ConfigureOutput(input) => decide_configure_output(state, env, input),
        KernelInput::CapabilitiesActivated(input) => {
            decide_capabilities_activated(state, env, input)
        }
        KernelInput::OutputValidated(input) => decide_output_validated(state, env, input),
        KernelInput::RecordExternalCommandRejected(_) => {
            unreachable!("external rejection returns before limit processing")
        }
        KernelInput::RequestInteraction(input) => {
            super::interaction::decide_request(state, env, &input)
        }
        KernelInput::InteractionSettled(input) => {
            super::interaction::decide_settled(state, env, &input)
        }
    }
}
