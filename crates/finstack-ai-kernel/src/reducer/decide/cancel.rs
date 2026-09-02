use crate::effects::{EffectCancelled, InteractionCancelled};
use crate::primitives::ErrorCode;
use crate::records::lifecycle::{RunCancelled, RunSuspended, TimerFired};
use crate::records::{APPEND_BATCH_MAX_RECORDS, RECORD_KIND_VERSION, RecordBody};
use crate::state::{KernelState, RunPhase, TransitionEnv};
use crate::{
    CancellationInitiator, CancellationReconciled, CancellationRequest, CancellationRequested,
};

use super::super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::super::capacity::{self, StateGrowth};
use super::super::decision::{Decision, KernelError, PostCommitAction};
use super::super::input::{CancelRequested, CancellationReconciledInput, TimerFiredInput};
use super::shared::timer_firing_contract;
use super::{
    decision_for, duplicate_decision, outstanding_requested_effects, reject_terminal, required,
};
use crate::primitives::SEMANTIC_ARRAY_MAX_ITEMS;

pub(super) fn decide_cancel(
    state: &KernelState,
    env: &TransitionEnv,
    input: &CancelRequested,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    if let Some(cancellation) = &state.cancellation {
        return if cancellation.request.initiator == input.initiator
            && cancellation.request.reason.as_deref() == input.reason.as_deref()
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    let accepted = state
        .accepted
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "cancel_requested",
        })?;
    validate_cancel_authorization(accepted, env, input)?;
    validate_allocated_ids(
        &env.ids,
        IdRequirements::new(1, 0, 0, 0, 0, 0).with_cancellations(1),
    )?;
    let request_id = required(
        env.ids.cancellation_request_ids(),
        0,
        "cancellation_request_ids",
    )?;
    let request =
        CancellationRequest::try_new(request_id, input.initiator.clone(), input.reason.as_deref())
            .map_err(|_| KernelError::InvalidInputPayload {
                field: "reason",
                reason_code: "invalid_label",
            })?;
    let actions = outstanding_requested_effects(state)
        .into_iter()
        .map(|effect_id| PostCommitAction::CancelEffect { effect_id })
        .collect();
    decision_for(
        state,
        env,
        vec![RecordBody::CancellationRequested(CancellationRequested {
            request,
        })],
        actions,
    )
}

