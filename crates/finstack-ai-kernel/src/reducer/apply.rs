//! Transactional committed-record validation and state derivation.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::capacity;
use super::decision::{CommittedBatch, KernelError};
use super::failure_from_state;
use super::fingerprint;
use super::fingerprint::{
    completed_record_digest, completed_tool_record_digest, failed_record_digest,
    failed_tool_record_digest, opened_tool_batch_plan_digest, synthetic_tool_digest,
    tool_batch_close_digest,
};
use crate::content::ContentBlock;
use crate::digest::Digest;
use crate::effects::{EffectInput, EffectKind, EffectOutputKind};
use crate::entries::{
    EntryAppended, RunCompleted, RunFailed, Stage, StageCursor, StageDisposition,
    StageOutcomeRecorded,
};
use crate::events::{EventCorrelations, RunEvent};
use crate::message::MessageRole;
use crate::records::{APPEND_BATCH_MAX_RECORDS, RecordBody, RecordEnvelope};
use crate::state::{
    BudgetReservationReplay, CancellationState, CompletionIdentity, CurrentTurn,
    InteractionTerminalOutcome, KernelState, ModelSettlementFingerprint, ModelSettlementKind,
    PendingInteraction, PendingModelEffect, ResolutionIdentity, RunPhase, TerminalCandidate,
    TerminalState,
};
use crate::tools::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, ToolBatchOutcome, ToolCallIdentity,
    ToolCallPlan, ToolSettlementFingerprint, ToolSettlementKind,
};

struct AppliedRecords {
    state: KernelState,
    event_correlations: Vec<EventCorrelations>,
}

pub(super) fn apply(
    original: &KernelState,
    committed: &CommittedBatch,
    first_transient_sequence: u64,
) -> Result<(KernelState, Arc<[RunEvent]>), KernelError> {
    validate_batch_range(original, committed)?;
    validate_record_sequences(committed)?;
    let external_rejection_only = matches!(
        committed.records.as_ref(),
        [record] if matches!(record.body(), RecordBody::ExternalCommandRejected(_))
    );
    let structural_only = !committed.records.is_empty()
        && committed
            .records
            .iter()
            .all(|record| record.body().is_structural());
    let post_terminal_budget_release = committed
        .records
        .iter()
        .all(|record| matches!(record.body(), RecordBody::BudgetReservationReleased(_)));
    let foreign_only = foreign_run_shape(original, &committed.records);
    if !external_rejection_only
        && !structural_only
        && !foreign_only
        && !post_terminal_budget_release
        && (original.terminal.is_some()
            || matches!(
                original.phase,
                Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled)
            ))
    {
        return Err(KernelError::TerminalStateImmutable);
    }
    validate_identities(original, &committed.records)?;
    validate_batch_shape(original, &committed.records)?;
    validate_stage_digests(&committed.records)?;
    validate_tool_digests(&committed.records)?;
    capacity::preflight_batch(original, &committed.records)?;
    let applied = apply_semantic_records(original, &committed.records)?;
    applied
        .state
        .validate()
        .map_err(|_| KernelError::InvalidRecordOrder)?;

    let mut events = Vec::new();
    for (record, correlations) in committed.records.iter().zip(applied.event_correlations) {
        for ordinal in 0..record.derived_event_ids().len() {
            let sequence_offset =
                u64::try_from(events.len()).map_err(|_| KernelError::InvalidInputPayload {
                    field: "first_transient_sequence",
                    reason_code: "overflow",
                })?;
            let transient_sequence = first_transient_sequence
                .checked_add(sequence_offset)
                .ok_or(KernelError::InvalidInputPayload {
                    field: "first_transient_sequence",
                    reason_code: "overflow",
                })?;
            events.push(
                RunEvent::try_from_record_with_correlations(
                    record,
                    ordinal,
                    transient_sequence,
                    correlations,
                )
                .map_err(|_| KernelError::InvalidInputPayload {
                    field: "derived_event",
                    reason_code: "record_event_projection_failed",
                })?,
            );
        }
    }
    Ok((applied.state, events.into()))
}

fn apply_semantic_records(
    original: &KernelState,
    records: &[RecordEnvelope],
) -> Result<AppliedRecords, KernelError> {
    let mut state = original.clone();
    let mut event_correlations = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        event_correlations.push(event_correlations_for(&state, record.body()));
        let next = records.get(index + 1).map(RecordEnvelope::body);
        apply_record(&mut state, record, next)?;
        state.last_applied_sequence = record.sequence();
    }
    if state.phase == Some(RunPhase::Accepted) {
        state.phase = Some(RunPhase::BeforeRun);
    }
    Ok(AppliedRecords {
        state,
        event_correlations,
    })
}

fn event_correlations_for(state: &KernelState, body: &RecordBody) -> EventCorrelations {
    let model_turn = state
        .pending_model_effect
        .as_ref()
        .map(|pending| pending.turn_id)
        .or_else(|| state.current_turn.as_ref().map(|turn| turn.turn_id));
    let model_request = state
        .pending_model_effect
        .as_ref()
        .map(|pending| pending.model_request_id)
        .or_else(|| {
            state
                .current_turn
                .as_ref()
                .and_then(|turn| turn.model_request_id)
        });
    let effect_id = match body {
        RecordBody::EffectRequested(value) => Some(value.effect_id()),
        RecordBody::EffectDeferred(value) => Some(value.effect_id),
        RecordBody::EffectCompleted(value) => Some(value.effect_id()),
        RecordBody::EffectFailed(value) => Some(value.effect_id()),
        RecordBody::EffectCancelled(value) => Some(value.effect_id()),
        RecordBody::ToolCallSettled(value) => Some(value.effect_id),
        _ => None,
    };
    let tool = state.active_tool_batch.as_ref().and_then(|batch| {
        let effect_id = effect_id?;
        batch
            .calls
            .iter()
            .find(|call| call.assigned.effect_id == effect_id)
            .map(|call| {
                (
                    batch.opened.turn_id,
                    batch.opened.tool_batch_id,
                    *call.assigned.plan.call().tool_call_id(),
                )
            })
    });
    EventCorrelations {
        model_turn,
        model_request,
        tool_turn: tool.map(|value| value.0),
        tool_batch: tool.map(|value| value.1),
        tool_call: tool.map(|value| value.2),
    }
}

fn validate_stage_digests(records: &[RecordEnvelope]) -> Result<(), KernelError> {
    for (index, record) in records.iter().enumerate() {
        if let RecordBody::StageOutcomeRecorded(outcome) = record.body() {
            let reconstructed = fingerprint::stage_record_digest(outcome, records.get(index + 1))?;
            if reconstructed != outcome.settlement_digest {
                return Err(KernelError::SettlementDigestMismatch);
            }
        }
    }
    Ok(())
}

