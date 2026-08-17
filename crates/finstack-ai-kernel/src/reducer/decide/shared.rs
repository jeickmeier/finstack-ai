use crate::EffectOutputContract;
use crate::effects::EffectOutputKind;
use crate::primitives::Digest;
use crate::primitives::{Diagnostic, DiagnosticSeverity};
use crate::records::lifecycle::{
    RunCompleted, RunFailed, Stage, StageCursor, StageDisposition, StageOutcomeRecorded,
};
use crate::records::{RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft};
use crate::state::{KernelState, RunPhase, TerminalCandidate, TransitionEnv};

use super::super::decision::{Decision, KernelError};

pub fn outstanding_requested_effects(state: &KernelState) -> Vec<crate::EffectId> {
    let mut effects = state
        .pending_model_effect
        .as_ref()
        .map(|pending| vec![pending.requested.effect_id()])
        .unwrap_or_default();
    if let Some(batch) = &state.active_tool_batch {
        effects.extend(batch.calls.iter().filter_map(|call| match call.status {
            crate::ActiveToolCallStatus::Requested { .. } => Some(call.assigned.effect_id),
            _ => None,
        }));
    }
    if let Some(pending) = &state.pending_interaction {
        effects.push(pending.request.effect_id());
    }
    if let Some(pending) = &state.retry.pending {
        effects.push(pending.timer_effect_id);
    }
    effects.sort_unstable();
    effects.dedup();
    effects
}

pub(super) fn timer_firing_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::TimerFiring,
        schema_version: 1,
        schema_digest: Digest::effect_output(br#"{"type":"timer_firing"}"#),
    }
}

pub fn draft_for_state(
    state: &KernelState,
    env: &TransitionEnv,
    bodies: Vec<RecordBody>,
) -> Result<Vec<RecordDraft>, KernelError> {
    let session_id = state.session_id.ok_or(KernelError::InvariantViolation)?;
    let lane_id = state.lane_id.ok_or(KernelError::InvariantViolation)?;
    let run_id = state
        .accepted
        .as_ref()
        .map(crate::records::run::RunAccepted::run_id)
        .ok_or(KernelError::InvariantViolation)?;
    let mut event_index = 0;
    let mut records = Vec::with_capacity(bodies.len());
    for (record_index, body) in bodies.into_iter().enumerate() {
        let count = body
            .derived_event_count(RECORD_KIND_VERSION)
            .map_err(|_| KernelError::InvariantViolation)?;
        let mut event_ids = Vec::with_capacity(count);
        for _ in 0..count {
            event_ids.push(required(env.ids.event_ids(), event_index, "event_ids")?);
            event_index += 1;
        }
        records.push(
            RecordDraft::try_new(
                RECORD_FORMAT_VERSION,
                RECORD_KIND_VERSION,
                required(env.ids.record_ids(), record_index, "record_ids")?,
                session_id,
                lane_id,
                Some(run_id),
                env.now,
                event_ids,
                body,
            )
            .map_err(|_| KernelError::InvariantViolation)?,
        );
    }
    Ok(records)
}

pub(super) fn stage_record(
    cursor: StageCursor,
    disposition: StageDisposition,
    settlement_digest: Digest,
) -> RecordBody {
    RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
        cursor,
        disposition,
        settlement_digest,
    })
}

pub(super) fn terminal_body_from_candidate(state: &KernelState) -> Result<RecordBody, KernelError> {
    match state
        .terminal_candidate
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        })? {
        TerminalCandidate::Completed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            result_digest,
        } => Ok(RecordBody::RunCompleted(RunCompleted {
            cycle: *cycle,
            turn_id: *turn_id,
            model_request_id: *model_request_id,
            effect_id: *effect_id,
            result_message_id: *message_id,
            result_digest: *result_digest,
        })),
        TerminalCandidate::Failed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            error,
        } => Ok(RecordBody::RunFailed(RunFailed {
            cycle: *cycle,
            turn_id: *turn_id,
            model_request_id: *model_request_id,
            effect_id: *effect_id,
            error: error.clone(),
        })),
    }
}

pub fn expected_stage_cursor(state: &KernelState) -> Option<StageCursor> {
    let expected_stage = match state.phase? {
        RunPhase::BeforeRun => Stage::BeforeRun,
        RunPhase::PreparingContext => Stage::PrepareContext,
        RunPhase::BeforeModel => Stage::BeforeModel,
        RunPhase::AfterModel => Stage::AfterModel,
        RunPhase::BeforeToolBatch => Stage::BeforeToolBatch,
        RunPhase::AfterToolBatch => Stage::AfterToolBatch,
        RunPhase::BeforeFinalize => Stage::BeforeFinalize,
        _ => return None,
    };
    Some(StageCursor {
        cycle: state.cycle,
        stage: expected_stage,
    })
}

pub fn duplicate_decision(state: &KernelState) -> Result<Decision, KernelError> {
    let diagnostic = Diagnostic::try_new(
        "duplicate_settlement",
        "equal committed settlement was ignored",
        DiagnosticSeverity::Info,
        crate::Metadata::empty(),
    )
    .map_err(|_| KernelError::InvariantViolation)?;
    Ok(Decision::duplicate(next_sequence(state)?, diagnostic))
}

pub fn reject_terminal(state: &KernelState) -> Result<(), KernelError> {
    if matches!(
        state.phase,
        Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled)
    ) || state.terminal.is_some()
    {
        return Err(KernelError::TerminalStateImmutable);
    }
    Ok(())
}

pub fn next_sequence(state: &KernelState) -> Result<u64, KernelError> {
    state
        .last_applied_sequence
        .checked_add(1)
        .ok_or(KernelError::InvariantViolation)
}

pub fn required<T: Copy>(values: &[T], index: usize, kind: &'static str) -> Result<T, KernelError> {
    values
        .get(index)
        .copied()
        .ok_or(KernelError::AllocatedIdsExhausted { kind })
}
