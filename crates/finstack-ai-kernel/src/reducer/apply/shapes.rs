use crate::conversation::MessageRole;
use crate::effects::{EffectInput, EffectKind, EffectOutputKind};
use crate::records::lifecycle::{
    EntryAppended, RunCompleted, RunFailed, Stage, StageCursor, StageDisposition,
    StageOutcomeRecorded,
};
use crate::records::tools::{ActiveToolCall, ActiveToolCallStatus, ToolCallPlan};
use crate::records::{RecordBody, RecordEnvelope};
use crate::state::{KernelState, PendingModelEffect, RunPhase, TerminalCandidate};

use super::super::decision::KernelError;
use super::super::failure_from_state;

#[expect(
    clippy::too_many_lines,
    reason = "the phase matrix remains exhaustive and composition records add one independent durable sidecar shape"
)]
pub(super) fn validate_batch_shape(
    state: &KernelState,
    records: &[RecordEnvelope],
) -> Result<(), KernelError> {
    if composition_record_shape(state, records)
        || structural_record_shape(records)
        || foreign_run_shape(state, records)
    {
        return Ok(());
    }
    if state.accepted.is_some()
        && matches!(
            records,
            [record] if matches!(record.body(), RecordBody::ExternalCommandRejected(_))
        )
    {
        return Ok(());
    }
    let valid = match state.phase {
        None => matches!(
            records,
            [record] if matches!(record.body(), RecordBody::RunAccepted(_))
                && record.run_id() == match record.body() {
                    RecordBody::RunAccepted(accepted) => Some(accepted.run_id()),
                    _ => None,
                }
        ),
        Some(RunPhase::BeforeRun) => {
            matches!(records, [record] if matches!(
                record.body(),
                RecordBody::OutputConfigured(_) | RecordBody::CapabilitiesActivated(_)
            )) || one_stage(records, state.cycle, Stage::BeforeRun, |disposition| {
                matches!(
                    disposition,
                    StageDisposition::Continued | StageDisposition::Failed { .. }
                )
            }) || interaction_request_shape(records)
        }
        Some(RunPhase::PreparingContext) => {
            prepare_context_shape(records, state.cycle)
                || one_failed_stage(records, state.cycle, Stage::PrepareContext)
                || interaction_request_shape(records)
        }
        Some(RunPhase::BeforeModel) => {
            request_model_shape(state, records)
                || one_failed_stage(records, state.cycle, Stage::BeforeModel)
                || interaction_request_shape(records)
        }
        Some(RunPhase::AwaitingModel) => model_settlement_shape(state, records, true),
        Some(RunPhase::AwaitingExternal) => {
            if state.pending_model_effect.is_some() {
                model_settlement_shape(state, records, false)
            } else {
                tool_settlement_shape(state, records)
            }
        }
        Some(RunPhase::AfterModel) => {
            matches!(records, [record] if matches!(
                record.body(),
                RecordBody::FinalResultRecorded(_) | RecordBody::OutputValidationFailed(_)
            )) || one_stage(records, state.cycle, Stage::AfterModel, |disposition| {
                matches!(
                    disposition,
                    StageDisposition::Continued | StageDisposition::Failed { .. }
                )
            }) || interaction_request_shape(records)
        }
        Some(RunPhase::BeforeToolBatch) => {
            tool_batch_open_shape(state, records)
                || one_failed_stage(records, state.cycle, Stage::BeforeToolBatch)
                || interaction_request_shape(records)
        }
        Some(RunPhase::AwaitingTools) => tool_settlement_shape(state, records),
        Some(RunPhase::AfterToolBatch) => {
            one_stage(records, state.cycle, Stage::AfterToolBatch, |disposition| {
                matches!(
                    disposition,
                    StageDisposition::Continued | StageDisposition::Failed { .. }
                )
            }) || interaction_request_shape(records)
        }
        Some(RunPhase::BeforeFinalize) => {
            finalize_shape(state, records)
                || retry_shape(state, records)
                || interaction_request_shape(records)
        }
        Some(RunPhase::Sleeping) => timer_fired_shape(state, records),
        Some(RunPhase::Cancelling) => reconciliation_shape(state, records),
        Some(RunPhase::AwaitingInteraction) => interaction_terminal_shape(records),
        Some(RunPhase::Accepted | RunPhase::Suspended) => false,
        Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled) => {
            return Err(KernelError::TerminalStateImmutable);
        }
    };
    let cancellation_shape = state.accepted.is_some()
        && state.cancellation.is_none()
        && matches!(records, [record] if matches!(record.body(), RecordBody::CancellationRequested(_)));
    let limit_shape = matches!(
        records,
        [limit, terminal]
            if matches!(limit.body(), RecordBody::LimitReached(_))
                && matches!(terminal.body(), RecordBody::RunFailed(_))
    );
    let control_failure_shape = matches!(
        records,
        [record] if matches!(record.body(), RecordBody::RunFailed(failed)
            if matches!(failed.error.category, crate::ErrorCategory::Limit | crate::ErrorCategory::Deadline))
            || matches!(record.body(), RecordBody::RunSuspended(_))
    );
    if valid || cancellation_shape || limit_shape || control_failure_shape {
        Ok(())
    } else {
        Err(KernelError::InvalidRecordOrder)
    }
}

