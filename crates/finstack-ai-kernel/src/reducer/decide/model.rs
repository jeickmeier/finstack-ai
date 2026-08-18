use crate::content::LABEL_MAX_BYTES;
use crate::conversation::Message;
use crate::effects::{
    EffectCompleted, EffectDeferred, EffectFailed, EffectInput, EffectKind, EffectOutputKind,
    EffectPurpose, EffectRequested, NestedModelKind,
};
use crate::primitives::Digest;
use crate::records::RecordBody;
use crate::records::lifecycle::EntryAppended;
use crate::state::{KernelState, RunPhase, TransitionEnv};

use super::super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::super::capacity::{self, StateGrowth};
use super::super::decision::{Decision, KernelError};
use super::super::fingerprint::{direct_digest, external_digest};
use super::super::input::{
    ExternalEffectCompletedInput, ExternalEffectOutcome, ModelSettled, ModelSettlement,
    RequestCompactionModel,
};
use super::super::validation::{
    assistant_tool_calls, validate_assistant_message_id, validate_assistant_semantics,
    validate_assistant_tool_call_ids, validate_completion_identity, validate_error_descriptor,
};
use super::{
    draft_for_state, duplicate_decision, expected_stage_cursor, next_sequence, reject_terminal,
    required,
};

/// Commit one runtime-owned compaction-summary model effect (ADR-042).
///
/// Does not consume the `BeforeModel` cursor. Emits no post-commit action; the
/// runtime phase executes the model after the request is journaled.
pub(super) fn decide_request_compaction_model(
    state: &KernelState,
    env: &TransitionEnv,
    input: &RequestCompactionModel,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    match &input.relation.purpose {
        EffectPurpose::CompactionSummary { .. } => {
            if state.phase != Some(RunPhase::BeforeModel) || state.pending_model_effect.is_some() {
                return Err(KernelError::InvalidPhaseInput {
                    phase: state.phase,
                    input: "request_compaction_model",
                });
            }
            expected_stage_cursor(state).ok_or(KernelError::InvalidPhaseInput {
                phase: state.phase,
                input: "request_compaction_model",
            })?;
        }
        EffectPurpose::NestedModel {
            kind: NestedModelKind::McpSampling,
        } => {
            if state.phase != Some(RunPhase::AwaitingTools) || state.pending_model_effect.is_some()
            {
                return Err(KernelError::InvalidPhaseInput {
                    phase: state.phase,
                    input: "request_nested_model",
                });
            }
            let parent = input.relation.parent_effect_id;
            let parent_open = state.active_tool_batch.as_ref().is_some_and(|batch| {
                batch.calls.iter().any(|call| {
                    matches!(
                        &call.status,
                        crate::ActiveToolCallStatus::Requested { requested, .. }
                            if requested.effect_id() == parent
                    )
                })
            });
            if !parent_open {
                return Err(KernelError::ModelRequestContractMismatch);
            }
        }
    }
    if state.current_turn.is_none() {
        let input = match &input.relation.purpose {
            EffectPurpose::CompactionSummary { .. } => "request_compaction_model",
            EffectPurpose::NestedModel { .. } => "request_nested_model",
        };
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input,
        });
    }
    if input.output_contract.kind != EffectOutputKind::ModelResponse {
        return Err(KernelError::ModelRequestContractMismatch);
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 1, 1, 0, 0, 0))?;
    let effect_id = required(env.ids.effect_ids(), 0, "effect_ids")?;
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Model,
        Some(input.relation.clone()),
        input.component.clone(),
        None,
        input.output_contract.clone(),
        EffectInput::Model {
            request: input.request.clone(),
        },
        input.retry_safety,
        input.deadline,
    )
    .map_err(|_| KernelError::ModelRequestContractMismatch)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(state, env, vec![RecordBody::EffectRequested(requested)])?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

use crate::primitives::SEMANTIC_ARRAY_MAX_ITEMS;

pub(super) fn decide_model(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ModelSettled,
) -> Result<Decision, KernelError> {
    if let ModelSettlement::Failed(failed) = &input.outcome {
        validate_error_descriptor(failed.error())?;
    }
    let settlement_digest = direct_digest(input)?;
    decide_normalized_model(state, env, input, "model_settled", settlement_digest)
}

fn validate_external_completion_input(
    input: &ExternalEffectCompletedInput,
) -> Result<(), KernelError> {
    if input.completion.completion_id.is_empty()
        || input.completion.completion_id.len() > LABEL_MAX_BYTES
        || input.completion.completion_id.as_bytes().contains(&0)
    {
        return Err(KernelError::InvalidInputPayload {
            field: "completion_id",
            reason_code: "invalid_label",
        });
    }
    if matches!(
        &input.completion.outcome,
        ExternalEffectOutcome::Completed { artifacts, .. }
            if artifacts.len() > SEMANTIC_ARRAY_MAX_ITEMS
    ) {
        return Err(KernelError::InvalidInputPayload {
            field: "artifacts",
            reason_code: "too_many_items",
        });
    }
    if let ExternalEffectOutcome::Failed { error } = &input.completion.outcome {
        validate_error_descriptor(error)?;
    }
    Ok(())
}