fn validate_tool_digests(records: &[RecordEnvelope]) -> Result<(), KernelError> {
    for record in records {
        match record.body() {
            RecordBody::ToolBatchOpened(opened)
                if opened_tool_batch_plan_digest(opened)? != opened.plan_digest =>
            {
                return Err(KernelError::SettlementDigestMismatch);
            }
            RecordBody::ToolBatchClosed(closed)
                if tool_batch_close_digest(
                    closed.cycle,
                    closed.turn_id,
                    closed.tool_batch_id,
                    closed.source_message_id,
                    &closed.result_message_ids,
                    &closed.outcome,
                )? != closed.close_digest =>
            {
                return Err(KernelError::SettlementDigestMismatch);
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_batch_range(
    state: &KernelState,
    committed: &CommittedBatch,
) -> Result<(), KernelError> {
    if committed.records.len() > APPEND_BATCH_MAX_RECORDS {
        return Err(KernelError::InvalidInputPayload {
            field: "records",
            reason_code: "too_many_items",
        });
    }
    if committed.records.is_empty() {
        return Err(KernelError::CommittedBatchRangeMismatch);
    }
    let expected_first = state
        .last_applied_sequence
        .checked_add(1)
        .ok_or(KernelError::CommittedBatchRangeMismatch)?;
    let span = u64::try_from(committed.records.len())
        .map_err(|_| KernelError::CommittedBatchRangeMismatch)?
        .checked_sub(1)
        .ok_or(KernelError::CommittedBatchRangeMismatch)?;
    let expected_last = committed
        .first_sequence
        .checked_add(span)
        .ok_or(KernelError::CommittedBatchRangeMismatch)?;
    if committed.first_sequence != expected_first || committed.last_sequence != expected_last {
        return Err(KernelError::CommittedBatchRangeMismatch);
    }
    Ok(())
}

fn validate_record_sequences(committed: &CommittedBatch) -> Result<(), KernelError> {
    for (index, record) in committed.records.iter().enumerate() {
        let offset = u64::try_from(index).map_err(|_| KernelError::NonContiguousRecordSequence)?;
        let expected = committed
            .first_sequence
            .checked_add(offset)
            .ok_or(KernelError::NonContiguousRecordSequence)?;
        if record.sequence() != expected {
            return Err(KernelError::NonContiguousRecordSequence);
        }
    }
    Ok(())
}

fn validate_identities(state: &KernelState, records: &[RecordEnvelope]) -> Result<(), KernelError> {
    let mut record_ids = BTreeSet::new();
    let mut event_ids = BTreeSet::new();
    let first_timestamp = records
        .first()
        .map(RecordEnvelope::timestamp)
        .ok_or(KernelError::CommittedBatchRangeMismatch)?;
    for record in records {
        if !record_ids.insert(record.record_id())
            || record
                .derived_event_ids()
                .iter()
                .any(|event_id| !event_ids.insert(*event_id))
            || record.timestamp() != first_timestamp
        {
            return Err(KernelError::RecordIdentityMismatch);
        }
        if record.body().is_structural() {
            if let Some(session_id) = state.session_id
                && record.session_id() != session_id
            {
                return Err(KernelError::RecordIdentityMismatch);
            }
            continue;
        }
        if let Some(session_id) = state.session_id
            && record.session_id() != session_id
        {
            return Err(KernelError::RecordIdentityMismatch);
        }
        if is_foreign_run(state, record) {
            continue;
        }
        if let (Some(lane_id), Some(accepted)) = (state.lane_id, state.accepted.as_ref())
            && (record.lane_id() != lane_id || record.run_id() != Some(accepted.run_id()))
        {
            return Err(KernelError::RecordIdentityMismatch);
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "the phase matrix remains exhaustive and composition records add one independent durable sidecar shape"
)]
fn validate_batch_shape(
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
                tool_settlement_shape(state, records, false)
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
        Some(RunPhase::AwaitingTools) => tool_settlement_shape(state, records, true),
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

fn structural_record_shape(records: &[RecordEnvelope]) -> bool {
    !records.is_empty() && records.iter().all(|record| record.body().is_structural())
}

fn is_foreign_run(state: &KernelState, record: &RecordEnvelope) -> bool {
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

fn foreign_run_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    !records.is_empty()
        && state.accepted.is_some()
        && records
            .iter()
            .all(|record| record.body().is_structural() || is_foreign_run(state, record))
}

fn composition_record_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
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

fn retry_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
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

fn timer_fired_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
    matches!(
        records,
        [record] if matches!(
            (state.retry.pending.as_ref(), record.body()),
            (Some(pending), RecordBody::TimerFired(fired))
                if pending.timer_effect_id == fired.effect_id && pending.due_at == fired.due_at
        )
    )
}

fn reconciliation_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
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

fn tool_batch_open_shape(state: &KernelState, records: &[RecordEnvelope]) -> bool {
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
fn tool_settlement_shape(
    state: &KernelState,
    records: &[RecordEnvelope],
    allow_deferred: bool,
) -> bool {
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
            if allow_deferred && value.output_contract.kind == EffectOutputKind::ToolResult =>
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
    if !matches!(
        batch.calls[target_index].status,
        ActiveToolCallStatus::Requested { .. }
    ) || batch.calls[target_index].assigned.group_index != batch.current_group
    {
        return false;
    }
    if deferred {
        return rest.is_empty();
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

fn one_stage(
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

fn one_failed_stage(records: &[RecordEnvelope], cycle: u64, stage: Stage) -> bool {
    one_stage(records, cycle, stage, |disposition| {
        matches!(disposition, StageDisposition::Failed { .. })
    })
}

fn prepare_context_shape(records: &[RecordEnvelope], cycle: u64) -> bool {
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

fn request_model_shape(kernel_state: &KernelState, records: &[RecordEnvelope]) -> bool {
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

fn model_settlement_shape(
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

fn entry_matches_completion(
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

fn finalize_shape(kernel_state: &KernelState, records: &[RecordEnvelope]) -> bool {
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

fn terminal_completed_matches(state: &KernelState, completed: &RunCompleted) -> bool {
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

fn terminal_failed_matches(state: &KernelState, failed: &RunFailed) -> bool {
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

#[expect(
    clippy::too_many_lines,
    reason = "the replay dispatcher exhaustively covers the frozen record vocabulary"
)]
fn apply_record(
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
            let (digest, context_bytes) = crate::entries::context_digest_and_len(&context.messages)
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
            let outstanding = super::decide::outstanding_requested_effects(state);
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
                super::tool::buffer_reconciled_tool_closures(batch, &reconciled.completed_effects)?;
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
            state.state_version = 4;
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
            state.state_version = 4;
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
            state.state_version = 5;
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
            state.state_version = 5;
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
            state.state_version = 5;
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
            state.state_version = 5;
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
            state.state_version = 5;
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
                if !super::tool::buffer_cancelled_effect(batch, cancelled.effect_id())? {
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

fn interaction_request_shape(records: &[RecordEnvelope]) -> bool {
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

fn interaction_terminal_shape(records: &[RecordEnvelope]) -> bool {
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

fn stage_cursor_for_phase(state: &KernelState) -> Option<crate::StageCursor> {
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

fn apply_interaction_requested(
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

fn apply_interaction_effect_requested(
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

fn apply_interaction_resolved(
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
    let digest = super::interaction::resolution_digest(resolved)
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    let outcome = super::interaction::approval_outcome(pending.request.kind(), resolved.response())
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    state.resolution_identities.insert(
        Arc::from(resolved.resolution_id()),
        ResolutionIdentity {
            interaction_id: resolved.interaction_id(),
            settlement_digest: digest,
        },
    );
    state.last_interaction_terminal =
        Some(super::interaction::terminal_from_pending(pending, outcome));
    state.state_version = state.state_version.max(6);
    Ok(())
}

fn apply_interaction_expired(
    state: &mut KernelState,
    expired: &crate::InteractionExpired,
) -> Result<(), KernelError> {
    let pending = state
        .pending_interaction
        .as_ref()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if pending.request.interaction_id() != expired.interaction_id {
        return Err(KernelError::InvalidRecordOrder);
    }
    state.last_interaction_terminal = Some(super::interaction::terminal_from_pending(
        pending,
        InteractionTerminalOutcome::Expired,
    ));
    state.state_version = state.state_version.max(6);
    Ok(())
}

fn apply_interaction_cancelled(
    state: &mut KernelState,
    cancelled: &crate::InteractionCancelled,
) -> Result<(), KernelError> {
    let pending = state
        .pending_interaction
        .as_ref()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if pending.request.interaction_id() != cancelled.interaction_id() {
        return Err(KernelError::InvalidRecordOrder);
    }
    state.last_interaction_terminal = Some(super::interaction::terminal_from_pending(
        pending,
        InteractionTerminalOutcome::Cancelled,
    ));
    state.state_version = state.state_version.max(6);
    Ok(())
}

fn apply_interaction_effect_terminal(
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

fn apply_timer_effect_cancelled(
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

fn apply_limit_reached(
    state: &mut KernelState,
    reached: &crate::LimitReached,
) -> Result<(), KernelError> {
    reached
        .validate()
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    state.limit_usage = reached.usage.clone();
    Ok(())
}

fn apply_effect_requested(
    state: &mut KernelState,
    requested: &crate::EffectRequested,
    next: Option<&RecordBody>,
) -> Result<(), KernelError> {
    if requested.kind() == EffectKind::Interaction {
        return apply_interaction_effect_requested(state, requested, next);
    }
    if requested.kind() == EffectKind::Timer {
        let pending = state
            .retry
            .pending
            .as_ref()
            .ok_or(KernelError::InvalidRecordOrder)?;
        if pending.timer_effect_id != requested.effect_id()
            || requested.output_contract().kind != EffectOutputKind::TimerFiring
            || !matches!(requested.input(), crate::EffectInput::Timer { due_at } if *due_at == pending.due_at)
        {
            return Err(KernelError::InvalidRecordOrder);
        }
        state.phase = Some(RunPhase::Sleeping);
        return Ok(());
    }
    if requested.kind() == EffectKind::Model {
        state.limit_usage.model_requests = state
            .limit_usage
            .model_requests
            .checked_add(1)
            .ok_or(KernelError::InvalidRecordOrder)?;
    }
    if requested.kind() == EffectKind::Tool {
        let batch = state
            .active_tool_batch
            .as_mut()
            .ok_or(KernelError::InvalidRecordOrder)?;
        let call_index = batch
            .calls
            .iter()
            .position(|call| call.assigned.effect_id == requested.effect_id())
            .ok_or(KernelError::InvalidRecordOrder)?;
        let active_call = &batch.calls[call_index];
        if !matches!(active_call.status, ActiveToolCallStatus::Undispatched)
            || active_call.assigned.group_index < batch.current_group
            || requested.output_contract().kind != EffectOutputKind::ToolResult
            || !matches!(requested.input(), crate::EffectInput::Tool { call: input } if input == active_call.assigned.plan.call())
        {
            return Err(KernelError::InvalidRecordOrder);
        }
        let requested_group = active_call.assigned.group_index;
        if requested_group > batch.current_group
            && batch.calls.iter().any(|call| {
                call.assigned.group_index < requested_group
                    && !matches!(call.status, ActiveToolCallStatus::Settled { .. })
            })
        {
            return Err(KernelError::InvalidRecordOrder);
        }
        let active_call = &mut Arc::make_mut(&mut batch.calls)[call_index];
        batch.current_group = requested_group;
        active_call.status = ActiveToolCallStatus::Requested {
            requested: requested.clone(),
            deferred: None,
        };
        state.phase = Some(RunPhase::AwaitingTools);
        return Ok(());
    }
    let turn = state
        .current_turn
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let model_request_id = turn
        .model_request_id
        .ok_or(KernelError::InvariantViolation)?;
    state.pending_model_effect = Some(PendingModelEffect {
        cycle: state.cycle,
        turn_id: turn.turn_id,
        model_request_id,
        requested: requested.clone(),
        deferred: None,
    });
    state.phase = Some(RunPhase::AwaitingModel);
    Ok(())
}

fn update_wall_usage(state: &mut KernelState, now: crate::Timestamp) -> Result<(), KernelError> {
    let Some(accepted_at) = state.accepted_at else {
        return Ok(());
    };
    let elapsed = now
        .as_unix_ms()
        .checked_sub(accepted_at.as_unix_ms())
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(KernelError::InvalidRecordOrder)?;
    state.limit_usage.wall_time = crate::Duration::from_millis(elapsed);
    Ok(())
}

fn apply_completed_usage(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
) -> Result<(), KernelError> {
    state.limit_usage.output_bytes = state
        .limit_usage
        .output_bytes
        .checked_add(
            u64::try_from(completed.output().as_bytes().len())
                .map_err(|_| KernelError::InvalidRecordOrder)?,
        )
        .ok_or(KernelError::InvalidRecordOrder)?;
    let Some(usage) = completed.usage() else {
        return Ok(());
    };
    if let Some(value) = usage.input_tokens() {
        state.limit_usage.input_tokens = state
            .limit_usage
            .input_tokens
            .checked_add(value)
            .ok_or(KernelError::InvalidRecordOrder)?;
    }
    if let Some(value) = usage.output_tokens() {
        state.limit_usage.output_tokens = state
            .limit_usage
            .output_tokens
            .checked_add(value)
            .ok_or(KernelError::InvalidRecordOrder)?;
    }
    if let Some(value) = usage.cost() {
        let current = state
            .limit_usage
            .cost
            .as_ref()
            .map_or(0, crate::CostAmount::micros);
        let micros = current
            .checked_add(value.micros())
            .ok_or(KernelError::InvalidRecordOrder)?;
        state.limit_usage.cost = Some(
            crate::CostAmount::try_new(value.unit(), micros, value.pricing_policy_version())
                .map_err(|_| KernelError::InvalidRecordOrder)?,
        );
    }
    for (key, delta) in usage.extension_counters() {
        let current = state
            .limit_usage
            .extension_counters
            .get(key)
            .copied()
            .unwrap_or(0);
        state.limit_usage.extension_counters.insert(
            key.clone(),
            current
                .checked_add(*delta)
                .ok_or(KernelError::InvalidRecordOrder)?,
        );
    }
    Ok(())
}

fn apply_effect_deferred(
    state: &mut KernelState,
    deferred: &crate::EffectDeferred,
) -> Result<(), KernelError> {
    if deferred.output_contract.kind == EffectOutputKind::ToolResult {
        let batch = state
            .active_tool_batch
            .as_mut()
            .ok_or(KernelError::InvalidRecordOrder)?;
        let call = Arc::make_mut(&mut batch.calls)
            .iter_mut()
            .find(|call| call.assigned.effect_id == deferred.effect_id)
            .ok_or(KernelError::InvalidRecordOrder)?;
        let ActiveToolCallStatus::Requested {
            requested,
            deferred: existing,
        } = &mut call.status
        else {
            return Err(KernelError::InvalidRecordOrder);
        };
        deferred
            .validate_against(requested)
            .map_err(|_| KernelError::ToolSettlementMismatch)?;
        if existing.is_some() {
            return Err(KernelError::InvalidRecordOrder);
        }
        *existing = Some(deferred.clone());
        state.phase = Some(RunPhase::AwaitingExternal);
        return Ok(());
    }
    let pending = state
        .pending_model_effect
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    pending.deferred = Some(deferred.clone());
    state.phase = Some(RunPhase::AwaitingExternal);
    Ok(())
}

fn apply_effect_completed(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
    next: Option<&RecordBody>,
) -> Result<(), KernelError> {
    if completed.output_contract().kind == EffectOutputKind::InteractionResolution {
        return apply_interaction_effect_terminal(
            state,
            completed.effect_id(),
            completed.completion_id(),
            completed.output_digest(),
        );
    }
    if completed.output_contract().kind == EffectOutputKind::ToolResult {
        return apply_tool_effect_completed(state, completed);
    }
    let RecordBody::EntryAppended(entry) = next.ok_or(KernelError::InvalidRecordOrder)? else {
        return Err(KernelError::InvalidRecordOrder);
    };
    index_completed_settlement(state, completed, entry)?;
    state.terminal_candidate = Some(TerminalCandidate::Completed {
        cycle: entry.cycle,
        turn_id: entry.turn_id,
        model_request_id: entry.model_request_id,
        effect_id: entry.effect_id,
        message_id: *entry.message.id(),
        result_digest: completed.output_digest(),
    });
    Ok(())
}

fn apply_entry_appended(state: &mut KernelState, entry: &EntryAppended) -> Result<(), KernelError> {
    let message_id = *entry.message.id();
    let mut has_tool_calls = false;
    for block in entry.message.content() {
        if let ContentBlock::ToolCall(call) = block
            && !crate::is_internal_tool_name(call.tool_name())
        {
            has_tool_calls = true;
            if state.tool_calls.contains_key(call.tool_call_id()) {
                return Err(KernelError::DuplicateToolCall);
            }
            state.tool_calls.insert(
                *call.tool_call_id(),
                ToolCallIdentity {
                    cycle: entry.cycle,
                    turn_id: entry.turn_id,
                    source_message_id: message_id,
                    tool_batch_id: None,
                    effect_id: None,
                    call: call.clone(),
                },
            );
        }
    }
    if has_tool_calls {
        state.state_version = state.state_version.max(2);
    }
    Arc::make_mut(&mut state.messages).push(entry.message.clone());
    if let Some(turn) = state.current_turn.as_mut() {
        turn.final_message_id = Some(message_id);
    }
    if !matches!(
        state.terminal_candidate,
        Some(TerminalCandidate::Completed {
            effect_id,
            message_id: candidate_message_id,
            ..
        }) if effect_id == entry.effect_id && candidate_message_id == message_id
    ) {
        return Err(KernelError::InvariantViolation);
    }
    state.pending_model_effect = None;
    state.phase = Some(RunPhase::AfterModel);
    Ok(())
}

fn apply_final_result(
    state: &mut KernelState,
    result: &crate::FinalResultRecorded,
) -> Result<(), KernelError> {
    result
        .validate()
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    let Some(crate::OutputConfiguration {
        output: crate::OutputSpec::JsonSchema { schema },
        end_strategy,
    }) = state.output_configuration.as_ref()
    else {
        return Err(KernelError::InvalidRecordOrder);
    };
    let candidate_matches = matches!(
        state.terminal_candidate.as_ref(),
        Some(TerminalCandidate::Completed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            ..
        }) if result.cycle == *cycle
            && result.turn_id == *turn_id
            && result.model_request_id == *model_request_id
            && result.effect_id == *effect_id
            && result.message_id == *message_id
    );
    let message = state
        .messages
        .last()
        .filter(|message| *message.id() == result.message_id)
        .ok_or(KernelError::InvalidRecordOrder)?;
    let expected_skipped = if *end_strategy == crate::OutputEndStrategy::Early {
        application_tool_ids(message)
    } else {
        Vec::new()
    };
    if !candidate_matches
        || &result.schema != schema
        || result.end_strategy != *end_strategy
        || result.value_digest != result.value.digest()
        || structured_source_value(message, &result.source) != Some(&result.value)
        || result.skipped_tool_call_ids.as_ref() != expected_skipped.as_slice()
        || state.final_result.is_some()
        || state.validation_failure.is_some()
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    state.terminal_candidate = Some(TerminalCandidate::Completed {
        cycle: result.cycle,
        turn_id: result.turn_id,
        model_request_id: result.model_request_id,
        effect_id: result.effect_id,
        message_id: result.message_id,
        result_digest: result.value_digest,
    });
    state.final_result = Some(result.clone());
    state.state_version = 4;
    Ok(())
}

fn apply_validation_failure(
    state: &mut KernelState,
    failure: &crate::OutputValidationFailed,
) -> Result<(), KernelError> {
    failure
        .validate()
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    let Some(crate::OutputConfiguration {
        output: crate::OutputSpec::JsonSchema { schema },
        ..
    }) = state.output_configuration.as_ref()
    else {
        return Err(KernelError::InvalidRecordOrder);
    };
    let candidate_matches = matches!(
        state.terminal_candidate.as_ref(),
        Some(TerminalCandidate::Completed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            ..
        }) if failure.cycle == *cycle
            && failure.turn_id == *turn_id
            && failure.model_request_id == *model_request_id
            && failure.effect_id == *effect_id
            && failure.message_id == *message_id
    );
    let message = state
        .messages
        .last()
        .filter(|message| *message.id() == failure.message_id)
        .ok_or(KernelError::InvalidRecordOrder)?;
    let expected_error = crate::validation::expected_validation_error(
        state.retry.attempts,
        state
            .accepted
            .as_ref()
            .and_then(|accepted| accepted.limits().max_retries),
    )
    .map_err(|_| KernelError::InvalidInputPayload {
        field: "validation_error",
        reason_code: "expected_error_unavailable",
    })?;
    if !candidate_matches
        || &failure.schema != schema
        || failure.issues.is_empty()
        || failure.error != expected_error
        || structured_source_value(message, &failure.source)
            .is_none_or(|value| value.digest() != failure.candidate_digest)
        || failure.skipped_tool_call_ids.as_ref() != application_tool_ids(message).as_slice()
        || state.final_result.is_some()
        || state.validation_failure.is_some()
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    state.terminal_candidate = Some(TerminalCandidate::Failed {
        cycle: failure.cycle,
        turn_id: Some(failure.turn_id),
        model_request_id: Some(failure.model_request_id),
        effect_id: Some(failure.effect_id),
        error: failure.error.clone(),
    });
    state.validation_failure = Some(failure.clone());
    state.state_version = 4;
    Ok(())
}

fn structured_source_value<'a>(
    message: &'a crate::Message,
    source: &crate::StructuredResultSource,
) -> Option<&'a crate::RawJson> {
    match source {
        crate::StructuredResultSource::JsonBlock { content_index } => {
            usize::try_from(*content_index)
                .ok()
                .and_then(|index| message.content().get(index))
                .and_then(|block| match block {
                    ContentBlock::Json(value) => Some(value.value()),
                    _ => None,
                })
        }
        crate::StructuredResultSource::InternalTool { tool_call_id } => {
            message.content().iter().find_map(|block| match block {
                ContentBlock::ToolCall(call)
                    if call.tool_call_id() == tool_call_id
                        && call.tool_name() == crate::SUBMIT_FINAL_OUTPUT_TOOL =>
                {
                    Some(call.arguments())
                }
                _ => None,
            })
        }
    }
}

fn application_tool_ids(message: &crate::Message) -> Vec<crate::ToolCallId> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if !crate::is_internal_tool_name(call.tool_name()) => {
                Some(*call.tool_call_id())
            }
            _ => None,
        })
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "batch opening validates source order, grouping, identities, and replay state atomically"
)]
fn apply_tool_batch_opened(
    state: &mut KernelState,
    opened: &crate::ToolBatchOpened,
) -> Result<(), KernelError> {
    if state.active_tool_batch.is_some()
        || opened.cycle != state.cycle
        || state
            .current_turn
            .as_ref()
            .is_none_or(|turn| turn.turn_id != opened.turn_id)
        || state
            .messages
            .last()
            .is_none_or(|message| *message.id() != opened.source_message_id)
        || opened.calls.is_empty()
        || opened.calls.len() > crate::SEMANTIC_ARRAY_MAX_ITEMS
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    let source_calls = source_tool_calls_for_open(state, opened)?;
    let mut calls = Vec::with_capacity(opened.calls.len());
    let mut prior_group = 0_u32;
    for (index, assigned) in opened.calls.iter().enumerate() {
        let source_index = u32::try_from(index).map_err(|_| KernelError::InvalidRecordOrder)?;
        let identity = state
            .tool_calls
            .get_mut(assigned.plan.call().tool_call_id())
            .ok_or(KernelError::ToolBatchPlanMismatch)?;
        if assigned.source_index != source_index
            || assigned.plan.call() != &source_calls[index]
            || identity.call != *assigned.plan.call()
            || identity.source_message_id != opened.source_message_id
            || identity.effect_id.is_some()
        {
            return Err(KernelError::ToolBatchPlanMismatch);
        }
        if index == 0 {
            if assigned.group_index != 0 {
                return Err(KernelError::ToolBatchPlanMismatch);
            }
        } else {
            let prior_mode = opened.calls[index - 1].plan.execution();
            let expected = if prior_mode == crate::ToolExecutionMode::Parallel
                && assigned.plan.execution() == crate::ToolExecutionMode::Parallel
            {
                prior_group
            } else {
                prior_group
                    .checked_add(1)
                    .ok_or(KernelError::ToolBatchPlanMismatch)?
            };
            if assigned.group_index != expected {
                return Err(KernelError::ToolBatchPlanMismatch);
            }
        }
        prior_group = assigned.group_index;
        identity.tool_batch_id = Some(opened.tool_batch_id);
        identity.effect_id = Some(assigned.effect_id);
        let status = match &assigned.plan {
            ToolCallPlan::Execute(call) => {
                if call.output_contract.kind != EffectOutputKind::ToolResult {
                    return Err(KernelError::ToolEffectContractMismatch);
                }
                ActiveToolCallStatus::Undispatched
            }
            ToolCallPlan::SyntheticClosure(closure) => {
                let result = super::tool::synthetic_result(&closure.call, &closure.error)?;
                let digest = synthetic_tool_digest(
                    opened.tool_batch_id,
                    *closure.call.tool_call_id(),
                    assigned.effect_id,
                    &result,
                    &closure.error,
                )?;
                ActiveToolCallStatus::Buffered {
                    result,
                    settlement_digest: digest,
                    synthetic: true,
                    error: Some(closure.error.clone()),
                }
            }
        };
        calls.push(ActiveToolCall {
            assigned: assigned.clone(),
            status,
        });
    }
    let current_group = calls
        .iter()
        .find_map(|call| {
            matches!(call.assigned.plan, ToolCallPlan::Execute(_))
                .then_some(call.assigned.group_index)
        })
        .unwrap_or(0);
    state.active_tool_batch = Some(ActiveToolBatch {
        opened: opened.clone(),
        calls: calls.into(),
        current_group,
        next_source_index: 0,
        result_message_ids: Arc::from([]),
        fatal_error: None,
    });
    state.last_tool_batch = None;
    state.phase = Some(RunPhase::AwaitingTools);
    Ok(())
}

fn source_tool_calls_for_open(
    state: &KernelState,
    opened: &crate::ToolBatchOpened,
) -> Result<Vec<crate::ToolCallBlock>, KernelError> {
    let source = state
        .messages
        .last()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let calls = source
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if !crate::is_internal_tool_name(call.tool_name()) => {
                Some(call.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if calls.len() != opened.calls.len() {
        return Err(KernelError::ToolBatchPlanMismatch);
    }
    Ok(calls)
}

fn apply_tool_effect_completed(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let index = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == completed.effect_id())
        .ok_or(KernelError::InvalidRecordOrder)?;
    let (requested, external, planned_call) = match &batch.calls[index].status {
        ActiveToolCallStatus::Requested {
            requested,
            deferred,
        } => (
            requested.clone(),
            deferred.is_some(),
            batch.calls[index].assigned.plan.call().clone(),
        ),
        _ => return Err(KernelError::InvalidRecordOrder),
    };
    completed
        .validate_against(&requested)
        .map_err(|_| KernelError::ToolSettlementMismatch)?;
    let result = serde_json::from_str::<crate::ToolResultBlock>(completed.output().as_str())
        .map_err(|_| KernelError::ToolResultMismatch)?;
    if result.tool_call_id() != planned_call.tool_call_id() {
        return Err(KernelError::ToolResultMismatch);
    }
    let digest = completed_tool_record_digest(batch.opened.tool_batch_id, external, completed)?;
    insert_tool_identity(
        state,
        completed.effect_id(),
        ToolSettlementKind::Completed,
        digest,
        completed.completion_id(),
    )?;
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut batch.calls)[index].status = ActiveToolCallStatus::Buffered {
        result,
        settlement_digest: digest,
        synthetic: false,
        error: None,
    };
    state.phase = Some(tool_wait_phase(batch));
    Ok(())
}

fn apply_tool_effect_failed(
    state: &mut KernelState,
    failed: &crate::EffectFailed,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let index = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == failed.effect_id())
        .ok_or(KernelError::InvalidRecordOrder)?;
    let (requested, external, policy, planned_call) = match &batch.calls[index].status {
        ActiveToolCallStatus::Requested {
            requested,
            deferred,
        } => (
            requested.clone(),
            deferred.is_some(),
            batch.calls[index].assigned.plan.failure_policy(),
            batch.calls[index].assigned.plan.call().clone(),
        ),
        _ => return Err(KernelError::InvalidRecordOrder),
    };
    failed
        .validate_against(&requested)
        .map_err(|_| KernelError::ToolSettlementMismatch)?;
    let digest = failed_tool_record_digest(batch.opened.tool_batch_id, external, failed)?;
    let result = super::tool::synthetic_result(&planned_call, failed.error())?;
    insert_tool_identity(
        state,
        failed.effect_id(),
        ToolSettlementKind::Failed,
        digest,
        failed.completion_id(),
    )?;
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut batch.calls)[index].status = ActiveToolCallStatus::Buffered {
        result,
        settlement_digest: digest,
        synthetic: true,
        error: Some(failed.error().clone()),
    };
    if policy == crate::ToolFailurePolicy::FailRun && batch.fatal_error.is_none() {
        batch.fatal_error = Some(failed.error().clone());
    }
    state.phase = Some(tool_wait_phase(batch));
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "the frozen source-order, authorship, and digest checks are one atomic apply invariant"
)]
fn apply_tool_call_settled(
    state: &mut KernelState,
    settled: &crate::ToolCallSettled,
    record: &RecordEnvelope,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvalidRecordOrder)?;
    let index =
        usize::try_from(batch.next_source_index).map_err(|_| KernelError::InvalidRecordOrder)?;
    let call = batch
        .calls
        .get(index)
        .ok_or(KernelError::InvalidRecordOrder)?;
    if settled.cycle != batch.opened.cycle
        || settled.turn_id != batch.opened.turn_id
        || settled.tool_batch_id != batch.opened.tool_batch_id
        || settled.tool_call_id != *call.assigned.plan.call().tool_call_id()
        || settled.effect_id != call.assigned.effect_id
        || settled.message.role() != MessageRole::Tool
        || settled.message.created_at() != record.timestamp()
        || settled.message.model().is_some()
        || settled.message.provider_ids() != &crate::ProviderIds::empty()
        || settled.message.metadata() != &crate::Metadata::empty()
        || settled.message.content().len() != 1
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    let ContentBlock::ToolResult(message_result) = &settled.message.content()[0] else {
        return Err(KernelError::InvalidRecordOrder);
    };
    if message_result.tool_call_id() != call.assigned.plan.call().tool_call_id() {
        return Err(KernelError::ToolResultMismatch);
    }
    match &call.status {
        ActiveToolCallStatus::Buffered {
            result,
            settlement_digest,
            synthetic,
            error,
        } if result == message_result
            && *settlement_digest == settled.settlement_digest
            && *synthetic == settled.synthetic
            && error == &settled.error => {}
        ActiveToolCallStatus::Undispatched
            if batch.fatal_error.is_some()
                && settled.synthetic
                && settled
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code.as_str() == "tool_batch_aborted") =>
        {
            let error = settled
                .error
                .as_ref()
                .ok_or(KernelError::ToolResultMismatch)?;
            let expected = super::tool::synthetic_result(call.assigned.plan.call(), error)?;
            if expected != *message_result
                || synthetic_tool_digest(
                    settled.tool_batch_id,
                    settled.tool_call_id,
                    settled.effect_id,
                    message_result,
                    error,
                )? != settled.settlement_digest
            {
                return Err(KernelError::SettlementDigestMismatch);
            }
        }
        _ => return Err(KernelError::InvalidRecordOrder),
    }
    if settled.synthetic {
        let error = settled
            .error
            .as_ref()
            .ok_or(KernelError::ToolResultMismatch)?;
        if !message_result.is_error() {
            return Err(KernelError::SettlementDigestMismatch);
        }
        let synthetic_digest = synthetic_tool_digest(
            settled.tool_batch_id,
            settled.tool_call_id,
            settled.effect_id,
            message_result,
            error,
        )?;
        if let Some(existing) = state.tool_settlements.get(&settled.effect_id) {
            if existing.digest != settled.settlement_digest
                || (existing.kind == ToolSettlementKind::Synthetic
                    && synthetic_digest != settled.settlement_digest)
                || !matches!(
                    existing.kind,
                    ToolSettlementKind::Failed | ToolSettlementKind::Synthetic
                )
            {
                return Err(KernelError::SettlementDigestMismatch);
            }
        } else {
            if synthetic_digest != settled.settlement_digest {
                return Err(KernelError::SettlementDigestMismatch);
            }
            insert_tool_identity(
                state,
                settled.effect_id,
                ToolSettlementKind::Synthetic,
                settled.settlement_digest,
                None,
            )?;
        }
    } else if settled.error.is_some() {
        return Err(KernelError::ToolResultMismatch);
    }
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut batch.calls)[index].status = ActiveToolCallStatus::Settled {
        result_message_id: *settled.message.id(),
        settlement_digest: settled.settlement_digest,
    };
    let mut result_ids = batch.result_message_ids.to_vec();
    result_ids.push(*settled.message.id());
    batch.result_message_ids = result_ids.into();
    batch.next_source_index = batch
        .next_source_index
        .checked_add(1)
        .ok_or(KernelError::InvariantViolation)?;
    Arc::make_mut(&mut state.messages).push(settled.message.clone());
    Ok(())
}

fn apply_tool_batch_closed(
    state: &mut KernelState,
    closed: &crate::ToolBatchClosed,
) -> Result<(), KernelError> {
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(KernelError::InvalidRecordOrder)?;
    if closed.cycle != batch.opened.cycle
        || closed.turn_id != batch.opened.turn_id
        || closed.tool_batch_id != batch.opened.tool_batch_id
        || closed.source_message_id != batch.opened.source_message_id
        || closed.result_message_ids != batch.result_message_ids
        || !batch
            .calls
            .iter()
            .all(|call| matches!(call.status, ActiveToolCallStatus::Settled { .. }))
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    match (
        &batch.fatal_error,
        &batch.opened.continuation,
        &closed.outcome,
    ) {
        (Some(expected), _, ToolBatchOutcome::Failed { error }) if expected == error => {}
        (None, crate::ToolBatchContinuation::ContinueModel, ToolBatchOutcome::ContinueModel)
        | (None, crate::ToolBatchContinuation::Finalize, ToolBatchOutcome::Finalize) => {}
        _ => return Err(KernelError::InvalidRecordOrder),
    }
    if let ToolBatchOutcome::Failed { error } = &closed.outcome {
        state.terminal_candidate = Some(match state.terminal_candidate.as_ref() {
            Some(TerminalCandidate::Completed {
                cycle,
                turn_id,
                model_request_id,
                effect_id,
                ..
            }) => TerminalCandidate::Failed {
                cycle: *cycle,
                turn_id: Some(*turn_id),
                model_request_id: Some(*model_request_id),
                effect_id: Some(*effect_id),
                error: error.clone(),
            },
            _ => return Err(KernelError::InvariantViolation),
        });
    }
    state.last_tool_batch = Some(closed.clone());
    state.active_tool_batch = None;
    state.phase = Some(RunPhase::AfterToolBatch);
    Ok(())
}

fn insert_tool_identity(
    state: &mut KernelState,
    effect_id: crate::EffectId,
    kind: ToolSettlementKind,
    digest: Digest,
    completion_id: Option<&str>,
) -> Result<(), KernelError> {
    if let Some(existing) = state.tool_settlements.get(&effect_id)
        && (existing.kind != kind || existing.digest != digest)
    {
        return Err(KernelError::ConflictingSettlement);
    }
    state
        .tool_settlements
        .insert(effect_id, ToolSettlementFingerprint { kind, digest });
    if let Some(completion_id) = completion_id {
        if let Some(existing) = state.completion_identities.get(completion_id)
            && (existing.effect_id != effect_id || existing.settlement_digest != digest)
        {
            return Err(KernelError::ConflictingCompletionId);
        }
        state.completion_identities.insert(
            Arc::from(completion_id),
            CompletionIdentity {
                effect_id,
                settlement_digest: digest,
            },
        );
    }
    Ok(())
}

fn tool_wait_phase(batch: &ActiveToolBatch) -> RunPhase {
    if batch.calls.iter().any(|call| {
        matches!(
            call.status,
            ActiveToolCallStatus::Requested {
                deferred: Some(_),
                ..
            }
        )
    }) {
        RunPhase::AwaitingExternal
    } else {
        RunPhase::AwaitingTools
    }
}

fn apply_effect_failed(
    state: &mut KernelState,
    failed: &crate::EffectFailed,
) -> Result<(), KernelError> {
    if failed.output_contract().kind == EffectOutputKind::InteractionResolution {
        return apply_interaction_effect_terminal(
            state,
            failed.effect_id(),
            failed.completion_id(),
            crate::Digest::raw_json(b"interaction-effect-failed"),
        );
    }
    if failed.output_contract().kind == EffectOutputKind::ToolResult {
        return apply_tool_effect_failed(state, failed);
    }
    index_failed_settlement(state, failed)?;
    let pending = state
        .pending_model_effect
        .take()
        .ok_or(KernelError::InvariantViolation)?;
    state.terminal_candidate = Some(TerminalCandidate::Failed {
        cycle: pending.cycle,
        turn_id: Some(pending.turn_id),
        model_request_id: Some(pending.model_request_id),
        effect_id: Some(pending.requested.effect_id()),
        error: failed.error().clone(),
    });
    state.phase = Some(RunPhase::BeforeFinalize);
    Ok(())
}

fn apply_stage_outcome(
    state: &mut KernelState,
    outcome: &StageOutcomeRecorded,
) -> Result<(), KernelError> {
    if let Some(existing) = state
        .stage_settlements
        .insert(outcome.cursor, outcome.settlement_digest)
        && existing != outcome.settlement_digest
    {
        return Err(KernelError::SettlementDigestMismatch);
    }
    match &outcome.disposition {
        StageDisposition::Continued => {
            state.phase = Some(match outcome.cursor.stage {
                Stage::BeforeRun => RunPhase::PreparingContext,
                Stage::AfterModel => {
                    if state.validation_failure.is_some() {
                        RunPhase::BeforeFinalize
                    } else if matches!(
                        state.output_configuration,
                        Some(crate::OutputConfiguration {
                            output: crate::OutputSpec::JsonSchema { .. },
                            ..
                        })
                    ) && state.final_result.is_none()
                    {
                        return Err(KernelError::InvalidRecordOrder);
                    } else {
                        let has_tools = state.messages.last().is_some_and(|message| {
                            message.content().iter().any(|block| {
                                matches!(block, ContentBlock::ToolCall(call)
                                if !crate::is_internal_tool_name(call.tool_name()))
                            })
                        });
                        let ends_early = state.final_result.as_ref().is_some_and(|result| {
                            result.end_strategy == crate::OutputEndStrategy::Early
                        });
                        if has_tools && !ends_early {
                            RunPhase::BeforeToolBatch
                        } else {
                            RunPhase::BeforeFinalize
                        }
                    }
                }
                Stage::AfterToolBatch => {
                    match state.last_tool_batch.as_ref().map(|closed| &closed.outcome) {
                        Some(crate::ToolBatchOutcome::ContinueModel) => {
                            state.cycle = state
                                .cycle
                                .checked_add(1)
                                .ok_or(KernelError::CycleOverflow)?;
                            state.current_turn = None;
                            state.terminal_candidate = None;
                            state.final_result = None;
                            state.validation_failure = None;
                            state.last_tool_batch = None;
                            RunPhase::PreparingContext
                        }
                        Some(
                            crate::ToolBatchOutcome::Finalize
                            | crate::ToolBatchOutcome::Failed { .. },
                        ) => RunPhase::BeforeFinalize,
                        None => return Err(KernelError::InvalidRecordOrder),
                    }
                }
                _ => return Err(KernelError::InvalidRecordOrder),
            });
        }
        StageDisposition::ContextPrepared { .. }
        | StageDisposition::ToolBatchPrepared { .. }
        | StageDisposition::FinalizeAccepted
        | StageDisposition::RetryScheduled { .. } => {}
        StageDisposition::ModelRequested {
            model_request_id,
            effect_id,
            ..
        } => {
            let turn = state
                .current_turn
                .as_mut()
                .ok_or(KernelError::InvariantViolation)?;
            turn.model_request_id = Some(*model_request_id);
            turn.effect_id = Some(*effect_id);
        }
        StageDisposition::ContinueModel { next_cycle } => {
            state.cycle = *next_cycle;
            state.current_turn = None;
            state.pending_model_effect = None;
            state.terminal_candidate = None;
            state.final_result = None;
            state.validation_failure = None;
            state.phase = Some(RunPhase::PreparingContext);
        }
        StageDisposition::Failed { error } => {
            state.terminal_candidate = Some(failure_candidate(state, error.clone()));
            if outcome.cursor.stage != Stage::BeforeFinalize {
                state.phase = Some(RunPhase::BeforeFinalize);
            }
        }
    }
    Ok(())
}

fn index_completed_settlement(
    state: &mut KernelState,
    completed: &crate::EffectCompleted,
    entry: &EntryAppended,
) -> Result<(), KernelError> {
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let digest = completed_record_digest(pending, completed, &entry.message)?;
    insert_model_identity(
        state,
        completed.effect_id(),
        ModelSettlementKind::Completed,
        digest,
        completed.completion_id(),
    )
}

fn index_failed_settlement(
    state: &mut KernelState,
    failed: &crate::EffectFailed,
) -> Result<(), KernelError> {
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let digest = failed_record_digest(pending, failed)?;
    insert_model_identity(
        state,
        failed.effect_id(),
        ModelSettlementKind::Failed,
        digest,
        failed.completion_id(),
    )
}

fn insert_model_identity(
    state: &mut KernelState,
    effect_id: crate::EffectId,
    kind: ModelSettlementKind,
    digest: Digest,
    completion_id: Option<&str>,
) -> Result<(), KernelError> {
    if let Some(existing) = state.model_settlements.get(&effect_id)
        && (existing.kind != kind || existing.digest != digest)
    {
        return Err(KernelError::ConflictingSettlement);
    }
    state
        .model_settlements
        .insert(effect_id, ModelSettlementFingerprint { kind, digest });
    if let Some(completion_id) = completion_id {
        if let Some(existing) = state.completion_identities.get(completion_id)
            && (existing.effect_id != effect_id || existing.settlement_digest != digest)
        {
            return Err(KernelError::ConflictingCompletionId);
        }
        state.completion_identities.insert(
            Arc::from(completion_id),
            CompletionIdentity {
                effect_id,
                settlement_digest: digest,
            },
        );
    }
    Ok(())
}

fn failure_candidate(state: &KernelState, error: crate::ErrorDescriptor) -> TerminalCandidate {
    let turn = state.current_turn.as_ref();
    TerminalCandidate::Failed {
        cycle: state.cycle,
        turn_id: turn.map(|value| value.turn_id),
        model_request_id: turn.and_then(|value| value.model_request_id),
        effect_id: turn.and_then(|value| value.effect_id),
        error,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::effects::{
        EffectInput, EffectOutputContract, EffectRequested, ReconciliationPolicy, RetrySafety,
    };
    use crate::ids::ComponentId;
    use crate::message::{Message, ProviderIds};
    use crate::raw_json::{Metadata, RawJson};
    use crate::records::{RECORD_FORMAT_VERSION, RECORD_KIND_VERSION};
    use crate::refs::ExternalHandleRef;
    use crate::state::{ModelSettlementFingerprint, ModelSettlementKind};
    use crate::time::Timestamp;
    use crate::{CommittedBatch, TextBlock};

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one end-to-end replay fixture keeps preparation, settlement, round-trip, and conflict evidence together"
    )]
    fn child_preparation_and_budget_replay_are_idempotent_and_conflict_closed() {
        let timestamp = Timestamp::from_unix_ms(1_000).expect("timestamp");
        let session_id = fixed_id::<crate::SessionTag>(10);
        let lane_id = fixed_id::<crate::LaneTag>(11);
        let parent_run_id = fixed_id::<crate::RunTag>(12);
        let parent_effect_id = fixed_id::<crate::EffectTag>(13);
        let child_run_id = fixed_id::<crate::RunTag>(14);
        let reservation_id = fixed_id::<crate::BudgetReservationTag>(15);
        let scope_id = fixed_id::<crate::BudgetScopeTag>(16);
        let accepted = crate::RunAccepted::try_new(
            parent_run_id,
            crate::RunRelation::root(parent_run_id).expect("root relation"),
            crate::RunSecurityContext::try_new(
                "tenant-a",
                crate::PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            crate::RunLimits::empty(),
            crate::RunPropagationPolicy {
                cancellation: crate::CancellationPropagation::Cascade,
                deadline: crate::DeadlinePropagation::MinimumOfParentAndChild,
                budget: crate::BudgetPropagation::ReservedChildAllocation,
                principal: crate::PrincipalPropagation::Inherit,
            },
            Digest::raw_json(b"agent-lock"),
            None,
        )
        .expect("accepted");
        let state = KernelState {
            session_id: Some(session_id),
            lane_id: Some(lane_id),
            accepted: Some(accepted),
            accepted_at: Some(timestamp),
            phase: Some(RunPhase::BeforeRun),
            ..KernelState::default()
        };
        let amount = crate::BudgetRequest {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cost: None,
            extension_counters: BTreeMap::new(),
        };
        let request_digest = crate::BudgetReserveRequest::compute_digest(
            scope_id,
            reservation_id,
            child_run_id,
            &amount,
        )
        .expect("request digest");
        let request = crate::BudgetReserveRequest {
            scope_id,
            reservation_id,
            run_id: child_run_id,
            amount: amount.clone(),
            request_digest,
        };
        let prepared = crate::ChildRunPrepared {
            parent_run_id,
            parent_effect_id,
            child: crate::ChildRunLocator {
                operation: crate::OperationLocator::try_new(
                    "tenant-a",
                    session_id,
                    fixed_id::<crate::LaneTag>(17),
                    child_run_id,
                )
                .expect("child locator"),
                remote: None,
            },
            request_digest: Digest::raw_json(b"child-request"),
            placement: crate::ChildPlacement::CompatibleLaneInParentSession,
            budget_reservation_id: Some(reservation_id),
        };
        let prepare_records = vec![
            composition_envelope(
                1,
                timestamp,
                session_id,
                lane_id,
                parent_run_id,
                RecordBody::ChildRunPrepared(prepared.clone()),
            ),
            composition_envelope(
                2,
                timestamp,
                session_id,
                lane_id,
                parent_run_id,
                RecordBody::BudgetReservationRequested(crate::BudgetReservationRequested {
                    request: request.clone(),
                }),
            ),
        ];
        let prepare_batch =
            CommittedBatch::try_new(fixed_id::<crate::AppendBatchTag>(20), 1, 2, prepare_records)
                .expect("prepare batch");
        let applied = apply(&state, &prepare_batch, 0).expect("prepare apply").0;
        assert_eq!(
            applied.child_preparations.get(&parent_effect_id),
            Some(&prepared)
        );
        assert_eq!(
            applied
                .budget_reservations
                .get(&reservation_id)
                .map(|replay| &replay.request),
            Some(&request)
        );

        let receipt = crate::BudgetReservationReceipt {
            scope_id,
            reservation_id,
            reserved: amount.clone(),
            remaining: crate::BudgetRequest::default(),
            request_digest,
            receipt_digest: Digest::raw_json(b"reservation-receipt"),
        };
        let settlement = composition_envelope(
            3,
            timestamp,
            session_id,
            lane_id,
            parent_run_id,
            RecordBody::BudgetReservationSettled(crate::BudgetReservationSettled {
                receipt: receipt.clone(),
            }),
        );
        let settlement_batch = CommittedBatch::try_new(
            fixed_id::<crate::AppendBatchTag>(21),
            3,
            3,
            vec![settlement],
        )
        .expect("settlement batch");
        let settled = apply(&applied, &settlement_batch, 0)
            .expect("settlement apply")
            .0;
        assert_eq!(
            settled
                .budget_reservations
                .get(&reservation_id)
                .and_then(|replay| replay.settlement.as_ref()),
            Some(&receipt)
        );
        let encoded = serde_json::to_vec(&settled).expect("v5 state JSON");
        let decoded: KernelState = serde_json::from_slice(&encoded).expect("v5 replay state");
        assert_eq!(decoded, settled);
        assert_eq!(decoded.state_hash(), settled.state_hash());

        let mut conflicting = prepared;
        conflicting.request_digest = Digest::raw_json(b"conflicting-child-request");
        let conflict_records = vec![
            composition_envelope(
                4,
                timestamp,
                session_id,
                lane_id,
                parent_run_id,
                RecordBody::ChildRunPrepared(conflicting),
            ),
            composition_envelope(
                5,
                timestamp,
                session_id,
                lane_id,
                parent_run_id,
                RecordBody::BudgetReservationRequested(crate::BudgetReservationRequested {
                    request,
                }),
            ),
        ];
        let conflict_batch = CommittedBatch::try_new(
            fixed_id::<crate::AppendBatchTag>(22),
            4,
            5,
            conflict_records,
        )
        .expect("conflict batch");
        assert_eq!(
            apply(&settled, &conflict_batch, 0),
            Err(KernelError::InvalidRecordOrder)
        );
    }

    #[test]
    fn semantic_tampering_precedes_capacity_failure() {
        let (state, batch) = external_batch_at_capacity(true);
        assert_eq!(
            apply(&state, &batch, 0),
            Err(KernelError::ModelSettlementMismatch)
        );
    }

    #[test]
    fn committed_batch_rejects_over_ceiling_with_reason_code() {
        let record = RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            fixed_id::<crate::RecordTag>(1),
            fixed_id::<crate::SessionTag>(1),
            fixed_id::<crate::LaneTag>(2),
            None,
            1,
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            Digest::raw_json(b"payload"),
            None,
            Digest::raw_json(b"checksum"),
            vec![],
            RecordBody::LaneCreated(crate::LaneCreated::try_new("research").expect("lane")),
        )
        .expect("record");
        let records = vec![record; APPEND_BATCH_MAX_RECORDS + 1];
        assert_eq!(
            CommittedBatch::try_new(fixed_id::<crate::AppendBatchTag>(99), 1, 1, records),
            Err(KernelError::InvalidInputPayload {
                field: "records",
                reason_code: "too_many_items",
            })
        );
    }

    #[test]
    fn event_sequence_overflow_keeps_reason_code() {
        let (mut state, batch) = external_batch_at_capacity(false);
        state.model_settlements.clear();
        assert_eq!(
            apply(&state, &batch, u64::MAX),
            Err(KernelError::InvalidInputPayload {
                field: "first_transient_sequence",
                reason_code: "overflow",
            })
        );
    }

    #[test]
    fn whole_batch_capacity_failure_leaves_original_state_unchanged() {
        let (state, batch) = external_batch_at_capacity(false);
        let before = state.clone();
        assert_eq!(
            apply(&state, &batch, 0),
            Err(KernelError::StateCapacityExceeded {
                field: "model_settlements"
            })
        );
        assert_eq!(state, before);
    }

    fn external_batch_at_capacity(tampered: bool) -> (KernelState, CommittedBatch) {
        let effect_id = fixed_id::<crate::EffectTag>(1);
        let turn_id = fixed_id::<crate::TurnTag>(2);
        let model_request_id = fixed_id::<crate::ModelRequestTag>(3);
        let contract = EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"schema"),
        };
        let requested = EffectRequested::try_new(
            effect_id,
            EffectKind::Model,
            None,
            None,
            None,
            contract.clone(),
            EffectInput::Model {
                request: RawJson::parse("{}").expect("request"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("request");
        let deferred = crate::EffectDeferred {
            effect_id,
            handle: ExternalHandleRef::try_new(
                ComponentId::parse("finstack.provider.fixture").expect("component"),
                "external-job",
                RawJson::parse("{}").expect("metadata"),
            )
            .expect("handle"),
            reconciliation: ReconciliationPolicy::CallbackOrPoll,
            next_poll_at: None,
            expires_at: None,
            output_contract: contract.clone(),
        };
        let timestamp = Timestamp::from_unix_ms(1_000).expect("timestamp");
        let completed = crate::EffectCompleted::try_new(
            effect_id,
            contract,
            RawJson::parse("{}").expect("output"),
            None,
            vec![],
            ProviderIds::empty(),
            Some("completion"),
            tampered.then(|| fixed_id::<crate::BudgetReservationTag>(4)),
        )
        .expect("shape-valid tampered completion");
        let message = Message::try_new(
            fixed_id::<crate::MessageTag>(5),
            MessageRole::Assistant,
            vec![ContentBlock::Text(
                TextBlock::try_new("hello").expect("text"),
            )],
            timestamp,
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let entry = EntryAppended {
            cycle: 0,
            turn_id,
            model_request_id,
            effect_id,
            parent_message_id: None,
            message,
        };
        let records = vec![
            envelope(1, 1, timestamp, RecordBody::EffectCompleted(completed)),
            envelope(2, 2, timestamp, RecordBody::EntryAppended(entry)),
        ];
        let batch = CommittedBatch::try_new(fixed_id::<crate::AppendBatchTag>(6), 1, 2, records)
            .expect("batch");
        let model_settlements = (0..crate::SEMANTIC_MAP_MAX_ENTRIES)
            .map(|ordinal| {
                let id =
                    fixed_id::<crate::EffectTag>(u64::try_from(ordinal + 100).expect("ordinal"));
                (
                    id,
                    ModelSettlementFingerprint {
                        kind: ModelSettlementKind::Completed,
                        digest: Digest::raw_json(format!("settlement-{ordinal}").as_bytes()),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let state = KernelState {
            phase: Some(RunPhase::AwaitingExternal),
            pending_model_effect: Some(PendingModelEffect {
                cycle: 0,
                turn_id,
                model_request_id,
                requested,
                deferred: Some(deferred),
            }),
            model_settlements,
            ..KernelState::default()
        };
        (state, batch)
    }

    fn envelope(
        record_ordinal: u64,
        event_ordinal: u64,
        timestamp: Timestamp,
        body: RecordBody,
    ) -> RecordEnvelope {
        RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            fixed_id::<crate::RecordTag>(record_ordinal),
            fixed_id::<crate::SessionTag>(10),
            fixed_id::<crate::LaneTag>(11),
            Some(fixed_id::<crate::RunTag>(12)),
            record_ordinal,
            timestamp,
            None,
            Digest::raw_json(b"payload"),
            None,
            Digest::raw_json(b"checksum"),
            vec![fixed_id::<crate::EventTag>(event_ordinal)],
            body,
        )
        .expect("record")
    }

    fn composition_envelope(
        sequence: u64,
        timestamp: Timestamp,
        session_id: crate::SessionId,
        lane_id: crate::LaneId,
        run_id: crate::RunId,
        body: RecordBody,
    ) -> RecordEnvelope {
        RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            fixed_id::<crate::RecordTag>(sequence + 100),
            session_id,
            lane_id,
            Some(run_id),
            sequence,
            timestamp,
            None,
            Digest::raw_json(b"payload"),
            None,
            Digest::raw_json(b"checksum"),
            vec![],
            body,
        )
        .expect("composition record")
    }

    fn fixed_id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }
}