pub(super) fn structural_record_shape(records: &[RecordEnvelope]) -> bool {
    !records.is_empty() && records.iter().all(|record| record.body().is_structural())
}

pub(super) fn is_foreign_run(state: &KernelState, record: &RecordEnvelope) -> bool {
    match (state.accepted.as_ref(), record.run_id()) {
        (Some(accepted), Some(run_id)) if run_id != accepted.run_id() => {
            matches!(record.body(), RecordBody::RunAccepted(_))
                || state
                    .child_preparations
                    .values()
                    .any(|prepared| prepared.child.operation.run_id == run_id)
        }
        _ => false,
    }
}

pub(super) fn foreign_run_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    !records.is_empty()
        && state.accepted.is_some()
        && records
            .iter()
            .all(|record| record.body().is_structural() || is_foreign_run(state, record))
}

pub(super) fn composition_record_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    match records {
        [prepared]
            if matches!(prepared.body(), RecordBody::ChildRunPrepared(value)
            if value.budget_reservation_id.is_none()) =>
        {
            true
        }
        [prepared, requested] => matches!(
            (prepared.body(), requested.body()),
            (
                RecordBody::ChildRunPrepared(child),
                RecordBody::BudgetReservationRequested(budget),
            ) if child.budget_reservation_id == Some(budget.request.reservation_id)
                && child.child.operation.run_id == budget.request.run_id
        ),
        [record] => match record.body() {
            RecordBody::BudgetReservationSettled(value) => state
                .budget_reservations
                .contains_key(&value.receipt.reservation_id),
            RecordBody::BudgetChargeRecorded(value) => state
                .budget_reservations
                .get(&value.receipt.reservation_id)
                .is_some_and(|reservation| reservation.settlement.is_some()),
            RecordBody::BudgetReservationReleased(value) => {
                state
                    .budget_reservations
                    .get(&value.receipt.reservation_id)
                    .is_some_and(|reservation| reservation.settlement.is_some())
                    && state
                        .accepted
                        .as_ref()
                        .is_some_and(|accepted| accepted.run_id() == value.receipt.terminal_run_id)
            }
            _ => false,
        },
        _ => false,
    }
}

