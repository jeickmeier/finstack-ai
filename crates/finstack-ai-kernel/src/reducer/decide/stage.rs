use crate::digest::Digest;
use crate::effects::{EffectInput, EffectKind, EffectOutputKind};
use crate::entries::{RetryClassification, RetryScheduled, Stage, StageCursor, StageDisposition};
use crate::records::{RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft};
use crate::state::{KernelState, TerminalCandidate, TransitionEnv};
use crate::{OutputConfiguration, OutputSpec, RetrySafety};

use super::super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::super::capacity::{self, StateGrowth};
use super::super::decision::{Decision, KernelError, PostCommitAction};
use super::super::fingerprint::stage_digest;
use super::super::input::{AcceptRun, ReducerStageOutcome, StageSettled};
use super::super::validation::validate_error_descriptor;
use super::bodies::{
    accepted_finalize_bodies, continued_model_bodies, failed_stage_bodies, prepared_context_bodies,
    requested_model_bodies,
};
use super::shared::{stage_record, terminal_body_from_candidate, timer_firing_contract};
use super::{
    draft_for_state, duplicate_decision, expected_stage_cursor, next_sequence, reject_terminal,
    required,
};
use crate::bounds::SEMANTIC_ARRAY_MAX_ITEMS;
use crate::content::TEXT_MAX_BYTES;

pub(super) fn decide_accept(
    state: &KernelState,
    env: &TransitionEnv,
    input: &AcceptRun,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    if state.phase.is_some() || state.accepted.is_some() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "accept_run",
        });
    }
    if input.accepted.run_id() != input.accepted.relation().root_run_id()
        && input.accepted.relation().parent_run_id().is_none()
    {
        return Err(KernelError::InvalidRunAcceptance);
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 1, 0, 0, 0, 0))?;
    let body = RecordBody::RunAccepted(input.accepted.clone());
    let record = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        required(env.ids.record_ids(), 0, "record_ids")?,
        input.session_id,
        input.lane_id,
        Some(input.accepted.run_id()),
        env.now,
        vec![required(env.ids.event_ids(), 0, "event_ids")?],
        body,
    )
    .map_err(|_| KernelError::InvalidRunAcceptance)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: vec![record],
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

pub(super) fn decide_stage(
    state: &KernelState,
    env: &TransitionEnv,
    input: &StageSettled,
    context_canonical: Option<(Digest, usize)>,
) -> Result<Decision, KernelError> {
    validate_stage_input(input)?;
    let settlement_digest = stage_digest(input)?;
    if let Some(existing) = state.stage_settlements.get(&input.cursor) {
        return if *existing == settlement_digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    reject_terminal(state)?;
    let expected = expected_stage_cursor(state).ok_or(KernelError::InvalidPhaseInput {
        phase: state.phase,
        input: "stage_settled",
    })?;
    if input.cursor != expected {
        return Err(KernelError::StageCursorMismatch {
            expected,
            actual: input.cursor,
        });
    }

    if matches!(input.outcome, ReducerStageOutcome::ToolBatchPrepared { .. }) {
        return super::super::tool::decide_batch_prepared(state, env, input, settlement_digest);
    }

    let requirements = stage_id_requirements(state, input)?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            stage: Some(input.cursor),
            ..StateGrowth::default()
        },
    )?;
    validate_allocated_ids(&env.ids, requirements)?;
    let (bodies, action) = stage_bodies(state, env, input, settlement_digest, context_canonical)?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: action.into_iter().collect(),
        diagnostics: Vec::new(),
    })
}

