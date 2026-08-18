//! One settlement review before decide-limit.

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

/// Equal committed redelivery, or a fresh incoming digest for later decide_*.
pub(super) enum SettlementReview {
    Duplicate,
    Fresh { digest: Option<Digest> },
}

/// Classify a settlement input as an equal redelivery, a fresh digest, or a
/// conflict. Non-settlement inputs stay `Fresh` with no digest so they skip
/// the same way both prior layers did.
pub(super) fn review_settlement(
    state: &KernelState,
    input: &KernelInput,
) -> Result<SettlementReview, KernelError> {
    match input {
        KernelInput::StageSettled(input) => review_index(
            state.stage_settlements.get(&input.cursor).copied(),
            stage_digest(input)?,
        ),
        KernelInput::ModelSettled(input) => review_model(state, input),
        KernelInput::ToolBatchSettled(input) => review_tool(state, input),
        KernelInput::ExternalEffectCompleted(input) => review_external(state, input),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(resolution)) => review_index(
            state
                .resolution_identities
                .get(resolution.resolution_id())
                .map(|existing| existing.settlement_digest),
            resolution_digest(resolution)?,
        ),
        _ => Ok(SettlementReview::Fresh { digest: None }),
    }
}

fn review_model(
    state: &KernelState,
    input: &ModelSettled,
) -> Result<SettlementReview, KernelError> {
    let digest = direct_digest(input)?;
    let reviewed = review_completion_or_map(
        state,
        input.outcome.completion_id(),
        input.outcome.effect_id(),
        digest,
        state
            .model_settlements
            .get(&input.outcome.effect_id())
            .map(|existing| existing.digest),
    )?;
    if matches!(reviewed, SettlementReview::Duplicate) {
        return Ok(reviewed);
    }
    review_model_deferred(state, input, digest)
}

fn review_tool(
    state: &KernelState,
    input: &ToolBatchSettled,
) -> Result<SettlementReview, KernelError> {
    let digest = direct_tool_digest(input.tool_batch_id, &input.outcome)?;
    let effect_id = input.outcome.effect_id();
    let reviewed = review_completion_or_map(
        state,
        input.outcome.completion_id(),
        effect_id,
        digest,
        state
            .tool_settlements
            .get(&effect_id)
            .map(|existing| existing.digest),
    )?;
    if matches!(reviewed, SettlementReview::Duplicate) {
        return Ok(reviewed);
    }
    review_tool_deferred(state, input, digest)
}

fn review_external(
    state: &KernelState,
    input: &ExternalEffectCompletedInput,
) -> Result<SettlementReview, KernelError> {
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
            return Ok(SettlementReview::Fresh { digest: None });
        };
        let digest = external_tool_digest(tool_batch_id, input)?;
        return review_completion_or_map(
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
    review_completion_or_map(
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

fn review_completion_or_map(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    digest: Digest,
    existing_settlement: Option<Digest>,
) -> Result<SettlementReview, KernelError> {
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
    {
        return if existing.effect_id == effect_id && existing.settlement_digest == digest {
            Ok(SettlementReview::Duplicate)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    review_index(existing_settlement, digest)
}

fn review_index(existing: Option<Digest>, digest: Digest) -> Result<SettlementReview, KernelError> {
    match existing {
        Some(existing) if existing == digest => Ok(SettlementReview::Duplicate),
        Some(_) => Err(KernelError::ConflictingSettlement),
        None => Ok(fresh(digest)),
    }
}

fn review_model_deferred(
    state: &KernelState,
    input: &ModelSettled,
    digest: Digest,
) -> Result<SettlementReview, KernelError> {
    let ModelSettlement::Deferred(deferred) = &input.outcome else {
        return Ok(fresh(digest));
    };
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(fresh(digest));
    };
    if pending.requested.effect_id() != deferred.effect_id {
        return Ok(fresh(digest));
    }
    let Some(existing) = pending.deferred.as_ref() else {
        return Ok(fresh(digest));
    };
    let existing_digest = direct_digest(&ModelSettled {
        turn_id: pending.turn_id,
        model_request_id: pending.model_request_id,
        outcome: ModelSettlement::Deferred(existing.clone()),
    })?;
    review_index(Some(existing_digest), digest)
}

fn review_tool_deferred(
    state: &KernelState,
    input: &ToolBatchSettled,
    digest: Digest,
) -> Result<SettlementReview, KernelError> {
    let ToolSettlement::Deferred(_) = &input.outcome else {
        return Ok(fresh(digest));
    };
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(fresh(digest));
    };
    if batch.opened.tool_batch_id != input.tool_batch_id {
        return Ok(fresh(digest));
    }
    let effect_id = input.outcome.effect_id();
    let Some(active) = batch.call(effect_id) else {
        return Ok(fresh(digest));
    };
    let crate::records::tools::ActiveToolCallStatus::Requested {
        deferred: Some(existing),
        ..
    } = &active.status
    else {
        return Ok(fresh(digest));
    };
    let existing_digest = direct_tool_digest(
        input.tool_batch_id,
        &ToolSettlement::Deferred(existing.clone()),
    )?;
    review_index(Some(existing_digest), digest)
}

fn fresh(digest: Digest) -> SettlementReview {
    SettlementReview::Fresh {
        digest: Some(digest),
    }
}