pub(super) fn retry_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    let [stage_record, retry_record, requested] = records else {
        return false;
    };
    matches!(
        (stage_record.body(), retry_record.body(), requested.body()),
        (
            RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
                cursor,
                disposition: StageDisposition::RetryScheduled { attempt, timer_effect_id, due_at },
                ..
            }),
            RecordBody::RetryScheduled(scheduled),
            RecordBody::EffectRequested(effect),
        ) if *cursor == (StageCursor { cycle: state.cycle, stage: Stage::BeforeFinalize })
            && scheduled.attempt == *attempt
            && scheduled.timer_effect_id == *timer_effect_id
            && scheduled.due_at == *due_at
            && effect.effect_id() == *timer_effect_id
            && effect.kind() == EffectKind::Timer
            && matches!(effect.input(), crate::EffectInput::Timer { due_at: effect_due } if effect_due == due_at)
    )
}

pub(super) fn timer_fired_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    matches!(
        records,
        [record] if matches!(
            (state.retry.pending.as_ref(), record.body()),
            (Some(pending), RecordBody::TimerFired(fired))
                if pending.timer_effect_id == fired.effect_id && pending.due_at == fired.due_at
        )
    )
}

pub(super) fn reconciliation_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    let Some(cancellation) = state.cancellation.as_ref() else {
        return false;
    };
    let Some(reconciled_index) = records
        .iter()
        .position(|record| matches!(record.body(), RecordBody::CancellationReconciled(_)))
    else {
        return false;
    };
    if records[..reconciled_index].iter().any(|record| {
        !matches!(
            record.body(),
            RecordBody::EffectCancelled(cancelled)
                if cancellation.outstanding_effects.contains(&cancelled.effect_id())
        ) && !matches!(record.body(), RecordBody::InteractionCancelled(_))
    }) {
        return false;
    }
    if !matches!(
        records[reconciled_index].body(),
        RecordBody::CancellationReconciled(value)
            if value.request_id == cancellation.request.request_id
    ) {
        return false;
    }
    let tail = &records[reconciled_index + 1..];
    let mut closed = false;
    for (index, record) in tail.iter().enumerate() {
        match record.body() {
            RecordBody::ToolCallSettled(_) if !closed => {}
            RecordBody::ToolBatchClosed(_) if !closed => closed = true,
            RecordBody::RunSuspended(_) | RecordBody::RunCancelled(_)
                if index + 1 == tail.len() => {}
            _ => return false,
        }
    }
    true
}

pub(super) fn tool_batch_open_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    let Some((stage_record, tail)) = records.split_first() else {
        return false;
    };
    let Some((opened_record, rest)) = tail.split_first() else {
        return false;
    };
    let (
        RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
            cursor,
            disposition:
                StageDisposition::ToolBatchPrepared {
                    tool_batch_id,
                    plan_digest,
                },
            ..
        }),
        RecordBody::ToolBatchOpened(opened),
    ) = (stage_record.body(), opened_record.body())
    else {
        return false;
    };
    let first_executable_group = opened
        .calls
        .iter()
        .find_map(|call| matches!(call.plan, ToolCallPlan::Execute(_)).then_some(call.group_index));
    let requested = first_executable_group.map_or_else(Vec::new, |group| {
        opened
            .calls
            .iter()
            .filter(|call| {
                call.group_index == group && matches!(call.plan, ToolCallPlan::Execute(_))
            })
            .map(|call| call.effect_id)
            .collect::<Vec<_>>()
    });
    let leading_synthetic = opened
        .calls
        .iter()
        .take_while(|call| matches!(call.plan, ToolCallPlan::SyntheticClosure(_)))
        .collect::<Vec<_>>();
    let expected_close = usize::from(first_executable_group.is_none());
    let expected_len = requested.len() + leading_synthetic.len() + expected_close;
    let requests_match =
        rest.iter()
            .take(requested.len())
            .zip(&requested)
            .all(|(record, effect_id)| {
                matches!(
                    record.body(),
                    RecordBody::EffectRequested(value)
                        if value.kind() == EffectKind::Tool && value.effect_id() == *effect_id
                )
            });
    let settlements_match = rest
        .iter()
        .skip(requested.len())
        .take(leading_synthetic.len())
        .zip(&leading_synthetic)
        .all(|(record, call)| {
            matches!(
                record.body(),
                RecordBody::ToolCallSettled(value)
                    if value.effect_id == call.effect_id
                        && value.tool_call_id == *call.plan.call().tool_call_id()
            )
        });
    let close_matches = expected_close == 0
        || matches!(
            rest.last().map(RecordEnvelope::body),
            Some(RecordBody::ToolBatchClosed(value))
                if value.tool_batch_id == opened.tool_batch_id
        );

    *cursor
        == StageCursor {
            cycle: state.cycle,
            stage: Stage::BeforeToolBatch,
        }
        && opened.cycle == state.cycle
        && opened.tool_batch_id == *tool_batch_id
        && opened.plan_digest == *plan_digest
        && state
            .current_turn
            .as_ref()
            .is_some_and(|turn| turn.turn_id == opened.turn_id)
        && state
            .messages
            .last()
            .is_some_and(|message| *message.id() == opened.source_message_id)
        && rest.len() == expected_len
        && requests_match
        && settlements_match
        && close_matches
}

