use std::sync::Arc;

use crate::effects::{EffectInput, EffectOutputKind};
use crate::records::RecordBody;
use crate::state::{
    CompletionIdentity, InteractionTerminalOutcome, KernelState, PendingInteraction,
    ResolutionIdentity, RunPhase,
};

use super::super::decision::KernelError;
use super::shapes::stage_cursor_for_phase;

pub(super) fn apply_interaction_requested(
    state: &mut KernelState,
    request: &crate::InteractionRequest,
) -> Result<(), KernelError> {
    state.state_version = state.state_version.max(6);
    if let Some(pending) = &state.pending_interaction {
        return if pending.request.interaction_id() == request.interaction_id()
            && pending.request.effect_id() == request.effect_id()
        {
            Ok(())
        } else {
            Err(KernelError::InvalidRecordOrder)
        };
    }
    let prior_phase = state.phase.ok_or(KernelError::InvalidRecordOrder)?;
    let cursor = stage_cursor_for_phase(state).ok_or(KernelError::InvalidRecordOrder)?;
    state.pending_interaction = Some(PendingInteraction {
        request: request.clone(),
        prior_phase,
        cursor,
    });
    Ok(())
}

pub(super) fn apply_interaction_effect_requested(
    state: &mut KernelState,
    requested: &crate::EffectRequested,
    next: Option<&RecordBody>,
) -> Result<(), KernelError> {
    let request = match next {
        Some(RecordBody::InteractionRequested(request)) => request.clone(),
        _ => state
            .pending_interaction
            .as_ref()
            .map(|pending| pending.request.clone())
            .ok_or(KernelError::InvalidRecordOrder)?,
    };
    let request_digest = request
        .request_digest()
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    if requested.effect_id() != request.effect_id()
        || !matches!(
            requested.input(),
            EffectInput::Interaction {
                interaction_id,
                request_digest: digest
            } if *interaction_id == request.interaction_id() && *digest == request_digest
        )
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    if state.pending_interaction.is_none() {
        apply_interaction_requested(state, &request)?;
    }
    state.phase = Some(RunPhase::AwaitingInteraction);
    state.state_version = state.state_version.max(6);
    Ok(())
}

pub(super) fn apply_interaction_resolved(
    state: &mut KernelState,
    resolved: &crate::InteractionResolution,
) -> Result<(), KernelError> {
    let pending = state
        .pending_interaction
        .as_ref()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if pending.request.interaction_id() != resolved.interaction_id() {
        return Err(KernelError::InvalidRecordOrder);
    }
    let digest = super::super::interaction::resolution_digest(resolved)
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    let outcome =
        super::super::interaction::approval_outcome(pending.request.kind(), resolved.response())
            .map_err(|_| KernelError::InvalidRecordOrder)?;
    state.resolution_identities.insert(
        Arc::from(resolved.resolution_id()),
        ResolutionIdentity {
            interaction_id: resolved.interaction_id(),
            settlement_digest: digest,
        },
    );
    apply_interaction_terminal_outcome(state, resolved.interaction_id(), outcome)
}

/// Record the terminal outcome of the pending interaction identified by
/// `interaction_id`; the effect record that follows restores the phase.
pub(super) fn apply_interaction_terminal_outcome(
    state: &mut KernelState,
    interaction_id: crate::InteractionId,
    outcome: InteractionTerminalOutcome,
) -> Result<(), KernelError> {
    let pending = state
        .pending_interaction
        .as_ref()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if pending.request.interaction_id() != interaction_id {
        return Err(KernelError::InvalidRecordOrder);
    }
    state.last_interaction_terminal = Some(super::super::interaction::terminal_from_pending(
        pending, outcome,
    ));
    state.state_version = state.state_version.max(6);
    Ok(())
}

pub(super) fn apply_interaction_effect_terminal(
    state: &mut KernelState,
    effect_id: crate::EffectId,
    completion_id: Option<&str>,
    settlement_digest: crate::Digest,
) -> Result<(), KernelError> {
    let pending = state
        .pending_interaction
        .take()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if pending.request.effect_id() != effect_id {
        state.pending_interaction = Some(pending);
        return Err(KernelError::InvalidRecordOrder);
    }
    if let Some(completion_id) = completion_id {
        state.completion_identities.insert(
            Arc::from(completion_id),
            CompletionIdentity {
                effect_id,
                settlement_digest,
            },
        );
    }
    if state.cancellation.is_some() {
        if !matches!(
            state.phase,
            Some(RunPhase::Cancelling | RunPhase::Suspended | RunPhase::Cancelled)
        ) {
            state.phase = Some(RunPhase::Cancelling);
        }
    } else {
        state.phase = Some(pending.prior_phase);
    }
    Ok(())
}

pub(super) fn apply_timer_effect_cancelled(
    state: &mut KernelState,
    cancelled: &crate::EffectCancelled,
) -> Result<(), KernelError> {
    let pending = state
        .retry
        .pending
        .take()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if pending.timer_effect_id != cancelled.effect_id()
        || cancelled.output_contract().kind != EffectOutputKind::TimerFiring
    {
        state.retry.pending = Some(pending);
        return Err(KernelError::InvalidRecordOrder);
    }
    state.state_version = state.state_version.max(3);
    Ok(())
}
