//! Tool-batch settlement decisions.

use super::super::allocated_ids::validate_allocated_ids;
use super::super::capacity::{self, StateGrowth};
use super::super::decide::{draft_for_state, duplicate_decision, next_sequence, reject_terminal};
use super::super::decision::{Decision, KernelError};
use super::super::fingerprint::{direct_tool_digest, external_tool_digest};
use super::super::input::{
    ExternalEffectCompletedInput, ExternalEffectOutcome, ToolBatchSettled, ToolSettlement,
};
use super::super::validation::{validate_completion_identity, validate_error_descriptor};
use crate::conversation::ProviderIds;
use crate::effects::{EffectCompleted, EffectFailed, EffectKind, EffectOutputKind};
use crate::records::RecordBody;
use crate::records::tools::{ActiveToolCallStatus, ToolFailurePolicy};
use crate::state::{KernelState, RunPhase, TransitionEnv};

use super::followups::followup_records;
use super::planning::ensure_record_batch_bound;
use super::records::synthetic_result;
use super::records::{decode_tool_result, requirements_for_bodies};

pub(crate) fn decide_tool_settled(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ToolBatchSettled,
) -> Result<Decision, KernelError> {
    if let ToolSettlement::Failed(failure) = &input.outcome {
        validate_error_descriptor(failure.error())?;
    }
    let digest = direct_tool_digest(input.tool_batch_id, &input.outcome)?;
    settle_normalized_tool(
        state,
        env,
        input.tool_batch_id,
        &input.outcome,
        digest,
        false,
    )
}

pub(crate) fn is_known_tool_effect(state: &KernelState, effect_id: crate::EffectId) -> bool {
    state
        .active_tool_batch
        .as_ref()
        .is_some_and(|batch| batch.call_index(effect_id).is_some())
        || state
            .tool_calls
            .values()
            .any(|identity| identity.effect_id == Some(effect_id))
}