#[expect(
    clippy::too_many_lines,
    reason = "exact settlement replay mirrors source-prefix finalization, group dispatch, and closure"
)]
pub(super) fn tool_settlement_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return false;
    };
    let Some((first, rest)) = records.split_first() else {
        return false;
    };
    let (effect_id, deferred, fail_run) = match first.body() {
        RecordBody::EffectCompleted(value)
            if value.output_contract().kind == EffectOutputKind::ToolResult =>
        {
            (value.effect_id(), false, false)
        }
        RecordBody::EffectFailed(value)
            if value.output_contract().kind == EffectOutputKind::ToolResult =>
        {
            let Some(call) = batch
                .calls
                .iter()
                .find(|call| call.assigned.effect_id == value.effect_id())
            else {
                return false;
            };
            (
                value.effect_id(),
                false,
                call.assigned.plan.failure_policy() == crate::ToolFailurePolicy::FailRun,
            )
        }
        RecordBody::EffectDeferred(value)
            if value.output_contract.kind == EffectOutputKind::ToolResult =>
        {
            (value.effect_id, true, false)
        }
        _ => return false,
    };
    let Some(target_index) = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == effect_id)
    else {
        return false;
    };
    if batch.calls[target_index].assigned.group_index != batch.current_group {
        return false;
    }
    if deferred {
        return matches!(
            batch.calls[target_index].status,
            ActiveToolCallStatus::Requested { deferred: None, .. }
        ) && rest.is_empty();
    }
    if !matches!(
        batch.calls[target_index].status,
        ActiveToolCallStatus::Requested { .. }
    ) {
        return false;
    }

    let fatal = batch.fatal_error.is_some() || fail_run;
    let current_group_complete = batch.calls.iter().enumerate().all(|(index, call)| {
        call.assigned.group_index != batch.current_group
            || index == target_index
            || matches!(
                call.status,
                ActiveToolCallStatus::Buffered { .. } | ActiveToolCallStatus::Settled { .. }
            )
    });
    let abort_undispatched = fatal && current_group_complete;
    let Ok(start) = usize::try_from(batch.next_source_index) else {
        return false;
    };
    let is_virtual_buffered = |index: usize, call: &ActiveToolCall| {
        index == target_index
            || matches!(call.status, ActiveToolCallStatus::Buffered { .. })
            || (abort_undispatched && matches!(call.status, ActiveToolCallStatus::Undispatched))
    };
    let mut settlement_effects = Vec::new();
    for (index, call) in batch.calls.iter().enumerate().skip(start) {
        if is_virtual_buffered(index, call) {
            settlement_effects.push(call.assigned.effect_id);
        } else {
            break;
        }
    }
    let settlements_match = rest
        .iter()
        .take(settlement_effects.len())
        .zip(&settlement_effects)
        .all(|(record, expected)| {
            matches!(
                record.body(),
                RecordBody::ToolCallSettled(value) if value.effect_id == *expected
            )
        });
    if !settlements_match {
        return false;
    }

    let settled_end = start.saturating_add(settlement_effects.len());
    let next_group = (!fatal && current_group_complete)
        .then(|| {
            batch.calls.iter().enumerate().find_map(|(index, call)| {
                (index >= settled_end
                    && matches!(call.status, ActiveToolCallStatus::Undispatched)
                    && matches!(call.assigned.plan, ToolCallPlan::Execute(_)))
                .then_some(call.assigned.group_index)
            })
        })
        .flatten();
    let requested = next_group.map_or_else(Vec::new, |group| {
        batch
            .calls
            .iter()
            .filter(|call| {
                call.assigned.group_index == group
                    && matches!(call.status, ActiveToolCallStatus::Undispatched)
                    && matches!(call.assigned.plan, ToolCallPlan::Execute(_))
            })
            .map(|call| call.assigned.effect_id)
            .collect::<Vec<_>>()
    });
    let requests_match = rest
        .iter()
        .skip(settlement_effects.len())
        .take(requested.len())
        .zip(&requested)
        .all(|(record, expected)| {
            matches!(
                record.body(),
                RecordBody::EffectRequested(value)
                    if value.kind() == EffectKind::Tool && value.effect_id() == *expected
            )
        });
    if !requests_match {
        return false;
    }

    let all_settled = batch.calls.iter().enumerate().all(|(index, call)| {
        matches!(call.status, ActiveToolCallStatus::Settled { .. })
            || (index >= start && index < settled_end && is_virtual_buffered(index, call))
    });
    let expected_close = usize::from(all_settled);
    let expected_len = settlement_effects.len() + requested.len() + expected_close;
    let close_matches = expected_close == 0
        || matches!(
            rest.last().map(RecordEnvelope::body),
            Some(RecordBody::ToolBatchClosed(value))
                if value.tool_batch_id == batch.opened.tool_batch_id
        );
    rest.len() == expected_len && close_matches
}

