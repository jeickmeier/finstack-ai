use std::collections::BTreeMap;

use finstack_ai_kernel::{CompactionAuthorization, Message, ToolCallId};
#[cfg(test)]
use finstack_ai_kernel::{
    EffectInput, EffectKind, EffectOutputKind, EffectPurpose, EffectRequested, RawJson, RecordBody,
    RecordEnvelope,
};

use super::digest::{
    compaction_projection_digest, compaction_protected_set_digest, compaction_source_digest,
    compaction_summary_digest, sensitivity_rank,
};
use super::error::MiddlewareError;
use super::types::{
    BeforeModelInput, CompactionModelRequest, CompactionResult, MiddlewareDescriptor,
    MiddlewareRole, Stage, StageInput, StageOutcome, canonical_bytes,
};
use super::{COMPACTION_BUDGET_EXCEEDED, COMPACTION_MODEL_NOT_AUTHORIZED};

/// Validate a related committed Model effect before model-assisted compaction dispatch.
///
/// # Errors
///
/// Returns `compaction_model_not_authorized` unless the child effect is
/// related to the exact compaction parent and freezes the requested model
/// draft.
#[cfg(test)]
pub(crate) fn validate_compaction_model_effect(
    parent: &EffectRequested,
    child_envelope: &RecordEnvelope,
    request: &CompactionModelRequest,
) -> Result<(), MiddlewareError> {
    let RecordBody::EffectRequested(child) = child_envelope.body() else {
        return Err(MiddlewareError::compaction_model_not_authorized());
    };
    let raw = RawJson::parse(request.request.canonical_bytes().map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request could not be normalized",
        )
    })?)
    .map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request could not be normalized",
        )
    })?;
    let relation_matches = child.relation().is_some_and(|relation| {
        relation.parent_effect_id == parent.effect_id()
            && matches!(
                &relation.purpose,
                EffectPurpose::CompactionSummary {
                    middleware_component_id
                } if parent.component().is_some_and(|component| &component.component == middleware_component_id)
            )
    });
    if parent.kind() != EffectKind::Middleware
        || child.kind() != EffectKind::Model
        || child_envelope.sequence() == 0
        || !relation_matches
        || !child.component().is_some_and(|value| {
            &value.component == request.model.id()
                && request
                    .model
                    .version()
                    .is_some_and(|version| value.version == version)
        })
        || !matches!(child.input(), EffectInput::Model { request } if request == &raw)
        || child.output_contract().kind != EffectOutputKind::ModelResponse
    {
        return Err(MiddlewareError::compaction_model_not_authorized());
    }
    Ok(())
}

/// Validate the fixed stage/outcome matrix and descriptor-specific restrictions.
///
/// # Errors
///
/// Returns `middleware_outcome_not_allowed` or a precise compaction error.
pub fn validate_stage_outcome(
    descriptor: &MiddlewareDescriptor,
    input: &StageInput,
    outcome: &StageOutcome,
) -> Result<(), MiddlewareError> {
    let stage = input.stage();
    if !descriptor.stages.contains(stage) {
        return Err(MiddlewareError::outcome_not_allowed());
    }
    let allowed = match outcome {
        StageOutcome::Continue
        | StageOutcome::Fail(_)
        | StageOutcome::Suspend(_)
        | StageOutcome::RequestInteraction(_) => true,
        StageOutcome::Replace(_) => stage != Stage::BeforeFinalize,
        StageOutcome::AddInstructions(_) | StageOutcome::AddContext(_) => {
            matches!(stage, Stage::PrepareContext | Stage::BeforeModel)
        }
        StageOutcome::FilterTools(_) => {
            matches!(stage, Stage::BeforeModel | Stage::BeforeToolBatch)
        }
        StageOutcome::CompactContext(_) | StageOutcome::RequestCompactionModel(_) => {
            stage == Stage::BeforeModel
                && matches!(descriptor.role, MiddlewareRole::ContextCompactor { .. })
        }
        StageOutcome::Retry(_) => {
            matches!(
                stage,
                Stage::AfterModel | Stage::AfterToolBatch | Stage::BeforeFinalize
            )
        }
        StageOutcome::Complete(_) => {
            matches!(
                stage,
                Stage::BeforeRun
                    | Stage::AfterModel
                    | Stage::AfterToolBatch
                    | Stage::BeforeFinalize
            )
        }
    };
    if !allowed {
        return Err(MiddlewareError::outcome_not_allowed());
    }
    if matches!(descriptor.role, MiddlewareRole::PostCompactionValidator)
        && !matches!(
            outcome,
            StageOutcome::Continue
                | StageOutcome::Fail(_)
                | StageOutcome::Suspend(_)
                | StageOutcome::RequestInteraction(_)
        )
    {
        return Err(MiddlewareError::outcome_not_allowed());
    }
    match outcome {
        StageOutcome::CompactContext(result) => {
            let StageInput::BeforeModel(before_model) = input else {
                return Err(MiddlewareError::outcome_not_allowed());
            };
            validate_compaction_result(descriptor, before_model, result)
        }
        StageOutcome::RequestCompactionModel(request) => validate_compaction_model_request(request),
        _ => Ok(()),
    }
}

