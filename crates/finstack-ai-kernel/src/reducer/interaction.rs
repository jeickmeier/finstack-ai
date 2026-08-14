//! Interaction request and settlement decisions.

use super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::canonical_digest;
use super::capacity::{self, StateGrowth};
use super::decide::{draft_for_state, duplicate_decision, expected_stage_cursor, next_sequence};
use super::decision::{Decision, KernelError};
use super::input::{InteractionSettled, RequestInteraction};
use crate::Digest;
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectFailed, EffectInput, EffectKind, EffectOutputContract,
    EffectOutputKind, EffectRequested, InteractionKind, InteractionResolution, RetrySafety,
};
use crate::error::{ErrorCategory, ErrorDescriptor};
use crate::message::ProviderIds;
use crate::records::RecordBody;
use crate::state::{
    InteractionTerminal, InteractionTerminalOutcome, KernelState, PendingInteraction, RunPhase,
    TransitionEnv,
};

pub(super) fn decide_request(
    state: &KernelState,
    env: &TransitionEnv,
    input: &RequestInteraction,
) -> Result<Decision, KernelError> {
    reject_busy(state, "request_interaction")?;
    if state.pending_interaction.is_some() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "request_interaction",
        });
    }
    let cursor = expected_stage_cursor(state).ok_or(KernelError::InvalidPhaseInput {
        phase: state.phase,
        input: "request_interaction",
    })?;
    let _ = cursor;
    let request = &input.request;
    validate_allocated_ids(
        &env.ids,
        IdRequirements::new(2, 2, 1, 0, 0, 0).with_interactions(1),
    )?;
    if env.ids.effect_ids().first().copied() != Some(request.effect_id()) {
        return Err(KernelError::UnusedAllocatedIds { kind: "effect_ids" });
    }
    if env.ids.interaction_ids().first().copied() != Some(request.interaction_id()) {
        return Err(KernelError::UnusedAllocatedIds {
            kind: "interaction_ids",
        });
    }
    let request_digest = request
        .request_digest()
        .map_err(|_| KernelError::InvariantViolation)?;
    let requested = EffectRequested::try_new(
        request.effect_id(),
        EffectKind::Interaction,
        None,
        None,
        None,
        interaction_contract(request.response_schema_digest()),
        EffectInput::Interaction {
            interaction_id: request.interaction_id(),
            request_digest,
        },
        RetrySafety::AtMostOnce,
        request.expires_at(),
    )
    .map_err(|_| KernelError::InvariantViolation)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(
            state,
            env,
            vec![
                RecordBody::EffectRequested(requested),
                RecordBody::InteractionRequested(request.clone()),
            ],
        )?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

pub(super) fn decide_settled(
    state: &KernelState,
    env: &TransitionEnv,
    input: &InteractionSettled,
) -> Result<Decision, KernelError> {
    if let InteractionSettled::Resolved(resolution) = input {
        let digest = resolution_digest(resolution)?;
        if let Some(existing) = state.resolution_identities.get(resolution.resolution_id()) {
            return if existing.settlement_digest == digest {
                duplicate_decision(state)
            } else {
                Err(KernelError::ConflictingSettlement)
            };
        }
    }
    reject_busy(state, "interaction_settled")?;
    if state.phase != Some(RunPhase::AwaitingInteraction) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "interaction_settled",
        });
    }
    let pending = state
        .pending_interaction
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "interaction_settled",
        })?;
    match input {
        InteractionSettled::Resolved(resolution) => {
            decide_resolved(state, env, pending, resolution)
        }
        InteractionSettled::Expired(expired) => {
            if expired.interaction_id != pending.request.interaction_id() {
                return Err(KernelError::InvalidPhaseInput {
                    phase: state.phase,
                    input: "interaction_settled",
                });
            }
            validate_allocated_ids(&env.ids, IdRequirements::new(2, 2, 0, 0, 0, 0))?;
            let failed = EffectFailed::try_new(
                pending.request.effect_id(),
                interaction_contract(pending.request.response_schema_digest()),
                ErrorDescriptor::new(
                    "interaction_expired",
                    "interaction expired before resolution",
                    ErrorCategory::Deadline,
                    false,
                )
                .map_err(|_| KernelError::InvariantViolation)?,
                None,
                Some(expired.interaction_id.to_canonical_string()),
            )
            .map_err(|_| KernelError::InvariantViolation)?;
            Ok(Decision {
                expected_sequence: next_sequence(state)?,
                records: draft_for_state(
                    state,
                    env,
                    vec![
                        RecordBody::InteractionExpired(expired.clone()),
                        RecordBody::EffectFailed(failed),
                    ],
                )?,
                actions: Vec::new(),
                diagnostics: Vec::new(),
            })
        }
        InteractionSettled::Cancelled(cancelled) => {
            if cancelled.interaction_id() != pending.request.interaction_id() {
                return Err(KernelError::InvalidPhaseInput {
                    phase: state.phase,
                    input: "interaction_settled",
                });
            }
            validate_allocated_ids(&env.ids, IdRequirements::new(2, 2, 0, 0, 0, 0))?;
            let effect = EffectCancelled::try_new(
                pending.request.effect_id(),
                interaction_contract(pending.request.response_schema_digest()),
                cancelled.reason(),
                Some(cancelled.interaction_id().to_canonical_string()),
            )
            .map_err(|_| KernelError::InvariantViolation)?;
            Ok(Decision {
                expected_sequence: next_sequence(state)?,
                records: draft_for_state(
                    state,
                    env,
                    vec![
                        RecordBody::InteractionCancelled(cancelled.clone()),
                        RecordBody::EffectCancelled(effect),
                    ],
                )?,
                actions: Vec::new(),
                diagnostics: Vec::new(),
            })
        }
    }
}