pub(super) fn one_stage(
    records: &[RecordEnvelope],
    cycle: u64,
    stage: Stage,
    disposition: impl FnOnce(&StageDisposition) -> bool,
) -> bool {
    matches!(
        records,
        [record]
            if matches!(
                record.body(),
                RecordBody::StageOutcomeRecorded(outcome)
                    if outcome.cursor == StageCursor { cycle, stage }
                        && disposition(&outcome.disposition)
            )
    )
}

pub(super) fn one_failed_stage(records: &[RecordEnvelope], cycle: u64, stage: Stage) -> bool {
    one_stage(records, cycle, stage, |disposition| {
        matches!(disposition, StageDisposition::Failed { .. })
    })
}

pub(super) fn prepare_context_shape(records: &[RecordEnvelope], cycle: u64) -> bool {
    let [stage, context] = records else {
        return false;
    };
    let (
        RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
            cursor,
            disposition:
                StageDisposition::ContextPrepared {
                    turn_id,
                    context_digest,
                },
            ..
        }),
        RecordBody::ContextPrepared(context),
    ) = (stage.body(), context.body())
    else {
        return false;
    };
    *cursor
        == StageCursor {
            cycle,
            stage: Stage::PrepareContext,
        }
        && context.cycle == cycle
        && context.turn_id == *turn_id
        && context.context_digest == *context_digest
}

