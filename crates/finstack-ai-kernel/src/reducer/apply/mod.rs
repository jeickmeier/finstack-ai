//! Transactional committed-record validation and state derivation.

mod effects;
mod interactions;
mod output;
mod record;
mod shapes;
mod stage;
mod tools;
mod validate;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::capacity;
use super::decision::{CommittedBatch, KernelError};
use crate::events::{EventCorrelations, RunEvent};
use crate::records::{RecordBody, RecordEnvelope};
use crate::state::{KernelState, RunPhase};

use record::apply_record;
use shapes::{foreign_run_shape, validate_batch_shape};
use validate::{
    validate_batch_range, validate_identities, validate_model_digests, validate_record_sequences,
    validate_stage_digests, validate_tool_digests,
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
    validate_model_digests(original, &committed.records)?;
    capacity::preflight_batch(original, &committed.records)?;
    // Working-copy apply after preflight. Per-record checks stay in apply_*;
    // full-state validate is reserved for try_restore / deserialize / state_hash.
    let applied = apply_semantic_records(original, &committed.records)?;

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
        })
        .or_else(|| match body {
            RecordBody::EffectRequested(requested) if requested.is_runtime_owned_child_model() => {
                Some(crate::ModelRequestId::from_bytes(
                    *requested.effect_id().as_bytes(),
                ))
            }
            _ => None,
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
        batch.call(effect_id).map(|call| {
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
