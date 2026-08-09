//! Transactional committed-record validation and state derivation.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::capacity;
use super::decision::{CommittedBatch, KernelError};
use super::fingerprint;
use super::fingerprint::{completed_record_digest, failed_record_digest};
use super::{canonical_digest, failure_from_state};
use crate::content::ContentBlock;
use crate::digest::Digest;
use crate::effects::{EffectKind, EffectOutputKind};
use crate::entries::{
    ContextPrepared, EntryAppended, RunCompleted, RunFailed, Stage, StageCursor, StageDisposition,
    StageOutcomeRecorded,
};
use crate::events::RunEvent;
use crate::message::MessageRole;
use crate::records::{APPEND_BATCH_MAX_RECORDS, RecordBody, RecordEnvelope};
use crate::state::{
    CompletionIdentity, CurrentTurn, KernelState, ModelSettlementFingerprint, ModelSettlementKind,
    PendingModelEffect, RunPhase, TerminalCandidate, TerminalState,
};

struct AppliedRecords {
    state: KernelState,
    event_correlations: Vec<(Option<crate::TurnId>, Option<crate::ModelRequestId>)>,
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
    let applied = apply_semantic_records(original, &committed.records)?;
    capacity::preflight_batch(original, &committed.records)?;

    let mut events = Vec::new();
    for (record, (model_turn_id, model_request_id)) in
        committed.records.iter().zip(applied.event_correlations)
    {
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
                RunEvent::try_from_record_with_model_correlations(
                    record,
                    ordinal,
                    transient_sequence,
                    model_turn_id,
                    model_request_id,
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
        event_correlations.push(model_event_correlations(&state));
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

fn model_event_correlations(
    state: &KernelState,
) -> (Option<crate::TurnId>, Option<crate::ModelRequestId>) {
    let turn_id = state
        .pending_model_effect
        .as_ref()
        .map(|pending| pending.turn_id)
        .or_else(|| state.current_turn.as_ref().map(|turn| turn.turn_id));
    let model_request_id = state
        .pending_model_effect
        .as_ref()
        .map(|pending| pending.model_request_id)
        .or_else(|| {
            state
                .current_turn
                .as_ref()
                .and_then(|turn| turn.model_request_id)
        });
    (turn_id, model_request_id)
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
        Some(RunPhase::AwaitingExternal) => model_settlement_shape(state, records, false),
        Some(RunPhase::AfterModel) => {
            one_stage(records, state.cycle, Stage::AfterModel, |disposition| {
                matches!(
                    disposition,
                    StageDisposition::Continued | StageDisposition::Failed { .. }
                )
            })
        }
        Some(RunPhase::BeforeFinalize) => finalize_shape(state, records),
        Some(
            RunPhase::Accepted
            | RunPhase::BeforeToolBatch
            | RunPhase::AwaitingTools
            | RunPhase::AfterToolBatch
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
        && !entry
            .message
            .content()
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolCall(_)))
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

fn apply_effect_failed(
    state: &mut KernelState,
    failed: &crate::EffectFailed,
) -> Result<(), KernelError> {
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
                Stage::AfterModel => RunPhase::BeforeFinalize,
                _ => return Err(KernelError::InvalidRecordOrder),
            });
        }
        StageDisposition::ContextPrepared { .. } | StageDisposition::FinalizeAccepted => {}
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