pub(super) fn validate_cancel_authorization(
    accepted: &crate::RunAccepted,
    env: &TransitionEnv,
    input: &CancelRequested,
) -> Result<(), KernelError> {
    let authorized = match &input.initiator {
        CancellationInitiator::Principal {
            principal,
            authorization,
        } => {
            principal == accepted.security().principal()
                && authorization.policy_version()
                    == accepted.security().authorization_policy_version()
                && authorization.decision_id() == accepted.security().authorization_decision_id()
        }
        CancellationInitiator::ParentRun { parent_run_id } => {
            accepted.relation().parent_run_id() == Some(*parent_run_id)
                && match accepted.propagation().cancellation {
                    crate::CancellationPropagation::Cascade => true,
                    crate::CancellationPropagation::DetachOnlyIfPreauthorized => !accepted
                        .security()
                        .authorization_decision_id()
                        .starts_with("detach:"),
                }
        }
        CancellationInitiator::Deadline => accepted
            .effective_deadline()
            .is_some_and(|deadline| env.now >= deadline),
        CancellationInitiator::RuntimeShutdown => true,
    };
    if !authorized {
        return Err(KernelError::InvalidInputPayload {
            field: "initiator",
            reason_code: "unauthorized",
        });
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "incremental classification and deterministic effect closure form one atomic decision"
)]
pub(super) fn decide_reconciliation(
    state: &KernelState,
    env: &TransitionEnv,
    input: &CancellationReconciledInput,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    let cancellation = state
        .cancellation
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "cancellation_reconciled",
        })?;
    if input.request_id != cancellation.request.request_id {
        return Err(KernelError::ConflictingSettlement);
    }
    validate_reconciliation_input(input, cancellation)?;
    let duplicate = input
        .completed_effects
        .iter()
        .all(|id| cancellation.completed_effects.contains(id))
        && input
            .cancelled_effects
            .iter()
            .all(|id| cancellation.cancelled_effects.contains(id))
        && input
            .uncertain_effects
            .iter()
            .all(|id| cancellation.uncertain_effects.contains(id));
    if duplicate
        && (!input.completed_effects.is_empty()
            || !input.cancelled_effects.is_empty()
            || !input.uncertain_effects.is_empty())
    {
        return duplicate_decision(state);
    }
    let mut completed = cancellation.completed_effects.to_vec();
    let mut cancelled = cancellation.cancelled_effects.to_vec();
    let mut uncertain = cancellation.uncertain_effects.to_vec();
    completed.extend_from_slice(&input.completed_effects);
    cancelled.extend_from_slice(&input.cancelled_effects);
    uncertain.extend_from_slice(&input.uncertain_effects);
    completed.sort_unstable();
    completed.dedup();
    cancelled.sort_unstable();
    cancelled.dedup();
    uncertain.sort_unstable();
    uncertain.dedup();
    let reconciled = CancellationReconciled {
        request_id: input.request_id,
        completed_effects: completed.clone().into(),
        cancelled_effects: cancelled.clone().into(),
        uncertain_effects: uncertain.clone().into(),
    };
    let classified = completed
        .iter()
        .chain(cancelled.iter())
        .chain(uncertain.iter())
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let remaining = cancellation
        .outstanding_effects
        .iter()
        .filter(|effect_id| !classified.contains(effect_id))
        .count();
    let newly_completed = input
        .completed_effects
        .iter()
        .filter(|effect_id| !cancellation.completed_effects.contains(effect_id))
        .copied()
        .collect::<Vec<_>>();
    let newly_cancelled = input
        .cancelled_effects
        .iter()
        .filter(|effect_id| !cancellation.cancelled_effects.contains(effect_id))
        .copied()
        .collect::<Vec<_>>();
    let mut bodies = Vec::new();
    for effect_id in &newly_cancelled {
        if let Some(pending) = state
            .pending_extension_effect
            .as_ref()
            .filter(|pending| pending.requested.effect_id() == *effect_id)
        {
            bodies.push(RecordBody::EffectCancelled(
                EffectCancelled::try_new(
                    *effect_id,
                    pending.requested.output_contract().clone(),
                    Some("cancelled"),
                    Option::<&str>::None,
                )
                .map_err(|_| KernelError::InvariantViolation)?,
            ));
        }
        if let Some(pending) = state
            .pending_model_effect
            .as_ref()
            .filter(|pending| pending.requested.effect_id() == *effect_id)
        {
            bodies.push(RecordBody::EffectCancelled(
                EffectCancelled::try_new(
                    *effect_id,
                    pending.requested.output_contract().clone(),
                    Some("cancelled"),
                    Option::<&str>::None,
                )
                .map_err(|_| KernelError::InvariantViolation)?,
            ));
        }
        if let Some(pending) = state
            .pending_interaction
            .as_ref()
            .filter(|pending| pending.request.effect_id() == *effect_id)
        {
            bodies.push(RecordBody::InteractionCancelled(
                InteractionCancelled::try_new(
                    pending.request.interaction_id(),
                    None,
                    None,
                    Some("cancelled"),
                )
                .map_err(|_| KernelError::InvariantViolation)?,
            ));
            bodies.push(RecordBody::EffectCancelled(
                EffectCancelled::try_new(
                    *effect_id,
                    super::super::interaction::interaction_contract(
                        pending.request.response_schema_digest(),
                    ),
                    Some("cancelled"),
                    Some(pending.request.interaction_id().to_canonical_string()),
                )
                .map_err(|_| KernelError::InvariantViolation)?,
            ));
        }
        if let Some(pending) = state
            .retry
            .pending
            .as_ref()
            .filter(|pending| pending.timer_effect_id == *effect_id)
        {
            bodies.push(RecordBody::EffectCancelled(
                EffectCancelled::try_new(
                    pending.timer_effect_id,
                    timer_firing_contract(),
                    Some("cancelled"),
                    Option::<&str>::None,
                )
                .map_err(|_| KernelError::InvariantViolation)?,
            ));
        }
    }
    let mut tool_followups =
        super::super::tool::cancellation_followups(state, env, &newly_completed, &newly_cancelled)?;
    let first_non_cancelled = tool_followups
        .bodies
        .iter()
        .position(|body| !matches!(body, RecordBody::EffectCancelled(_)))
        .unwrap_or(tool_followups.bodies.len());
    bodies.extend(tool_followups.bodies.drain(..first_non_cancelled));
    bodies.push(RecordBody::CancellationReconciled(reconciled));
    bodies.append(&mut tool_followups.bodies);
    if !uncertain.is_empty() {
        bodies.push(RecordBody::RunSuspended(RunSuspended {
            reason_code: ErrorCode::new("cancellation_uncertain")
                .map_err(|_| KernelError::InvariantViolation)?,
            cancellation_request_id: Some(input.request_id),
        }));
    } else if remaining == 0 {
        bodies.push(RecordBody::RunCancelled(RunCancelled {
            request_id: input.request_id,
            reason_code: ErrorCode::new("cancelled")
                .map_err(|_| KernelError::InvariantViolation)?,
        }));
    }
    let event_count = bodies.iter().try_fold(0_usize, |count, body| {
        count
            .checked_add(
                body.derived_event_count(RECORD_KIND_VERSION)
                    .map_err(|_| KernelError::InvariantViolation)?,
            )
            .ok_or(KernelError::InvariantViolation)
    })?;
    if bodies.len() > APPEND_BATCH_MAX_RECORDS {
        return Err(KernelError::InvalidInputPayload {
            field: "records",
            reason_code: "too_many_items",
        });
    }
    capacity::preflight_decision(
        state,
        StateGrowth {
            messages: tool_followups.messages,
            tool_settlements: &tool_followups.settlements,
            ..StateGrowth::default()
        },
    )?;
    validate_allocated_ids(
        &env.ids,
        IdRequirements::new(bodies.len(), event_count, 0, 0, 0, tool_followups.messages),
    )?;
    decision_for(state, env, bodies, Vec::new())
}