/// Validate protected content, tool-pair atomicity, attribution, and hard budget.
///
/// # Errors
///
/// Returns a stable compaction or context-budget error without mutating canonical history.
pub fn validate_compaction_result(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    result: &CompactionResult,
) -> Result<(), MiddlewareError> {
    let MiddlewareRole::ContextCompactor {
        strategy_id,
        strategy_version,
    } = &descriptor.role
    else {
        return Err(MiddlewareError::outcome_not_allowed());
    };
    validate_compaction_evidence(descriptor, input, result, strategy_id, *strategy_version)?;

    if !input.source_entries.last().is_some_and(|entry| {
        entry.protected && entry.message.role() == finstack_ai_kernel::MessageRole::User
    }) {
        return Err(MiddlewareError::compaction_invalid());
    }
    let source_by_message = input
        .source_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (*entry.message.id(), (index, entry)))
        .collect::<BTreeMap<_, _>>();
    let mut replacements = BTreeMap::new();
    let mut actual_retained = Vec::new();
    let mut last_source_index = None;
    for message in result.replacement_messages.iter() {
        let Some((source_index, source)) = source_by_message.get(message.id()) else {
            return Err(MiddlewareError::compaction_invalid());
        };
        if last_source_index.is_some_and(|last| last >= *source_index) {
            return Err(MiddlewareError::compaction_invalid());
        }
        if replacements.insert(*message.id(), message).is_some() {
            return Err(MiddlewareError::compaction_invalid());
        }
        last_source_index = Some(*source_index);
        actual_retained.push(source.entry_id);
    }
    if actual_retained.as_slice() != result.evidence.retained_entry_ids.as_ref() {
        return Err(MiddlewareError::compaction_invalid());
    }
    for entry in input.source_entries.iter().filter(|entry| entry.protected) {
        let Some(replacement) = replacements.get(entry.message.id()) else {
            return Err(MiddlewareError::compaction_invalid());
        };
        if canonical_bytes(*replacement)? != canonical_bytes(&entry.message)? {
            return Err(MiddlewareError::compaction_invalid());
        }
    }
    let source_pairs = tool_pairs(input.source_entries.iter().map(|entry| &entry.message))?;
    let replacement_pairs = tool_pairs(result.replacement_messages.iter())?;
    for (call, has_result) in source_pairs {
        let retained = replacement_pairs.get(&call).copied();
        if has_result && retained.is_some_and(|result_present| !result_present) {
            return Err(MiddlewareError::compaction_invalid());
        }
    }
    let source_sensitivity = input
        .source_entries
        .iter()
        .map(|entry| sensitivity_rank(entry.sensitivity))
        .max()
        .unwrap_or(0);
    for summary in result.derived_summaries.iter() {
        if summary.kind != crate::ContextItemKind::DerivedSummary
            || summary.authority != crate::ContextAuthority::Untrusted
            || sensitivity_rank(summary.sensitivity) < source_sensitivity
        {
            return Err(MiddlewareError::compaction_invalid());
        }
    }
    validate_checkpoint(descriptor, input, result)?;
    Ok(())
}

fn validate_compaction_evidence(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    result: &CompactionResult,
    strategy_id: &str,
    strategy_version: u32,
) -> Result<(), MiddlewareError> {
    if input.source_entries.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
        || result.replacement_messages.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
        || result.derived_summaries.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
    {
        return Err(MiddlewareError::compaction_invalid());
    }
    let source_digest = compaction_source_digest(&input.source_entries)?;
    let protected_ids = input
        .source_entries
        .iter()
        .filter(|entry| entry.protected)
        .map(|entry| entry.entry_id)
        .collect::<Vec<_>>();
    let protected_digest = compaction_protected_set_digest(&protected_ids)?;
    let projection_digest = compaction_projection_digest(&result.replacement_messages)?;
    let covered = input
        .source_entries
        .iter()
        .map(|entry| entry.entry_id)
        .collect::<Vec<_>>();
    if result.evidence.strategy_id.as_ref() != strategy_id
        || result.evidence.strategy_version != strategy_version
        || result.evidence.configuration_digest != descriptor.invocation.configuration_digest
        || result.evidence.model_context_profile_digest != input.model_context_profile_digest
        || result.evidence.source_digest != source_digest
        || result.evidence.protected_item_set_digest != protected_digest
        || result.evidence.projection_digest != projection_digest
        || result.evidence.covered_entry_ids.as_ref() != covered
        || result.evidence.estimated_tokens_after > input.hard_input_tokens
        || result.evidence.estimated_tokens_after > result.evidence.estimated_tokens_before
    {
        return Err(
            if result.evidence.estimated_tokens_after > input.hard_input_tokens {
                MiddlewareError::stable(
                    COMPACTION_BUDGET_EXCEEDED,
                    "compacted projection exceeds the hard model-input budget",
                )
            } else {
                MiddlewareError::compaction_invalid()
            },
        );
    }

    Ok(())
}