pub(super) fn request_model_shape(kernel_state: &KernelState, records: &[RecordEnvelope]) -> bool {
    let [outcome_record, effect_record] = records else {
        return false;
    };
    let (
        RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
            cursor,
            disposition:
                StageDisposition::ModelRequested {
                    turn_id,
                    model_request_id: _,
                    effect_id,
                },
            ..
        }),
        RecordBody::EffectRequested(requested),
    ) = (outcome_record.body(), effect_record.body())
    else {
        return false;
    };
    *cursor
        == StageCursor {
            cycle: kernel_state.cycle,
            stage: Stage::BeforeModel,
        }
        && kernel_state
            .current_turn
            .as_ref()
            .is_some_and(|current| current.turn_id == *turn_id)
        && requested.effect_id() == *effect_id
        && requested.kind() == EffectKind::Model
        && requested.relation().is_none()
        && requested.pipeline().is_none()
        && requested.output_contract().kind == EffectOutputKind::ModelResponse
        && matches!(requested.input(), crate::EffectInput::Model { .. })
}

pub(super) fn model_settlement_shape(
    state: &KernelState,
    records: &[RecordEnvelope],
    allow_deferred: bool,
) -> bool {
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return false;
    };
    // AwaitingExternal settlements require a retained EffectDeferred value so
    // source-discriminated digest reconstruction cannot flip to Direct*.
    let terminal_settlement_ok = allow_deferred || pending.deferred.is_some();
    match records {
        [record] => match record.body() {
            RecordBody::EffectDeferred(deferred) => {
                allow_deferred && deferred.validate_against(&pending.requested).is_ok()
            }
            RecordBody::EffectFailed(failed) => {
                terminal_settlement_ok && failed.validate_against(&pending.requested).is_ok()
            }
            _ => false,
        },
        [completed_record, entry_record] => {
            let (RecordBody::EffectCompleted(completed), RecordBody::EntryAppended(entry)) =
                (completed_record.body(), entry_record.body())
            else {
                return false;
            };
            terminal_settlement_ok
                && completed.validate_against(&pending.requested).is_ok()
                && entry_matches_completion(state, pending, entry, completed, entry_record)
        }
        _ => false,
    }
}

pub(super) fn entry_matches_completion(
    state: &KernelState,
    pending: &PendingModelEffect,
    entry: &EntryAppended,
    completion: &crate::EffectCompleted,
    entry_record: &RecordEnvelope,
) -> bool {
    entry.cycle == pending.cycle
        && entry.turn_id == pending.turn_id
        && entry.model_request_id == pending.model_request_id
        && entry.effect_id == pending.requested.effect_id()
        && entry.parent_message_id == state.messages.last().map(|message| *message.id())
        && entry.message.role() == MessageRole::Assistant
        && entry.message.created_at() == entry_record.timestamp()
        && entry.message.provider_ids() == completion.provider_ids()
}

pub(super) fn finalize_shape(kernel_state: &KernelState, records: &[RecordEnvelope]) -> bool {
    match records {
        [record] => matches!(
            record.body(),
            RecordBody::StageOutcomeRecorded(outcome)
                if outcome.cursor == StageCursor {
                    cycle: kernel_state.cycle,
                    stage: Stage::BeforeFinalize,
                }
                    && matches!(
                        (&outcome.disposition, &kernel_state.terminal_candidate),
                        (
                            StageDisposition::ContinueModel { next_cycle },
                            Some(TerminalCandidate::Completed { .. })
                        ) if kernel_state
                            .cycle
                            .checked_add(1)
                            .is_some_and(|expected| *next_cycle == expected)
                    )
        ),
        [outcome_record, terminal_record] => {
            let RecordBody::StageOutcomeRecorded(outcome) = outcome_record.body() else {
                return false;
            };
            if outcome.cursor
                != (StageCursor {
                    cycle: kernel_state.cycle,
                    stage: Stage::BeforeFinalize,
                })
            {
                return false;
            }
            match (&outcome.disposition, terminal_record.body()) {
                (StageDisposition::FinalizeAccepted, RecordBody::RunCompleted(completed)) => {
                    terminal_completed_matches(kernel_state, completed)
                }
                (StageDisposition::FinalizeAccepted, RecordBody::RunFailed(failed)) => {
                    terminal_failed_matches(kernel_state, failed)
                }
                (StageDisposition::Failed { error }, RecordBody::RunFailed(failed)) => {
                    failed == &failure_from_state(kernel_state, error.clone())
                }
                _ => false,
            }
        }
        _ => false,
    }
}

