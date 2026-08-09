//! Transactional committed-record validation and state derivation.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::capacity;
use super::decision::{CommittedBatch, KernelError};
use super::fingerprint;
use super::fingerprint::{
    completed_record_digest, completed_tool_record_digest, failed_record_digest,
    failed_tool_record_digest, opened_tool_batch_plan_digest, synthetic_tool_digest,
    tool_batch_close_digest,
};
use super::{canonical_digest, failure_from_state};
use crate::content::ContentBlock;
use crate::digest::Digest;
use crate::effects::{EffectKind, EffectOutputKind};
use crate::entries::{
    ContextPrepared, EntryAppended, RunCompleted, RunFailed, Stage, StageCursor, StageDisposition,
    StageOutcomeRecorded,
};
use crate::events::{EventCorrelations, RunEvent};
use crate::message::MessageRole;
use crate::records::{APPEND_BATCH_MAX_RECORDS, RecordBody, RecordEnvelope};
use crate::state::{
    CompletionIdentity, CurrentTurn, KernelState, ModelSettlementFingerprint, ModelSettlementKind,
    PendingModelEffect, RunPhase, TerminalCandidate, TerminalState,
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
    if original.terminal.is_some()
        || matches!(
            original.phase,
            Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled)
        )
    {
        return Err(KernelError::TerminalStateImmutable);
    }
    validate_identities(original, &committed.records)?;
    validate_batch_shape(original, &committed.records)?;
    validate_stage_digests(&committed.records)?;
    validate_tool_digests(&committed.records)?;
    let applied = apply_semantic_records(original, &committed.records)?;
    capacity::preflight_batch(original, &committed.records)?;
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
                .map_err(|_| KernelError::InvariantViolation)?,
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
        if let (Some(session_id), Some(lane_id), Some(accepted)) =
            (state.session_id, state.lane_id, state.accepted.as_ref())
            && (record.session_id() != session_id
                || record.lane_id() != lane_id
                || record.run_id() != Some(accepted.run_id()))
        {
            return Err(KernelError::RecordIdentityMismatch);
        }
    }
    Ok(())
}

fn validate_batch_shape(
    state: &KernelState,
    records: &[RecordEnvelope],
) -> Result<(), KernelError> {
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
            one_stage(records, state.cycle, Stage::BeforeRun, |disposition| {
                matches!(
                    disposition,
                    StageDisposition::Continued | StageDisposition::Failed { .. }
                )
            })
        }
        Some(RunPhase::PreparingContext) => {
            prepare_context_shape(records, state.cycle)
                || one_failed_stage(records, state.cycle, Stage::PrepareContext)
        }
        Some(RunPhase::BeforeModel) => {
            request_model_shape(state, records)
                || one_failed_stage(records, state.cycle, Stage::BeforeModel)
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
            one_stage(records, state.cycle, Stage::AfterModel, |disposition| {
                matches!(
                    disposition,
                    StageDisposition::Continued | StageDisposition::Failed { .. }
                )
            })
        }
        Some(RunPhase::BeforeToolBatch) => {
            tool_batch_open_shape(state, records)
                || one_failed_stage(records, state.cycle, Stage::BeforeToolBatch)
        }
        Some(RunPhase::AwaitingTools) => tool_settlement_shape(state, records, true),
        Some(RunPhase::AfterToolBatch) => {
            one_stage(records, state.cycle, Stage::AfterToolBatch, |disposition| {
                matches!(
                    disposition,
                    StageDisposition::Continued | StageDisposition::Failed { .. }
                )
            })
        }
        Some(RunPhase::BeforeFinalize) => finalize_shape(state, records),
        Some(
            RunPhase::Accepted
            | RunPhase::AwaitingInteraction
            | RunPhase::Sleeping
            | RunPhase::Cancelling
            | RunPhase::Suspended,
        ) => false,
        Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled) => {
            return Err(KernelError::TerminalStateImmutable);
        }
    };
    if valid {
        Ok(())
    } else {
        Err(KernelError::InvalidRecordOrder)
    }
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

fn apply_record(
    state: &mut KernelState,
    record: &RecordEnvelope,
    next: Option<&RecordBody>,
) -> Result<(), KernelError> {
    match record.body() {
        RecordBody::RunAccepted(accepted) => {
            state.session_id = Some(record.session_id());
            state.lane_id = Some(record.lane_id());
            state.accepted = Some(accepted.clone());
            state.phase = Some(RunPhase::Accepted);
        }
        RecordBody::StageOutcomeRecorded(outcome) => apply_stage_outcome(state, outcome)?,
        RecordBody::ContextPrepared(context) => {
            if !context_digest_matches(context) {
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
        }
        RecordBody::EffectRequested(requested) => {
            apply_effect_requested(state, requested)?;
        }
        RecordBody::EffectDeferred(deferred) => {
            apply_effect_deferred(state, deferred)?;
        }
        RecordBody::EffectCompleted(completed) => {
            apply_effect_completed(state, completed, next)?;
        }
        RecordBody::EntryAppended(entry) => {
            apply_entry_appended(state, entry)?;
        }
        RecordBody::ToolBatchOpened(opened) => {
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
        }
        RecordBody::EffectCancelled(_)
        | RecordBody::InteractionRequested(_)
        | RecordBody::InteractionResolved(_)
        | RecordBody::InteractionExpired(_)
        | RecordBody::InteractionCancelled(_) => {
            return Err(KernelError::InvalidRecordOrder);
        }
    }
    Ok(())
}

fn apply_effect_requested(
    state: &mut KernelState,
    requested: &crate::EffectRequested,
) -> Result<(), KernelError> {
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
        if let ContentBlock::ToolCall(call) = block {
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
        state.state_version = 2;
    }
    let mut messages = state.messages.to_vec();
    messages.push(entry.message.clone());
    state.messages = messages.into();
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
            ContentBlock::ToolCall(call) => Some(call.clone()),
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
    let mut messages = state.messages.to_vec();
    messages.push(settled.message.clone());
    state.messages = messages.into();
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
                    let has_tools = state.messages.last().is_some_and(|message| {
                        message
                            .content()
                            .iter()
                            .any(|block| matches!(block, ContentBlock::ToolCall(_)))
                    });
                    if has_tools {
                        RunPhase::BeforeToolBatch
                    } else {
                        RunPhase::BeforeFinalize
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
        | StageDisposition::FinalizeAccepted => {}
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

fn context_digest_matches(context: &ContextPrepared) -> bool {
    canonical_digest("model-context", &context.messages.as_ref())
        .is_ok_and(|digest| digest == context.context_digest)
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
    fn semantic_tampering_precedes_capacity_failure() {
        let (state, batch) = external_batch_at_capacity(true);
        assert_eq!(
            apply(&state, &batch, 0),
            Err(KernelError::ModelSettlementMismatch)
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

    fn fixed_id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }
}