pub(super) fn decide_external(
    state: &KernelState,
    env: &TransitionEnv,
    input: ExternalEffectCompletedInput,
) -> Result<Decision, KernelError> {
    validate_external_completion_input(&input)?;
    if super::super::tool::is_known_tool_effect(state, input.completion.effect_id) {
        return super::super::tool::decide_external_tool(state, env, input);
    }
    let settlement_digest = external_digest(&input)?;
    if let Some(decision) = classify_model_duplicate(
        state,
        Some(input.completion.completion_id.as_ref()),
        input.completion.effect_id,
        settlement_digest,
        None,
    )? {
        return Ok(decision);
    }
    if state.phase != Some(RunPhase::AwaitingExternal) {
        reject_terminal(state)?;
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "external_effect_completed",
        });
    }
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    if pending.requested.effect_id() != input.completion.effect_id {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    }
    if pending.deferred.is_none() {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    }
    let completion_id = input.completion.completion_id;
    let outcome = match input.completion.outcome {
        ExternalEffectOutcome::Completed {
            output,
            usage,
            artifacts,
        } => {
            let assistant_message = input
                .assistant_message
                .ok_or(KernelError::AssistantMessagePresenceMismatch)?;
            let completion = EffectCompleted::try_new(
                pending.requested.effect_id(),
                pending.requested.output_contract().clone(),
                output,
                usage,
                artifacts.to_vec(),
                assistant_message.provider_ids().clone(),
                Some(completion_id.as_ref()),
                None,
            )
            .map_err(|_| KernelError::ModelSettlementMismatch)?;
            ModelSettlement::Completed {
                completion,
                assistant_message,
            }
        }
        ExternalEffectOutcome::Failed { error } => {
            if input.assistant_message.is_some() {
                return Err(KernelError::AssistantMessagePresenceMismatch);
            }
            ModelSettlement::Failed(
                EffectFailed::try_new(
                    pending.requested.effect_id(),
                    pending.requested.output_contract().clone(),
                    error,
                    None,
                    Some(completion_id.as_ref()),
                )
                .map_err(|_| KernelError::ModelSettlementMismatch)?,
            )
        }
    };
    let normalized = ModelSettled {
        turn_id: pending.turn_id,
        model_request_id: pending.model_request_id,
        outcome,
    };
    decide_normalized_model(
        state,
        env,
        &normalized,
        "external_effect_completed",
        settlement_digest,
    )
}