pub(super) fn terminal_completed_matches(state: &KernelState, completed: &RunCompleted) -> bool {
    matches!(
        state.terminal_candidate.as_ref(),
        Some(TerminalCandidate::Completed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            result_digest,
        }) if completed.cycle == *cycle
            && completed.turn_id == *turn_id
            && completed.model_request_id == *model_request_id
            && completed.effect_id == *effect_id
            && completed.result_message_id == *message_id
            && completed.result_digest == *result_digest
    )
}

pub(super) fn terminal_failed_matches(state: &KernelState, failed: &RunFailed) -> bool {
    matches!(
        state.terminal_candidate.as_ref(),
        Some(TerminalCandidate::Failed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            error,
        }) if failed.cycle == *cycle
            && failed.turn_id == *turn_id
            && failed.model_request_id == *model_request_id
            && failed.effect_id == *effect_id
            && failed.error == *error
    )
}

pub(super) fn interaction_request_shape(records: &[RecordEnvelope]) -> bool {
    if records.len() != 2 {
        return false;
    }
    let mut requested = None;
    let mut interaction = None;
    for record in records {
        match record.body() {
            RecordBody::EffectRequested(effect) if effect.kind() == EffectKind::Interaction => {
                requested = Some(effect);
            }
            RecordBody::InteractionRequested(request) => interaction = Some(request),
            _ => return false,
        }
    }
    match (requested, interaction) {
        (Some(effect), Some(request)) => {
            matches!(
                effect.input(),
                EffectInput::Interaction {
                    interaction_id,
                    request_digest
                } if *interaction_id == request.interaction_id()
                    && effect.effect_id() == request.effect_id()
                    && request.request_digest().is_ok_and(|digest| digest == *request_digest)
            )
        }
        _ => false,
    }
}

pub(super) fn interaction_terminal_shape(records: &[RecordEnvelope]) -> bool {
    matches!(
        records,
        [left, right]
            if matches!(left.body(), RecordBody::InteractionResolved(_))
                && matches!(
                    right.body(),
                    RecordBody::EffectCompleted(completed)
                        if completed.output_contract().kind == EffectOutputKind::InteractionResolution
                )
    ) || matches!(
        records,
        [left, right]
            if matches!(left.body(), RecordBody::InteractionExpired(_))
                && matches!(
                    right.body(),
                    RecordBody::EffectFailed(failed)
                        if failed.output_contract().kind == EffectOutputKind::InteractionResolution
                )
    ) || matches!(
        records,
        [left, right]
            if matches!(left.body(), RecordBody::InteractionCancelled(_))
                && matches!(
                    right.body(),
                    RecordBody::EffectCancelled(cancelled)
                        if cancelled.output_contract().kind == EffectOutputKind::InteractionResolution
                )
    )
}

pub(super) fn stage_cursor_for_phase(state: &KernelState) -> Option<crate::StageCursor> {
    let cursor_stage = match state.phase? {
        RunPhase::BeforeRun => Stage::BeforeRun,
        RunPhase::PreparingContext => Stage::PrepareContext,
        RunPhase::BeforeModel => Stage::BeforeModel,
        RunPhase::AfterModel => Stage::AfterModel,
        RunPhase::BeforeToolBatch => Stage::BeforeToolBatch,
        RunPhase::AfterToolBatch => Stage::AfterToolBatch,
        RunPhase::BeforeFinalize => Stage::BeforeFinalize,
        _ => return None,
    };
    Some(crate::StageCursor {
        cycle: state.cycle,
        stage: cursor_stage,
    })
}
