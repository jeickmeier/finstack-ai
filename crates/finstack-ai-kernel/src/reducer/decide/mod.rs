//! Pure transition decisions.

mod bodies;
mod cancel;
mod external;
mod limit;
mod model;
mod output;
mod precheck;
mod shared;
mod stage;

use super::decision::{Decision, KernelError};
use super::fingerprint::{
    direct_digest, direct_tool_digest, external_digest, external_tool_digest, stage_digest,
};
use super::input::{
    ExternalEffectCompletedInput, InteractionSettled, KernelInput, ModelSettled, ModelSettlement,
    ReducerStageOutcome, StageSettled, ToolBatchSettled, ToolSettlement,
};
use super::interaction::resolution_digest;
use super::tool::is_known_tool_effect;
use crate::state::{KernelState, TransitionEnv};

use cancel::{
    decide_cancel, decide_reconciliation, decide_timer_fired, validate_cancel_authorization,
};
use external::decide_external_command_rejected;
use limit::decide_limit;
use model::{decide_external, decide_model, decide_request_compaction_model};
use output::{decide_capabilities_activated, decide_configure_output, decide_output_validated};
use precheck::precheck;
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
    if let Some(decision) = equal_committed_redelivery(state, &input)? {
        return Ok(decision);
    }
    precheck(state, &input)?;
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
        KernelInput::RequestCompactionModel(input) => {
            decide_request_compaction_model(state, env, &input)
        }
    }
}

fn equal_committed_redelivery(
    state: &KernelState,
    input: &KernelInput,
) -> Result<Option<Decision>, KernelError> {
    match input {
        KernelInput::StageSettled(input) => {
            let digest = stage_digest(input)?;
            equal_index_duplicate(
                state,
                state.stage_settlements.get(&input.cursor).copied(),
                digest,
            )
        }
        KernelInput::ModelSettled(input) => {
            let digest = direct_digest(input)?;
            if let Some(decision) = equal_completion_or_model_duplicate(
                state,
                model_settlement_completion_id(&input.outcome),
                model_settlement_effect_id(&input.outcome),
                digest,
            )? {
                return Ok(Some(decision));
            }
            equal_model_deferred_duplicate(state, input, digest)
        }
        KernelInput::ToolBatchSettled(input) => {
            let digest = direct_tool_digest(input.tool_batch_id, &input.outcome)?;
            if let Some(decision) = equal_completion_or_tool_duplicate(
                state,
                tool_settlement_completion_id(&input.outcome),
                tool_settlement_effect_id(&input.outcome),
                digest,
            )? {
                return Ok(Some(decision));
            }
            equal_tool_deferred_duplicate(state, input, digest)
        }
        KernelInput::ExternalEffectCompleted(input) => {
            equal_external_completion_duplicate(state, input)
        }
        KernelInput::InteractionSettled(InteractionSettled::Resolved(resolution)) => {
            let digest = resolution_digest(resolution)?;
            equal_index_duplicate(
                state,
                state
                    .resolution_identities
                    .get(resolution.resolution_id())
                    .map(|existing| existing.settlement_digest),
                digest,
            )
        }
        _ => Ok(None),
    }
}

fn equal_external_completion_duplicate(
    state: &KernelState,
    input: &ExternalEffectCompletedInput,
) -> Result<Option<Decision>, KernelError> {
    let effect_id = input.completion.effect_id;
    let completion_id = input.completion.completion_id.as_ref();
    if let Some(existing) = state.completion_identities.get(completion_id)
        && existing.effect_id != effect_id
    {
        return Err(KernelError::ConflictingCompletionId);
    }
    if is_known_tool_effect(state, effect_id) {
        let Some(tool_batch_id) = state
            .active_tool_batch
            .as_ref()
            .map(|batch| batch.opened.tool_batch_id)
            .or_else(|| {
                state
                    .tool_calls
                    .values()
                    .find(|identity| identity.effect_id == Some(effect_id))
                    .and_then(|identity| identity.tool_batch_id)
            })
        else {
            return Ok(None);
        };
        let digest = external_tool_digest(tool_batch_id, input)?;
        return equal_completion_or_tool_duplicate(state, Some(completion_id), effect_id, digest);
    }
    let digest = external_digest(input)?;
    equal_completion_or_model_duplicate(state, Some(completion_id), effect_id, digest)
}