fn stage_id_requirements(
    state: &KernelState,
    input: &StageSettled,
) -> Result<IdRequirements, KernelError> {
    let cursor = input.cursor;
    match &input.outcome {
        ReducerStageOutcome::Continue
            if matches!(
                cursor.stage,
                Stage::BeforeRun | Stage::AfterModel | Stage::AfterToolBatch
            ) =>
        {
            if cursor.stage == Stage::AfterModel
                && matches!(
                    state.output_configuration,
                    Some(OutputConfiguration {
                        output: OutputSpec::JsonSchema { .. },
                        ..
                    })
                )
                && state.final_result.is_none()
                && state.validation_failure.is_none()
            {
                return Err(KernelError::InvalidPhaseInput {
                    phase: state.phase,
                    input: "stage_settled",
                });
            }
            Ok(IdRequirements::new(1, 0, 0, 0, 0, 0))
        }
        ReducerStageOutcome::ContextPrepared { .. } if cursor.stage == Stage::PrepareContext => {
            Ok(IdRequirements::new(2, 0, 0, 1, 0, 0))
        }
        ReducerStageOutcome::ModelRequestPrepared {
            output_contract, ..
        } if cursor.stage == Stage::BeforeModel => {
            if output_contract.kind != EffectOutputKind::ModelResponse {
                return Err(KernelError::ModelRequestContractMismatch);
            }
            Ok(IdRequirements::new(2, 1, 1, 0, 1, 0))
        }
        ReducerStageOutcome::FinalizeAccepted if cursor.stage == Stage::BeforeFinalize => {
            terminal_body_from_candidate(state)?;
            Ok(IdRequirements::new(2, 1, 0, 0, 0, 0))
        }
        ReducerStageOutcome::ContinueModel { .. }
            if cursor.stage == Stage::BeforeFinalize
                && matches!(
                    state.terminal_candidate,
                    Some(TerminalCandidate::Completed { .. })
                ) =>
        {
            state
                .cycle
                .checked_add(1)
                .ok_or(KernelError::CycleOverflow)?;
            Ok(IdRequirements::new(1, 0, 0, 0, 0, 0))
        }
        ReducerStageOutcome::Fail(_) if cursor.stage == Stage::BeforeFinalize => {
            Ok(IdRequirements::new(2, 1, 0, 0, 0, 0))
        }
        ReducerStageOutcome::Retry(_) if cursor.stage == Stage::BeforeFinalize => {
            Ok(IdRequirements::new(3, 1, 1, 0, 0, 0))
        }
        ReducerStageOutcome::Fail(_)
            if matches!(
                cursor.stage,
                Stage::BeforeRun
                    | Stage::PrepareContext
                    | Stage::BeforeModel
                    | Stage::AfterModel
                    | Stage::BeforeToolBatch
                    | Stage::AfterToolBatch
            ) =>
        {
            Ok(IdRequirements::new(1, 0, 0, 0, 0, 0))
        }
        _ => Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        }),
    }
}

fn validate_stage_input(input: &StageSettled) -> Result<(), KernelError> {
    match &input.outcome {
        ReducerStageOutcome::ContextPrepared { messages }
            if messages.len() > SEMANTIC_ARRAY_MAX_ITEMS =>
        {
            Err(KernelError::InvalidInputPayload {
                field: "messages",
                reason_code: "too_many_items",
            })
        }
        ReducerStageOutcome::ContinueModel {
            reason: Some(reason),
        } if reason.is_empty()
            || reason.len() > TEXT_MAX_BYTES
            || reason.as_bytes().contains(&0) =>
        {
            Err(KernelError::InvalidInputPayload {
                field: "reason",
                reason_code: "invalid_text",
            })
        }
        ReducerStageOutcome::Fail(error) => validate_error_descriptor(error),
        _ => Ok(()),
    }
}