fn decide_normalized_model(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ModelSettled,
    input_name: &'static str,
    settlement_digest: Digest,
) -> Result<Decision, KernelError> {
    let effect_id = settlement_effect_id(&input.outcome);
    if let Some(decision) = classify_model_duplicate(
        state,
        settlement_completion_id(&input.outcome),
        effect_id,
        settlement_digest,
        Some(input),
    )? {
        return Ok(decision);
    }
    reject_terminal(state)?;
    let required_phase = match input_name {
        "model_settled" => RunPhase::AwaitingModel,
        "external_effect_completed" => RunPhase::AwaitingExternal,
        _ => return Err(KernelError::InvariantViolation),
    };
    if state.phase != Some(required_phase) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: input_name,
        });
    }
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::ModelSettlementMismatch)?;
    if pending.turn_id != input.turn_id
        || pending.model_request_id != input.model_request_id
        || pending.requested.effect_id() != effect_id
    {
        return Err(KernelError::ModelSettlementMismatch);
    }
    if pending.requested.kind() != EffectKind::Model {
        return Err(KernelError::ModelSettlementMismatch);
    }

    let (requirements, bodies) =
        model_settlement_bodies(state, env, pending, input, effect_id, settlement_digest)?;
    validate_allocated_ids(&env.ids, requirements)?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn classify_model_duplicate(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
    input: Option<&ModelSettled>,
) -> Result<Option<Decision>, KernelError> {
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
    {
        return if existing.effect_id == effect_id && existing.settlement_digest == settlement_digest
        {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.model_settlements.get(&effect_id) {
        return if existing.digest == settlement_digest {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if let Some(ModelSettled {
        outcome: ModelSettlement::Deferred(deferred),
        ..
    }) = input
        && let Some(pending) = state.pending_model_effect.as_ref()
        && pending.requested.effect_id() == deferred.effect_id
        && let Some(existing) = pending.deferred.as_ref()
    {
        let existing_digest = direct_digest(&ModelSettled {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            outcome: ModelSettlement::Deferred(existing.clone()),
        })?;
        return if existing_digest == settlement_digest {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    Ok(None)
}

fn model_settlement_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    pending: &crate::PendingModelEffect,
    input: &ModelSettled,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    match &input.outcome {
        ModelSettlement::Completed {
            completion,
            assistant_message,
        } => completed_settlement_bodies(
            state,
            env,
            pending,
            completion,
            assistant_message,
            effect_id,
            settlement_digest,
        ),
        ModelSettlement::Deferred(deferred) => deferred_settlement_bodies(pending, deferred),
        ModelSettlement::Failed(failed) => {
            failed_settlement_bodies(state, pending, failed, effect_id, settlement_digest)
        }
    }
}

fn completed_settlement_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    pending: &crate::PendingModelEffect,
    completion: &EffectCompleted,
    assistant_message: &Message,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    completion
        .validate_against(&pending.requested)
        .map_err(|_| KernelError::ModelSettlementMismatch)?;
    if completion.output_contract().kind != EffectOutputKind::ModelResponse {
        return Err(KernelError::ModelSettlementMismatch);
    }
    if pending.requested.is_runtime_owned_child_model() {
        return compaction_summary_completion_bodies(
            state,
            completion,
            effect_id,
            settlement_digest,
        );
    }
    assistant_completion_bodies(
        state,
        env,
        pending,
        completion,
        assistant_message,
        effect_id,
        settlement_digest,
    )
}

fn compaction_summary_completion_bodies(
    state: &KernelState,
    completion: &EffectCompleted,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    validate_completion_identity(
        state,
        completion.completion_id(),
        effect_id,
        settlement_digest,
    )?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            model: Some(effect_id),
            completion: completion.completion_id(),
            ..StateGrowth::default()
        },
    )?;
    Ok((
        IdRequirements::new(1, 1, 0, 0, 0, 0),
        vec![RecordBody::EffectCompleted(completion.clone())],
    ))
}

fn assistant_completion_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    pending: &crate::PendingModelEffect,
    completion: &EffectCompleted,
    assistant_message: &Message,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    validate_completion_identity(
        state,
        completion.completion_id(),
        effect_id,
        settlement_digest,
    )?;
    validate_assistant_semantics(state, env, assistant_message, completion)?;
    let tool_call_ids = assistant_tool_calls(assistant_message)
        .iter()
        .map(|call| *call.tool_call_id())
        .collect::<Vec<_>>();
    capacity::preflight_decision(
        state,
        StateGrowth {
            messages: 1,
            model: Some(effect_id),
            completion: completion.completion_id(),
            tool_calls: &tool_call_ids,
            ..StateGrowth::default()
        },
    )?;
    let requirements = IdRequirements::new(2, 2, 0, 0, 0, 1).with_tools(0, tool_call_ids.len());
    validate_allocated_ids(&env.ids, requirements)?;
    let message_id = required(env.ids.message_ids(), 0, "message_ids")?;
    validate_assistant_message_id(message_id, assistant_message)?;
    validate_assistant_tool_call_ids(env.ids.tool_call_ids(), assistant_message)?;
    let parent_message_id = state.messages.last().map(|message| *message.id());
    Ok((
        requirements,
        vec![
            RecordBody::EffectCompleted(completion.clone()),
            RecordBody::EntryAppended(EntryAppended {
                cycle: pending.cycle,
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                effect_id,
                parent_message_id,
                message: assistant_message.clone(),
            }),
        ],
    ))
}

fn deferred_settlement_bodies(
    pending: &crate::PendingModelEffect,
    deferred: &EffectDeferred,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    if pending.requested.is_runtime_owned_child_model() {
        return Err(KernelError::ModelSettlementMismatch);
    }
    deferred
        .validate_against(&pending.requested)
        .map_err(|_| KernelError::ModelSettlementMismatch)?;
    Ok((
        IdRequirements::new(1, 1, 0, 0, 0, 0),
        vec![RecordBody::EffectDeferred(deferred.clone())],
    ))
}

fn failed_settlement_bodies(
    state: &KernelState,
    pending: &crate::PendingModelEffect,
    failed: &EffectFailed,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    failed
        .validate_against(&pending.requested)
        .map_err(|_| KernelError::ModelSettlementMismatch)?;
    validate_completion_identity(state, failed.completion_id(), effect_id, settlement_digest)?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            model: Some(effect_id),
            completion: failed.completion_id(),
            ..StateGrowth::default()
        },
    )?;
    Ok((
        IdRequirements::new(1, 1, 0, 0, 0, 0),
        vec![RecordBody::EffectFailed(failed.clone())],
    ))
}

fn settlement_effect_id(outcome: &ModelSettlement) -> crate::EffectId {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.effect_id(),
        ModelSettlement::Deferred(deferred) => deferred.effect_id,
        ModelSettlement::Failed(failed) => failed.effect_id(),
    }
}

fn settlement_completion_id(outcome: &ModelSettlement) -> Option<&str> {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.completion_id(),
        ModelSettlement::Deferred(_) => None,
        ModelSettlement::Failed(failed) => failed.completion_id(),
    }
}