pub(super) fn decide_timer_fired(
    state: &KernelState,
    env: &TransitionEnv,
    input: &TimerFiredInput,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    if let Some(existing) = state.retry.timer_firings.get(&input.effect_id) {
        return if existing.effect_id == input.effect_id
            && existing.due_at == input.due_at
            && existing.fired_at == input.fired_at
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if state.phase != Some(RunPhase::Sleeping) || state.cancellation.is_some() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "timer_fired",
        });
    }
    let pending = state
        .retry
        .pending
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "timer_fired",
        })?;
    if pending.timer_effect_id != input.effect_id
        || pending.due_at != input.due_at
        || input.fired_at < input.due_at
    {
        return Err(KernelError::ConflictingSettlement);
    }
    capacity::preflight_decision(
        state,
        StateGrowth {
            timer_firings: Some(input.effect_id),
            ..StateGrowth::default()
        },
    )?;
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    decision_for(
        state,
        env,
        vec![RecordBody::TimerFired(TimerFired {
            effect_id: input.effect_id,
            due_at: input.due_at,
            fired_at: input.fired_at,
        })],
        Vec::new(),
    )
}

fn validate_reconciliation_input(
    input: &CancellationReconciledInput,
    cancellation: &crate::CancellationState,
) -> Result<(), KernelError> {
    for (field, values) in [
        ("completed_effects", input.completed_effects.as_ref()),
        ("cancelled_effects", input.cancelled_effects.as_ref()),
        ("uncertain_effects", input.uncertain_effects.as_ref()),
    ] {
        if values.len() > SEMANTIC_ARRAY_MAX_ITEMS
            || values.windows(2).any(|pair| pair[0] >= pair[1])
            || values.iter().any(|id| {
                !cancellation.outstanding_effects.contains(id)
                    && !cancellation.completed_effects.contains(id)
                    && !cancellation.cancelled_effects.contains(id)
                    && !cancellation.uncertain_effects.contains(id)
            })
        {
            return Err(KernelError::InvalidInputPayload {
                field,
                reason_code: "invalid_effect_set",
            });
        }
    }
    if input
        .completed_effects
        .iter()
        .any(|id| input.cancelled_effects.contains(id) || input.uncertain_effects.contains(id))
        || input
            .cancelled_effects
            .iter()
            .any(|id| input.uncertain_effects.contains(id))
    {
        return Err(KernelError::InvalidInputPayload {
            field: "reconciliation",
            reason_code: "overlapping_effect_sets",
        });
    }
    if input.completed_effects.iter().any(|id| {
        cancellation.cancelled_effects.contains(id) || cancellation.uncertain_effects.contains(id)
    }) || input.cancelled_effects.iter().any(|id| {
        cancellation.completed_effects.contains(id) || cancellation.uncertain_effects.contains(id)
    }) || input.uncertain_effects.iter().any(|id| {
        cancellation.completed_effects.contains(id) || cancellation.cancelled_effects.contains(id)
    }) {
        return Err(KernelError::ConflictingSettlement);
    }
    Ok(())
}