fn decide_resolved(
    state: &KernelState,
    env: &TransitionEnv,
    pending: &PendingInteraction,
    resolution: &InteractionResolution,
) -> Result<Decision, KernelError> {
    if resolution.interaction_id() != pending.request.interaction_id() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "interaction_settled",
        });
    }
    let accepted = state
        .accepted
        .as_ref()
        .ok_or(KernelError::InvalidRunAcceptance)?;
    let security = accepted.security();
    if security.principal() != resolution.principal()
        || security.authorization_policy_version() != resolution.authorization().policy_version()
        || security.authorization_decision_id() != resolution.authorization().decision_id()
    {
        return Err(KernelError::InvalidRunAcceptance);
    }
    if let Some(crate::AssigneeHint::Principal(expected)) = pending.request.assignee_hint()
        && expected != resolution.principal()
        && !(pending.request.delegatable() && security.delegated_from() == Some(expected))
    {
        return Err(KernelError::InvalidRunAcceptance);
    }
    if let Some(deadline) = pending.request.expires_at()
        && env.now >= deadline
    {
        return Err(KernelError::InvalidInputPayload {
            field: "expires_at",
            reason_code: "interaction_expired",
        });
    }
    let _ = approval_outcome(pending.request.kind(), resolution.response())?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            completion: Some(resolution.resolution_id()),
            resolution: Some(resolution.resolution_id()),
            ..StateGrowth::default()
        },
    )?;
    validate_allocated_ids(&env.ids, IdRequirements::new(2, 2, 0, 0, 0, 0))?;
    let completed = EffectCompleted::try_new(
        pending.request.effect_id(),
        interaction_contract(pending.request.response_schema_digest()),
        resolution.response().clone(),
        None,
        vec![],
        ProviderIds::empty(),
        Some(resolution.resolution_id()),
        None,
    )
    .map_err(|_| KernelError::InvariantViolation)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(
            state,
            env,
            vec![
                RecordBody::InteractionResolved(resolution.clone()),
                RecordBody::EffectCompleted(completed),
            ],
        )?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

pub(super) fn approval_outcome(
    kind: &InteractionKind,
    response: &crate::RawJson,
) -> Result<InteractionTerminalOutcome, KernelError> {
    if !matches!(kind, InteractionKind::Approval) {
        return Ok(InteractionTerminalOutcome::Granted);
    }
    let value: serde_json::Value =
        serde_json::from_str(response.as_str()).map_err(|_| KernelError::InvalidInputPayload {
            field: "response",
            reason_code: "invalid_approval",
        })?;
    let Some(object) = value.as_object() else {
        return Err(KernelError::InvalidInputPayload {
            field: "response",
            reason_code: "invalid_approval",
        });
    };
    if object.len() != 1 {
        return Err(KernelError::InvalidInputPayload {
            field: "response",
            reason_code: "invalid_approval",
        });
    }
    match object.get("approved").and_then(serde_json::Value::as_bool) {
        Some(true) => Ok(InteractionTerminalOutcome::Granted),
        Some(false) => Ok(InteractionTerminalOutcome::Denied),
        None => Err(KernelError::InvalidInputPayload {
            field: "response",
            reason_code: "invalid_approval",
        }),
    }
}

pub(super) fn resolution_digest(resolution: &InteractionResolution) -> Result<Digest, KernelError> {
    canonical_digest("interaction-resolution", resolution)
}

pub(super) fn interaction_contract(schema_digest: Digest) -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::InteractionResolution,
        schema_version: 1,
        schema_digest,
    }
}

pub(super) fn terminal_from_pending(
    pending: &PendingInteraction,
    outcome: InteractionTerminalOutcome,
) -> InteractionTerminal {
    InteractionTerminal {
        interaction_id: pending.request.interaction_id(),
        kind: pending.request.kind().clone(),
        cursor: pending.cursor,
        outcome,
    }
}

fn reject_busy(state: &KernelState, input: &'static str) -> Result<(), KernelError> {
    if state.terminal.is_some() {
        return Err(KernelError::TerminalStateImmutable);
    }
    if state.cancellation.is_some() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input,
        });
    }
    Ok(())
}