pub(crate) fn decide_external_tool(
    state: &KernelState,
    env: &TransitionEnv,
    input: ExternalEffectCompletedInput,
) -> Result<Decision, KernelError> {
    let indexed_completion = state
        .completion_identities
        .get(input.completion.completion_id.as_ref());
    if indexed_completion.is_some_and(|existing| existing.effect_id != input.completion.effect_id) {
        return Err(KernelError::ConflictingCompletionId);
    }
    let tool_batch_id = state
        .active_tool_batch
        .as_ref()
        .and_then(|batch| {
            batch
                .call_index(input.completion.effect_id)
                .map(|_| batch.opened.tool_batch_id)
        })
        .or_else(|| {
            state
                .tool_calls
                .values()
                .find(|identity| identity.effect_id == Some(input.completion.effect_id))
                .and_then(|identity| identity.tool_batch_id)
        })
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    let digest = external_tool_digest(tool_batch_id, &input)?;
    if let Some(existing) = indexed_completion {
        return if existing.effect_id == input.completion.effect_id
            && existing.settlement_digest == digest
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.tool_settlements.get(&input.completion.effect_id) {
        return if existing.digest == digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if input.assistant_message.is_some() {
        return Err(KernelError::AssistantMessagePresenceMismatch);
    }
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    let active = batch
        .call(input.completion.effect_id)
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    let ActiveToolCallStatus::Requested {
        requested,
        deferred: Some(_),
    } = &active.status
    else {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    };
    let completion_id = input.completion.completion_id;
    let outcome = match input.completion.outcome {
        ExternalEffectOutcome::Completed {
            output,
            usage,
            artifacts,
        } => ToolSettlement::Completed(
            EffectCompleted::try_new(
                requested.effect_id(),
                requested.output_contract().clone(),
                output,
                usage,
                artifacts.to_vec(),
                ProviderIds::empty(),
                Some(completion_id.as_ref()),
                None,
            )
            .map_err(|_| KernelError::ToolSettlementMismatch)?,
        ),
        ExternalEffectOutcome::Failed { error } => ToolSettlement::Failed(
            EffectFailed::try_new(
                requested.effect_id(),
                requested.output_contract().clone(),
                error,
                None,
                Some(completion_id.as_ref()),
            )
            .map_err(|_| KernelError::ToolSettlementMismatch)?,
        ),
    };
    settle_normalized_tool(state, env, tool_batch_id, &outcome, digest, true)
}

#[expect(
    clippy::too_many_lines,
    reason = "direct and external settlements share one ordered fail-closed validation pipeline"
)]
fn settle_normalized_tool(
    state: &KernelState,
    env: &TransitionEnv,
    tool_batch_id: crate::ToolBatchId,
    outcome: &ToolSettlement,
    settlement_digest: crate::Digest,
    external: bool,
) -> Result<Decision, KernelError> {
    let effect_id = outcome.effect_id();
    let completion_id = outcome.completion_id();
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
    {
        return if existing.effect_id == effect_id && existing.settlement_digest == settlement_digest
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.tool_settlements.get(&effect_id) {
        return if existing.digest == settlement_digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    reject_terminal(state)?;
    let valid_phase = if external {
        state.phase == Some(RunPhase::AwaitingExternal)
    } else {
        matches!(
            state.phase,
            Some(RunPhase::AwaitingTools | RunPhase::AwaitingExternal)
        )
    };
    if !valid_phase {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: if external {
                "external_effect_completed"
            } else {
                "tool_batch_settled"
            },
        });
    }
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(KernelError::ToolSettlementMismatch)?;
    if batch.opened.tool_batch_id != tool_batch_id {
        return Err(KernelError::ToolSettlementMismatch);
    }
    let call_index = batch
        .call_index(effect_id)
        .ok_or(KernelError::ToolSettlementMismatch)?;
    let active_call = &batch.calls[call_index];
    let ActiveToolCallStatus::Requested {
        requested,
        deferred,
    } = &active_call.status
    else {
        return Err(KernelError::ToolSettlementMismatch);
    };
    if !external && let (Some(existing), ToolSettlement::Deferred(_)) = (deferred, outcome) {
        let prior = ToolSettlement::Deferred(existing.clone());
        return if direct_tool_digest(tool_batch_id, &prior)? == settlement_digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if external != deferred.is_some() {
        return Err(KernelError::ToolSettlementMismatch);
    }
    if requested.kind() != EffectKind::Tool
        || requested.output_contract().kind != EffectOutputKind::ToolResult
    {
        return Err(KernelError::ToolEffectContractMismatch);
    }

    let mut prospective = batch.clone();
    let mut bodies = Vec::new();
    match outcome {
        ToolSettlement::Completed(completion) => {
            completion
                .validate_against(requested)
                .map_err(|_| KernelError::ToolSettlementMismatch)?;
            if completion.output_contract().kind != EffectOutputKind::ToolResult
                || completion.provider_ids() != &ProviderIds::empty()
                || completion.reservation_id().is_some()
            {
                return Err(KernelError::ToolEffectContractMismatch);
            }
            validate_completion_identity(
                state,
                completion.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            let result = decode_tool_result(completion, active_call.assigned.plan.call())?;
            prospective.set_call_status(
                call_index,
                ActiveToolCallStatus::Buffered {
                    result,
                    settlement_digest,
                    synthetic: false,
                    error: None,
                },
            );
            bodies.push(RecordBody::EffectCompleted(completion.clone()));
        }
        ToolSettlement::Failed(failure) => {
            failure
                .validate_against(requested)
                .map_err(|_| KernelError::ToolSettlementMismatch)?;
            validate_completion_identity(
                state,
                failure.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            let error = failure.error().clone();
            let result = synthetic_result(active_call.assigned.plan.call(), &error)?;
            prospective.set_call_status(
                call_index,
                ActiveToolCallStatus::Buffered {
                    result,
                    settlement_digest,
                    synthetic: true,
                    error: Some(error.clone()),
                },
            );
            if active_call.assigned.plan.failure_policy() == ToolFailurePolicy::FailRun
                && prospective.fatal_error.is_none()
            {
                prospective.fatal_error = Some(error);
            }
            bodies.push(RecordBody::EffectFailed(failure.clone()));
        }
        ToolSettlement::Deferred(value) => {
            value
                .validate_against(requested)
                .map_err(|_| KernelError::ToolSettlementMismatch)?;
            if external {
                return Err(KernelError::ToolSettlementMismatch);
            }
            prospective.set_call_status(
                call_index,
                ActiveToolCallStatus::Requested {
                    requested: requested.clone(),
                    deferred: Some(value.clone()),
                },
            );
            bodies.push(RecordBody::EffectDeferred(value.clone()));
            let requirements = requirements_for_bodies(&bodies, 0, 0, 0)?;
            validate_allocated_ids(&env.ids, requirements)?;
            return Ok(Decision {
                expected_sequence: next_sequence(state)?,
                records: draft_for_state(state, env, bodies)?,
                actions: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
    }

    let mut followups = followup_records(&mut prospective, env)?;
    followups.settlements.push(effect_id);
    bodies.append(&mut followups.bodies);
    ensure_record_batch_bound(bodies.len())?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            messages: followups.messages,
            tool_settlements: &followups.settlements,
            completion: completion_id,
            ..StateGrowth::default()
        },
    )?;
    let requirements = requirements_for_bodies(&bodies, followups.messages, 0, 0)?;
    validate_allocated_ids(&env.ids, requirements)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(state, env, bodies)?,
        actions: followups.actions,
        diagnostics: Vec::new(),
    })
}
