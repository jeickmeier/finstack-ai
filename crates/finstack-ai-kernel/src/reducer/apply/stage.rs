use crate::content::ContentBlock;
use crate::records::lifecycle::{Stage, StageDisposition, StageOutcomeRecorded};
use crate::state::{KernelState, RunPhase, TerminalCandidate};

use super::super::decision::KernelError;

pub(super) fn apply_stage_outcome(
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
                .ok_or(KernelError::InvalidRecordOrder)?;
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