fn validate_compaction_model_request(
    request: &CompactionModelRequest,
) -> Result<(), MiddlewareError> {
    if request.resume_state.as_bytes().len() > 1_048_576 {
        return Err(MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction resume state exceeds its hard bound",
        ));
    }
    request.request.canonical_bytes().map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request is invalid",
        )
    })?;
    Ok(())
}

/// Require the accepted durable compaction lock before commit, dispatch, or resume.
///
/// Absence, model mismatch, residency-digest mismatch, or `source_sensitivity`
/// above the lock ceiling fails closed with `compaction_model_not_authorized`.
///
/// # Errors
///
/// Returns [`MiddlewareError`] when the request is not authorized by `authorization`.
pub fn authorize_compaction_model_request(
    authorization: Option<&CompactionAuthorization>,
    request: &CompactionModelRequest,
) -> Result<(), MiddlewareError> {
    validate_compaction_model_request(request)?;
    let Some(authorization) = authorization else {
        return Err(MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request lacks durable run authorization",
        ));
    };
    if !authorization.authorizes(
        &request.model,
        request.source_sensitivity,
        &request.residency_policy_digest,
    ) {
        return Err(MiddlewareError::stable(
            COMPACTION_MODEL_NOT_AUTHORIZED,
            "compaction model request does not match the accepted authorization lock",
        ));
    }
    Ok(())
}

fn validate_checkpoint(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    result: &CompactionResult,
) -> Result<(), MiddlewareError> {
    let Some(checkpoint) = &result.checkpoint else {
        return Ok(());
    };
    let Some(last_covered) = result.evidence.covered_entry_ids.last() else {
        return Err(MiddlewareError::compaction_invalid());
    };
    if checkpoint.component_id != descriptor.invocation.component
        || checkpoint.strategy_id != result.evidence.strategy_id
        || checkpoint.strategy_version != result.evidence.strategy_version
        || checkpoint.configuration_digest != descriptor.invocation.configuration_digest
        || checkpoint.model_context_profile_digest != input.model_context_profile_digest
        || checkpoint.covered_through_entry_id != *last_covered
        || checkpoint.source_digest != result.evidence.source_digest
    {
        return Err(MiddlewareError::compaction_invalid());
    }
    let summary_digest = compaction_summary_digest(&checkpoint.summary)?;
    let source_sensitivity = input
        .source_entries
        .iter()
        .map(|entry| sensitivity_rank(entry.sensitivity))
        .max()
        .unwrap_or(0);
    if checkpoint.summary_digest != summary_digest
        || result.evidence.summary_digest != Some(summary_digest)
        || sensitivity_rank(checkpoint.sensitivity) < source_sensitivity
    {
        return Err(MiddlewareError::compaction_invalid());
    }
    Ok(())
}

fn tool_pairs<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
) -> Result<BTreeMap<ToolCallId, bool>, MiddlewareError> {
    let mut pairs = BTreeMap::<ToolCallId, bool>::new();
    for message in messages {
        let known = pairs.keys().copied().collect::<Vec<_>>();
        message
            .validate_tool_associations(Some(&known))
            .map_err(|_| MiddlewareError::compaction_invalid())?;
        for block in message.content() {
            match block {
                finstack_ai_kernel::ContentBlock::ToolCall(call) => {
                    if pairs.insert(*call.tool_call_id(), false).is_some() {
                        return Err(MiddlewareError::compaction_invalid());
                    }
                }
                finstack_ai_kernel::ContentBlock::ToolResult(result) => {
                    let Some(value) = pairs.get_mut(result.tool_call_id()) else {
                        return Err(MiddlewareError::compaction_invalid());
                    };
                    *value = true;
                }
                _ => {}
            }
        }
    }
    Ok(pairs)
}