fn equal_completion_or_model_duplicate(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    digest: crate::primitives::Digest,
) -> Result<Option<Decision>, KernelError> {
    if let Some(decision) = equal_completion_duplicate(state, completion_id, effect_id, digest)? {
        return Ok(Some(decision));
    }
    equal_index_duplicate(
        state,
        state
            .model_settlements
            .get(&effect_id)
            .map(|existing| existing.digest),
        digest,
    )
}

fn equal_completion_or_tool_duplicate(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    digest: crate::primitives::Digest,
) -> Result<Option<Decision>, KernelError> {
    if let Some(decision) = equal_completion_duplicate(state, completion_id, effect_id, digest)? {
        return Ok(Some(decision));
    }
    equal_index_duplicate(
        state,
        state
            .tool_settlements
            .get(&effect_id)
            .map(|existing| existing.digest),
        digest,
    )
}

fn equal_completion_duplicate(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    digest: crate::primitives::Digest,
) -> Result<Option<Decision>, KernelError> {
    let Some(completion_id) = completion_id else {
        return Ok(None);
    };
    let Some(existing) = state.completion_identities.get(completion_id) else {
        return Ok(None);
    };
    if existing.effect_id == effect_id && existing.settlement_digest == digest {
        duplicate_decision(state).map(Some)
    } else {
        Ok(None)
    }
}

fn equal_index_duplicate(
    state: &KernelState,
    existing: Option<crate::primitives::Digest>,
    digest: crate::primitives::Digest,
) -> Result<Option<Decision>, KernelError> {
    match existing {
        Some(existing) if existing == digest => duplicate_decision(state).map(Some),
        _ => Ok(None),
    }
}

fn equal_model_deferred_duplicate(
    state: &KernelState,
    input: &ModelSettled,
    digest: crate::primitives::Digest,
) -> Result<Option<Decision>, KernelError> {
    let ModelSettlement::Deferred(deferred) = &input.outcome else {
        return Ok(None);
    };
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(None);
    };
    if pending.requested.effect_id() != deferred.effect_id {
        return Ok(None);
    }
    let Some(existing) = pending.deferred.as_ref() else {
        return Ok(None);
    };
    let existing_digest = direct_digest(&ModelSettled {
        turn_id: pending.turn_id,
        model_request_id: pending.model_request_id,
        outcome: ModelSettlement::Deferred(existing.clone()),
    })?;
    if existing_digest == digest {
        duplicate_decision(state).map(Some)
    } else {
        Ok(None)
    }
}

fn equal_tool_deferred_duplicate(
    state: &KernelState,
    input: &ToolBatchSettled,
    digest: crate::primitives::Digest,
) -> Result<Option<Decision>, KernelError> {
    let ToolSettlement::Deferred(_) = &input.outcome else {
        return Ok(None);
    };
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(None);
    };
    if batch.opened.tool_batch_id != input.tool_batch_id {
        return Ok(None);
    }
    let effect_id = tool_settlement_effect_id(&input.outcome);
    let Some(active) = batch.call(effect_id) else {
        return Ok(None);
    };
    let crate::records::tools::ActiveToolCallStatus::Requested {
        deferred: Some(existing),
        ..
    } = &active.status
    else {
        return Ok(None);
    };
    let existing_digest = direct_tool_digest(
        input.tool_batch_id,
        &ToolSettlement::Deferred(existing.clone()),
    )?;
    if existing_digest == digest {
        duplicate_decision(state).map(Some)
    } else {
        Ok(None)
    }
}

fn model_settlement_effect_id(outcome: &ModelSettlement) -> crate::EffectId {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.effect_id(),
        ModelSettlement::Deferred(deferred) => deferred.effect_id,
        ModelSettlement::Failed(failed) => failed.effect_id(),
    }
}

fn model_settlement_completion_id(outcome: &ModelSettlement) -> Option<&str> {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.completion_id(),
        ModelSettlement::Deferred(_) => None,
        ModelSettlement::Failed(failed) => failed.completion_id(),
    }
}

fn tool_settlement_effect_id(outcome: &ToolSettlement) -> crate::EffectId {
    match outcome {
        ToolSettlement::Completed(value) => value.effect_id(),
        ToolSettlement::Deferred(value) => value.effect_id,
        ToolSettlement::Failed(value) => value.effect_id(),
    }
}

fn tool_settlement_completion_id(outcome: &ToolSettlement) -> Option<&str> {
    match outcome {
        ToolSettlement::Completed(value) => value.completion_id(),
        ToolSettlement::Deferred(_) => None,
        ToolSettlement::Failed(value) => value.completion_id(),
    }
}