fn stage_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    input: &StageSettled,
    settlement_digest: Digest,
    context_canonical: Option<(Digest, usize)>,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let cursor = input.cursor;
    match &input.outcome {
        ReducerStageOutcome::Continue
            if matches!(
                cursor.stage,
                Stage::BeforeRun | Stage::AfterModel | Stage::AfterToolBatch
            ) =>
        {
            Ok((
                vec![stage_record(
                    cursor,
                    StageDisposition::Continued,
                    settlement_digest,
                )],
                None,
            ))
        }
        ReducerStageOutcome::ContextPrepared { messages }
            if cursor.stage == Stage::PrepareContext =>
        {
            prepared_context_bodies(
                env,
                cursor,
                messages,
                settlement_digest,
                context_canonical.ok_or(KernelError::InvariantViolation)?.0,
            )
        }
        ReducerStageOutcome::ModelRequestPrepared {
            request: _,
            component: _,
            output_contract: _,
            retry_safety: _,
            deadline: _,
        } if cursor.stage == Stage::BeforeModel => {
            requested_model_bodies(state, env, cursor, settlement_digest, &input.outcome)
        }
        ReducerStageOutcome::FinalizeAccepted if cursor.stage == Stage::BeforeFinalize => {
            accepted_finalize_bodies(state, cursor, settlement_digest)
        }
        ReducerStageOutcome::ContinueModel { .. }
            if cursor.stage == Stage::BeforeFinalize
                && matches!(
                    state.terminal_candidate,
                    Some(TerminalCandidate::Completed { .. })
                ) =>
        {
            continued_model_bodies(state, cursor, settlement_digest)
        }
        ReducerStageOutcome::Fail(error) => {
            Ok(failed_stage_bodies(state, cursor, settlement_digest, error))
        }
        ReducerStageOutcome::Retry(directive) if cursor.stage == Stage::BeforeFinalize => {
            retry_bodies(state, env, cursor, settlement_digest, directive)
        }
        _ => Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        }),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "retry admission and its atomic timer-intent record batch are one fail-closed transition"
)]
fn retry_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    cursor: StageCursor,
    settlement_digest: Digest,
    directive: &crate::RetryDirective,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    if directive.classification == RetryClassification::Validation
        && state.validation_failure.is_none()
    {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    let TerminalCandidate::Failed { error, .. } =
        state
            .terminal_candidate
            .as_ref()
            .ok_or(KernelError::InvalidPhaseInput {
                phase: state.phase,
                input: "stage_settled",
            })?
    else {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    };
    if state.validation_failure.is_some()
        && directive.classification != RetryClassification::Validation
    {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    if !error.retryable {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    let attempt = state
        .retry
        .attempts
        .checked_add(1)
        .ok_or(KernelError::InvalidInputPayload {
            field: "retry.attempt",
            reason_code: "overflow",
        })?;
    if state
        .accepted
        .as_ref()
        .and_then(|accepted| accepted.limits().max_retries)
        .is_some_and(|maximum| attempt > maximum)
    {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "retry_limit_reached",
        });
    }
    let due_at =
        env.now
            .checked_add(directive.backoff)
            .map_err(|_| KernelError::InvalidInputPayload {
                field: "retry.backoff",
                reason_code: "overflow",
            })?;
    let timer_effect_id = required(env.ids.effect_ids(), 0, "effect_ids")?;
    let retry = RetryScheduled::try_new(
        state.cycle,
        attempt,
        directive.classification,
        directive.policy_version.as_ref(),
        timer_effect_id,
        due_at,
        error.clone(),
    )
    .map_err(|_| KernelError::InvalidInputPayload {
        field: "retry",
        reason_code: "invalid",
    })?;
    let requested = crate::EffectRequested::try_new(
        timer_effect_id,
        EffectKind::Timer,
        None,
        None,
        None,
        timer_firing_contract(),
        EffectInput::Timer { due_at },
        RetrySafety::IdempotentWithKey,
        Some(due_at),
    )
    .map_err(|_| KernelError::InvariantViolation)?;
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::RetryScheduled {
                    attempt,
                    timer_effect_id,
                    due_at,
                },
                settlement_digest,
            ),
            RecordBody::RetryScheduled(retry),
            RecordBody::EffectRequested(requested),
        ],
        Some(PostCommitAction::ExecuteEffect {
            effect_id: timer_effect_id,
        }),
    ))
}
