use std::collections::BTreeSet;
use std::sync::Arc;

use crate::effects::EffectOutputKind;
use crate::records::tools::{ActiveToolCallStatus, ToolSettlementKind};
use crate::records::{RecordBody, RecordEnvelope};
use crate::state::{
    BudgetReservationReplay, CancellationState, CurrentTurn, KernelState, RunPhase, TerminalState,
};

use super::super::decision::KernelError;
use super::effects::{
    apply_completed_usage, apply_effect_completed, apply_effect_deferred, apply_effect_failed,
    apply_effect_requested, apply_entry_appended, apply_limit_reached, update_wall_usage,
};
use super::interactions::{
    apply_interaction_cancelled, apply_interaction_effect_terminal, apply_interaction_expired,
    apply_interaction_requested, apply_interaction_resolved, apply_timer_effect_cancelled,
};
use super::output::{apply_final_result, apply_validation_failure};
use super::shapes::is_foreign_run;
use super::stage::apply_stage_outcome;
use super::tools::{
    apply_tool_batch_closed, apply_tool_batch_opened, apply_tool_call_settled, insert_tool_identity,
};

#[expect(
    clippy::too_many_lines,
    reason = "the replay dispatcher exhaustively covers the frozen record vocabulary"
)]
pub(super) fn apply_record(
    state: &mut KernelState,
    record: &RecordEnvelope,
    next: Option<&RecordBody>,
) -> Result<(), KernelError> {
    if is_foreign_run(state, record) {
        return Ok(());
    }
    if !matches!(
        record.body(),
        RecordBody::ExternalCommandRejected(_)
            | RecordBody::SessionCreated(_)
            | RecordBody::LaneCreated(_)
            | RecordBody::LaneMoved(_)
            | RecordBody::SnapshotWritten(_)
            | RecordBody::ConversationEntry(_)
    ) {
        update_wall_usage(state, record.timestamp())?;
    }
    match record.body() {
        RecordBody::RunAccepted(accepted) => {
            state.session_id = Some(record.session_id());
            state.lane_id = Some(record.lane_id());
            state.accepted = Some(accepted.clone());
            state.accepted_at = Some(record.timestamp());
            state.phase = Some(RunPhase::Accepted);
        }
        RecordBody::StageOutcomeRecorded(outcome) => apply_stage_outcome(state, outcome)?,
        RecordBody::ContextPrepared(context) => {
            // Verifying the digest and measuring the context are the same
            // canonicalization; doing them separately walked the whole
            // conversation twice per turn.
            let (digest, context_bytes) =
                crate::records::lifecycle::context_digest_and_len(&context.messages)
                    .map_err(|_| KernelError::ContextDigestMismatch)?;
            if digest != context.context_digest {
                return Err(KernelError::ContextDigestMismatch);
            }
            state.current_turn = Some(CurrentTurn {
                cycle: context.cycle,
                turn_id: context.turn_id,
                context: context.clone(),
                model_request_id: None,
                effect_id: None,
                final_message_id: None,
            });
            state.phase = Some(RunPhase::BeforeModel);
            state.limit_usage.turns = state
                .limit_usage
                .turns
                .checked_add(1)
                .ok_or(KernelError::InvalidRecordOrder)?;
            state.limit_usage.context_bytes = state
                .limit_usage
                .context_bytes
                .checked_add(
                    u64::try_from(context_bytes).map_err(|_| KernelError::InvalidRecordOrder)?,
                )
                .ok_or(KernelError::InvalidRecordOrder)?;
        }
        RecordBody::EffectRequested(requested) => {
            apply_effect_requested(state, requested, next)?;
        }
        RecordBody::EffectDeferred(deferred) => {
            apply_effect_deferred(state, deferred)?;
        }
        RecordBody::EffectCompleted(completed) => {
            apply_completed_usage(state, completed)?;
            apply_effect_completed(state, completed, next)?;
        }
        RecordBody::EntryAppended(entry) => {
            apply_entry_appended(state, entry)?;
        }
        RecordBody::ToolBatchOpened(opened) => {
            state.limit_usage.tool_calls = state
                .limit_usage
                .tool_calls
                .checked_add(
                    u64::try_from(opened.calls.len())
                        .map_err(|_| KernelError::InvalidRecordOrder)?,
                )
                .ok_or(KernelError::InvalidRecordOrder)?;
            let mut group_counts = std::collections::BTreeMap::<u32, u32>::new();
            for call in opened.calls.iter() {
                let count = group_counts.entry(call.group_index).or_default();
                *count = count
                    .checked_add(1)
                    .ok_or(KernelError::InvalidRecordOrder)?;
            }
            state.limit_usage.max_parallel_tools = state
                .limit_usage
                .max_parallel_tools
                .max(group_counts.values().copied().max().unwrap_or(0));
            apply_tool_batch_opened(state, opened)?;
        }
        RecordBody::ToolCallSettled(settled) => {
            apply_tool_call_settled(state, settled, record)?;
        }
        RecordBody::ToolBatchClosed(closed) => {
            apply_tool_batch_closed(state, closed)?;
        }
        RecordBody::EffectFailed(failed) => {
            apply_effect_failed(state, failed)?;
        }
        RecordBody::RunCompleted(completed) => {
            state.terminal = Some(TerminalState::Completed(completed.clone()));
            state.phase = Some(RunPhase::Completed);
        }
        RecordBody::RunFailed(failed) => {
            state.terminal = Some(TerminalState::Failed(failed.clone()));
            state.phase = Some(RunPhase::Failed);
            if matches!(
                failed.error.category,
                crate::ErrorCategory::Limit | crate::ErrorCategory::Deadline
            ) {
                state.state_version = state.state_version.max(3);
            }
        }
        RecordBody::CancellationRequested(requested) => {
            let outstanding = super::super::decide::outstanding_requested_effects(state);
            state.cancellation = Some(CancellationState {
                request: requested.request.clone(),
                prior_phase: state.phase.ok_or(KernelError::InvalidRecordOrder)?,
                completed_effects: Arc::from([]),
                cancelled_effects: Arc::from([]),
                uncertain_effects: Arc::from([]),
                outstanding_effects: outstanding.into(),
            });
            state.phase = Some(RunPhase::Cancelling);
            state.state_version = state.state_version.max(3);
        }
        RecordBody::CancellationReconciled(reconciled) => {
            let cancellation = state
                .cancellation
                .as_mut()
                .ok_or(KernelError::InvalidRecordOrder)?;
            if cancellation.request.request_id != reconciled.request_id {
                return Err(KernelError::InvalidRecordOrder);
            }
            cancellation.completed_effects = reconciled.completed_effects.clone();
            cancellation.cancelled_effects = reconciled.cancelled_effects.clone();
            cancellation.uncertain_effects = reconciled.uncertain_effects.clone();
            let classified: BTreeSet<_> = reconciled
                .completed_effects
                .iter()
                .chain(reconciled.cancelled_effects.iter())
                .chain(reconciled.uncertain_effects.iter())
                .copied()
                .collect();
            cancellation.outstanding_effects = cancellation
                .outstanding_effects
                .iter()
                .filter(|effect_id| !classified.contains(effect_id))
                .copied()
                .collect::<Vec<_>>()
                .into();
            if let Some(pending) = &state.pending_model_effect
                && reconciled
                    .completed_effects
                    .contains(&pending.requested.effect_id())
            {
                state.pending_model_effect = None;
            }
            if let Some(batch) = state.active_tool_batch.as_mut() {
                super::super::tool::buffer_reconciled_tool_closures(
                    batch,
                    &reconciled.completed_effects,
                )?;
            }
            let cancellation_fingerprints = state
                .active_tool_batch
                .as_ref()
                .map(|batch| {
                    batch
                        .calls
                        .iter()
                        .filter_map(|call| match &call.status {
                            ActiveToolCallStatus::Buffered {
                                settlement_digest,
                                synthetic: true,
                                error: Some(error),
                                ..
                            } if error.code.as_str() == "cancelled" => {
                                Some((call.assigned.effect_id, *settlement_digest))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for (effect_id, digest) in cancellation_fingerprints {
                insert_tool_identity(
                    state,
                    effect_id,
                    ToolSettlementKind::Synthetic,
                    digest,
                    None,
                )?;
            }
            state.state_version = state.state_version.max(3);
        }
        RecordBody::RetryScheduled(retry) => {
            state.retry.attempts = retry.attempt;
            state.retry.pending = Some(retry.clone());
            state.limit_usage.retries = retry.attempt;
            state.phase = Some(RunPhase::Sleeping);
            state.state_version = state.state_version.max(3);
        }
        RecordBody::TimerFired(fired) => {
            let pending = state
                .retry
                .pending
                .take()
                .ok_or(KernelError::InvalidRecordOrder)?;
            if pending.timer_effect_id != fired.effect_id
                || pending.due_at != fired.due_at
                || fired.fired_at < fired.due_at
            {
                return Err(KernelError::InvalidRecordOrder);
            }
            state
                .retry
                .timer_firings
                .insert(fired.effect_id, fired.clone());
            state.cycle = state
                .cycle
                .checked_add(1)
                .ok_or(KernelError::CycleOverflow)?;
            state.current_turn = None;
            state.pending_model_effect = None;
            state.terminal_candidate = None;
            state.final_result = None;
            state.validation_failure = None;
            state.phase = Some(RunPhase::PreparingContext);
            state.state_version = state.state_version.max(3);
        }
        RecordBody::LimitReached(limit) => {
            apply_limit_reached(state, limit)?;
            state.last_limit = Some(limit.clone());
            state.state_version = state.state_version.max(3);
        }
        RecordBody::RunSuspended(suspended) => {
            state.suspension = Some(suspended.clone());
            state.phase = Some(RunPhase::Suspended);
            state.state_version = state.state_version.max(3);
        }
        RecordBody::RunCancelled(cancelled) => {
            state.terminal = Some(TerminalState::Cancelled(cancelled.clone()));
            state.phase = Some(RunPhase::Cancelled);
            state.state_version = state.state_version.max(3);
        }
        RecordBody::OutputConfigured(configuration) => {
            if configuration.validate().is_err() || state.output_configuration.is_some() {
                return Err(KernelError::InvalidRecordOrder);
            }
            state.output_configuration = Some(configuration.clone());
            state.state_version = state.state_version.max(4);
        }
        RecordBody::CapabilitiesActivated(activation) => {
            activation
                .validate()
                .map_err(|_| KernelError::InvalidRecordOrder)?;
            if activation.prior_plan_digest != state.resolved_plan_digest {
                return Err(KernelError::InvalidRecordOrder);
            }
            state.active_capabilities = activation.active.clone();
            state.resolved_plan_digest = Some(activation.resolved_plan_digest);
            state.state_version = state.state_version.max(4);
        }
        RecordBody::FinalResultRecorded(result) => apply_final_result(state, result)?,
        RecordBody::OutputValidationFailed(failure) => {
            apply_validation_failure(state, failure)?;
        }
        RecordBody::ExternalCommandRejected(_)
        | RecordBody::SessionCreated(_)
        | RecordBody::LaneCreated(_)
        | RecordBody::LaneMoved(_)
        | RecordBody::SnapshotWritten(_)
        | RecordBody::ConversationEntry(_) => {}
        RecordBody::ChildRunPrepared(prepared) => {
            let accepted = state
                .accepted
                .as_ref()
                .ok_or(KernelError::InvalidRecordOrder)?;
            let parent_session_id = state.session_id.ok_or(KernelError::InvalidRecordOrder)?;
            let same_session = prepared.child.operation.session_id == parent_session_id;
            let placement_matches = match prepared.placement {
                crate::ChildPlacement::CompatibleLaneInParentSession => {
                    same_session
                        && state
                            .lane_id
                            .is_some_and(|lane_id| prepared.child.operation.lane_id != lane_id)
                }
                crate::ChildPlacement::IsolatedChildSession
                | crate::ChildPlacement::RemoteChildSession => !same_session,
            };
            if prepared.parent_run_id != accepted.run_id()
                || prepared
                    .validate(accepted.security().tenant_scope())
                    .is_err()
                || !placement_matches
            {
                return Err(KernelError::InvalidRecordOrder);
            }
            match state.child_preparations.get(&prepared.parent_effect_id) {
                Some(existing) if existing == prepared => {}
                Some(_) => return Err(KernelError::InvalidRecordOrder),
                None => {
                    state
                        .child_preparations
                        .insert(prepared.parent_effect_id, prepared.clone());
                }
            }
            state.state_version = state.state_version.max(5);
        }
        RecordBody::BudgetReservationRequested(requested) => {
            requested
                .request
                .validate()
                .map_err(|_| KernelError::InvalidRecordOrder)?;
            let linked = state.child_preparations.values().any(|prepared| {
                prepared.budget_reservation_id == Some(requested.request.reservation_id)
                    && prepared.child.operation.run_id == requested.request.run_id
            });
            if !linked {
                return Err(KernelError::InvalidRecordOrder);
            }
            match state
                .budget_reservations
                .get(&requested.request.reservation_id)
            {
                Some(existing) if existing.request == requested.request => {}
                Some(_) => return Err(KernelError::InvalidRecordOrder),
                None => {
                    state.budget_reservations.insert(
                        requested.request.reservation_id,
                        BudgetReservationReplay {
                            request: requested.request.clone(),
                            settlement: None,
                            release: None,
                        },
                    );
                }
            }
            state.state_version = state.state_version.max(5);
        }
        RecordBody::BudgetReservationSettled(settled) => {
            settled
                .receipt
                .validate()
                .map_err(|_| KernelError::InvalidRecordOrder)?;
            let replay = state
                .budget_reservations
                .get_mut(&settled.receipt.reservation_id)
                .ok_or(KernelError::InvalidRecordOrder)?;
            if settled.receipt.scope_id != replay.request.scope_id
                || settled.receipt.request_digest != replay.request.request_digest
                || settled.receipt.reserved != replay.request.amount
            {
                return Err(KernelError::InvalidRecordOrder);
            }
            match replay.settlement.as_ref() {
                Some(existing) if existing == &settled.receipt => {}
                Some(_) => return Err(KernelError::InvalidRecordOrder),
                None => replay.settlement = Some(settled.receipt.clone()),
            }
            state.state_version = state.state_version.max(5);
        }
        RecordBody::BudgetChargeRecorded(charged) => {
            charged
                .receipt
                .validate()
                .map_err(|_| KernelError::InvalidRecordOrder)?;
            let replay = state
                .budget_reservations
                .get(&charged.receipt.reservation_id)
                .ok_or(KernelError::InvalidRecordOrder)?;
            if replay.settlement.is_none() || replay.request.scope_id != charged.receipt.scope_id {
                return Err(KernelError::InvalidRecordOrder);
            }
            match state.budget_charges.get(&charged.receipt.effect_id) {
                Some(existing) if existing == &charged.receipt => {}
                Some(_) => return Err(KernelError::InvalidRecordOrder),
                None => {
                    state
                        .budget_charges
                        .insert(charged.receipt.effect_id, charged.receipt.clone());
                }
            }
            state.state_version = state.state_version.max(5);
        }
        RecordBody::BudgetReservationReleased(released) => {
            released
                .receipt
                .validate()
                .map_err(|_| KernelError::InvalidRecordOrder)?;
            let replay = state
                .budget_reservations
                .get_mut(&released.receipt.reservation_id)
                .ok_or(KernelError::InvalidRecordOrder)?;
            if replay.settlement.is_none()
                || replay.request.scope_id != released.receipt.scope_id
                || state
                    .accepted
                    .as_ref()
                    .is_none_or(|accepted| accepted.run_id() != released.receipt.terminal_run_id)
            {
                return Err(KernelError::InvalidRecordOrder);
            }
            match replay.release.as_ref() {
                Some(existing) if existing == &released.receipt => {}
                Some(_) => return Err(KernelError::InvalidRecordOrder),
                None => replay.release = Some(released.receipt.clone()),
            }
            state.state_version = state.state_version.max(5);
        }
        RecordBody::EffectCancelled(cancelled) => {
            if cancelled.output_contract().kind == EffectOutputKind::InteractionResolution {
                apply_interaction_effect_terminal(
                    state,
                    cancelled.effect_id(),
                    cancelled.completion_id(),
                    crate::Digest::raw_json(b"interaction-effect-cancelled"),
                )?;
                return Ok(());
            }
            if cancelled.output_contract().kind == EffectOutputKind::TimerFiring {
                apply_timer_effect_cancelled(state, cancelled)?;
                return Ok(());
            }
            if let Some(pending) = state.pending_model_effect.as_ref()
                && pending.requested.effect_id() == cancelled.effect_id()
            {
                cancelled
                    .validate_against(&pending.requested)
                    .map_err(|_| KernelError::InvalidRecordOrder)?;
                state.pending_model_effect = None;
            } else {
                let batch = state
                    .active_tool_batch
                    .as_mut()
                    .ok_or(KernelError::InvalidRecordOrder)?;
                let requested = batch
                    .calls
                    .iter()
                    .find_map(|call| match &call.status {
                        ActiveToolCallStatus::Requested { requested, .. }
                            if requested.effect_id() == cancelled.effect_id() =>
                        {
                            Some(requested)
                        }
                        _ => None,
                    })
                    .ok_or(KernelError::InvalidRecordOrder)?;
                cancelled
                    .validate_against(requested)
                    .map_err(|_| KernelError::InvalidRecordOrder)?;
                if !super::super::tool::buffer_cancelled_effect(batch, cancelled.effect_id())? {
                    return Err(KernelError::InvalidRecordOrder);
                }
                let digest = batch
                    .calls
                    .iter()
                    .find_map(|call| match &call.status {
                        ActiveToolCallStatus::Buffered {
                            settlement_digest, ..
                        } if call.assigned.effect_id == cancelled.effect_id() => {
                            Some(*settlement_digest)
                        }
                        _ => None,
                    })
                    .ok_or(KernelError::InvalidRecordOrder)?;
                insert_tool_identity(
                    state,
                    cancelled.effect_id(),
                    ToolSettlementKind::Synthetic,
                    digest,
                    None,
                )?;
            }
            state.state_version = state.state_version.max(3);
        }
        RecordBody::InteractionRequested(request) => {
            apply_interaction_requested(state, request)?;
        }
        RecordBody::InteractionResolved(resolved) => {
            apply_interaction_resolved(state, resolved)?;
        }
        RecordBody::InteractionExpired(expired) => {
            apply_interaction_expired(state, expired)?;
        }
        RecordBody::InteractionCancelled(cancelled) => {
            apply_interaction_cancelled(state, cancelled)?;
        }
    }
    Ok(())
}
