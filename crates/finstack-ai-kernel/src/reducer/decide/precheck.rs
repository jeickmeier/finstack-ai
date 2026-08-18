//! Narrow rejectable-settlement hoist before decide-limit.

use super::super::decision::KernelError;
use super::super::fingerprint::{
    direct_digest, direct_tool_digest, external_digest, external_tool_digest, stage_digest,
};
use super::super::input::{
    ExternalEffectCompletedInput, InteractionSettled, KernelInput, ModelSettled, ModelSettlement,
    ToolBatchSettled, ToolSettlement,
};
use super::super::interaction::resolution_digest;
use super::super::tool::is_known_tool_effect;
use crate::primitives::Digest;
use crate::state::KernelState;

/// Reject conflicting redelivery before limit accounting can terminally fail a
/// healthy run. Does not inspect assistant-message presence; completion-id
/// conflicts stay first.
pub(super) fn precheck(state: &KernelState, input: &KernelInput) -> Result<(), KernelError> {
    match input {
        KernelInput::StageSettled(input) => {
            let digest = stage_digest(input)?;
            reject_digest_conflict(state.stage_settlements.get(&input.cursor).copied(), digest)
        }
        KernelInput::ModelSettled(input) => {
            let digest = direct_digest(input)?;
            reject_completion_or_map_conflict(
                state,
                input.outcome.completion_id(),
                input.outcome.effect_id(),
                digest,
                state
                    .model_settlements
                    .get(&input.outcome.effect_id())
                    .map(|existing| existing.digest),
            )?;
            reject_model_deferred_conflict(state, input, digest)
        }
        KernelInput::ToolBatchSettled(input) => {
            let digest = direct_tool_digest(input.tool_batch_id, &input.outcome)?;
            let effect_id = input.outcome.effect_id();
            reject_completion_or_map_conflict(
                state,
                input.outcome.completion_id(),
                effect_id,
                digest,
                state
                    .tool_settlements
                    .get(&effect_id)
                    .map(|existing| existing.digest),
            )?;
            reject_tool_deferred_conflict(state, input, digest)
        }
        KernelInput::ExternalEffectCompleted(input) => reject_external_conflicts(state, input),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(resolution)) => {
            let digest = resolution_digest(resolution)?;
            reject_digest_conflict(
                state
                    .resolution_identities
                    .get(resolution.resolution_id())
                    .map(|existing| existing.settlement_digest),
                digest,
            )
        }
        _ => Ok(()),
    }
}

fn reject_external_conflicts(
    state: &KernelState,
    input: &ExternalEffectCompletedInput,
) -> Result<(), KernelError> {
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
            return Ok(());
        };
        let digest = external_tool_digest(tool_batch_id, input)?;
        return reject_completion_or_map_conflict(
            state,
            Some(completion_id),
            effect_id,
            digest,
            state
                .tool_settlements
                .get(&effect_id)
                .map(|existing| existing.digest),
        );
    }
    let digest = external_digest(input)?;
    reject_completion_or_map_conflict(
        state,
        Some(completion_id),
        effect_id,
        digest,
        state
            .model_settlements
            .get(&effect_id)
            .map(|existing| existing.digest),
    )
}

fn reject_completion_or_map_conflict(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    digest: Digest,
    existing_settlement: Option<Digest>,
) -> Result<(), KernelError> {
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
        && (existing.effect_id != effect_id || existing.settlement_digest != digest)
    {
        return Err(KernelError::ConflictingCompletionId);
    }
    reject_digest_conflict(existing_settlement, digest)
}

fn reject_digest_conflict(existing: Option<Digest>, digest: Digest) -> Result<(), KernelError> {
    match existing {
        Some(existing) if existing != digest => Err(KernelError::ConflictingSettlement),
        _ => Ok(()),
    }
}

fn reject_model_deferred_conflict(
    state: &KernelState,
    input: &ModelSettled,
    digest: Digest,
) -> Result<(), KernelError> {
    let ModelSettlement::Deferred(deferred) = &input.outcome else {
        return Ok(());
    };
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(());
    };
    if pending.requested.effect_id() != deferred.effect_id {
        return Ok(());
    }
    let Some(existing) = pending.deferred.as_ref() else {
        return Ok(());
    };
    let existing_digest = direct_digest(&ModelSettled {
        turn_id: pending.turn_id,
        model_request_id: pending.model_request_id,
        outcome: ModelSettlement::Deferred(existing.clone()),
    })?;
    reject_digest_conflict(Some(existing_digest), digest)
}

fn reject_tool_deferred_conflict(
    state: &KernelState,
    input: &ToolBatchSettled,
    digest: Digest,
) -> Result<(), KernelError> {
    let ToolSettlement::Deferred(_) = &input.outcome else {
        return Ok(());
    };
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(());
    };
    if batch.opened.tool_batch_id != input.tool_batch_id {
        return Ok(());
    }
    let effect_id = input.outcome.effect_id();
    let Some(active) = batch.call(effect_id) else {
        return Ok(());
    };
    let crate::records::tools::ActiveToolCallStatus::Requested {
        deferred: Some(existing),
        ..
    } = &active.status
    else {
        return Ok(());
    };
    let existing_digest = direct_tool_digest(
        input.tool_batch_id,
        &ToolSettlement::Deferred(existing.clone()),
    )?;
    reject_digest_conflict(Some(existing_digest), digest)
}
