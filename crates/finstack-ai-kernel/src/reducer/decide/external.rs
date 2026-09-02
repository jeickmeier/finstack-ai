use crate::records::RecordBody;
use crate::state::{KernelState, TransitionEnv};

use super::super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::super::decision::{Decision, KernelError};
use super::decision_for;

pub(super) fn decide_external_command_rejected(
    state: &KernelState,
    env: &TransitionEnv,
    input: &crate::RecordExternalCommandRejected,
) -> Result<Decision, KernelError> {
    let accepted = state
        .accepted
        .as_ref()
        .ok_or(KernelError::InvalidRunAcceptance)?;
    let security = accepted.security();
    if state.session_id != Some(input.locator.session_id)
        || state.lane_id != Some(input.locator.lane_id)
        || accepted.run_id() != input.locator.run_id
        || security.tenant_scope() != input.locator.tenant_scope.as_ref()
        || security.principal() != &input.rejection.principal
        || security.authorization_policy_version() != input.rejection.authorization.policy_version()
        || security.authorization_decision_id() != input.rejection.authorization.decision_id()
    {
        return Err(KernelError::InvalidRunAcceptance);
    }
    match input.rejection.target {
        crate::ExternalCommandTarget::Effect(effect_id) if !known_effect(state, effect_id) => {
            return Err(KernelError::EffectNotPending { effect_id });
        }
        crate::ExternalCommandTarget::Interaction(interaction_id)
            if state
                .pending_interaction
                .as_ref()
                .is_none_or(|pending| pending.request.interaction_id() != interaction_id)
                && state
                    .last_interaction_terminal
                    .as_ref()
                    .is_none_or(|terminal| terminal.interaction_id != interaction_id) =>
        {
            return Err(KernelError::InvalidPhaseInput {
                phase: state.phase,
                input: "record_external_command_rejected",
            });
        }
        _ => {}
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    decision_for(
        state,
        env,
        vec![RecordBody::ExternalCommandRejected(input.rejection.clone())],
        Vec::new(),
    )
}

fn known_effect(state: &KernelState, effect_id: crate::EffectId) -> bool {
    state
        .pending_model_effect
        .as_ref()
        .is_some_and(|pending| pending.requested.effect_id() == effect_id)
        || state.model_settlements.contains_key(&effect_id)
        || state.tool_settlements.contains_key(&effect_id)
        || state
            .tool_calls
            .values()
            .any(|identity| identity.effect_id == Some(effect_id))
        || state
            .completion_identities
            .values()
            .any(|identity| identity.effect_id == effect_id)
        || state
            .pending_interaction
            .as_ref()
            .is_some_and(|pending| pending.request.effect_id() == effect_id)
}
