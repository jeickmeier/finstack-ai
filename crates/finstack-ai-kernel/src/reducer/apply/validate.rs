use std::collections::BTreeSet;

use crate::effects::EffectOutputKind;
use crate::records::{APPEND_BATCH_MAX_RECORDS, RecordBody, RecordEnvelope};
use crate::state::KernelState;

use super::super::decision::{CommittedBatch, KernelError};
use super::super::fingerprint;
use super::super::fingerprint::{
    completed_record_digest, failed_record_digest, opened_tool_batch_plan_digest,
    tool_batch_close_digest,
};
use super::shapes::is_foreign_run;

pub(super) fn validate_stage_digests(records: &[RecordEnvelope]) -> Result<(), KernelError> {
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

pub(super) fn validate_tool_digests(records: &[RecordEnvelope]) -> Result<(), KernelError> {
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

pub(super) fn validate_model_digests(
    state: &KernelState,
    records: &[RecordEnvelope],
) -> Result<(), KernelError> {
    if let Some(pending) = state.pending_model_effect.as_ref() {
        match records {
            [completed_record, entry_record] => {
                if let (RecordBody::EffectCompleted(completed), RecordBody::EntryAppended(entry)) =
                    (completed_record.body(), entry_record.body())
                    && completed.output_contract().kind == EffectOutputKind::ModelResponse
                {
                    completed_record_digest(pending, completed, &entry.message)?;
                }
            }
            [failed_record] => {
                if let RecordBody::EffectFailed(failed) = failed_record.body()
                    && failed.output_contract().kind == EffectOutputKind::ModelResponse
                {
                    failed_record_digest(pending, failed)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn validate_batch_range(
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

pub(super) fn validate_record_sequences(committed: &CommittedBatch) -> Result<(), KernelError> {
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

pub(super) fn validate_identities(
    state: &KernelState,
    records: &[RecordEnvelope],
) -> Result<(), KernelError> {
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
