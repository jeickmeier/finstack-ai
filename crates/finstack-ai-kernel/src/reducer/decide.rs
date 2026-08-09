//! Pure PR-009 transition decisions.

use std::sync::Arc;

use super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::capacity::{self, StateGrowth};
use super::decision::{Decision, KernelError, PostCommitAction};
use super::failure_from_state;
use super::fingerprint::{direct_digest, external_digest, stage_digest};
use super::input::{
    AcceptRun, ExternalEffectCompletedInput, ExternalEffectOutcome, KernelInput, ModelSettled,
    ModelSettlement, ReducerStageOutcome, StageSettled,
};
use super::validation::{
    validate_assistant_message_id, validate_assistant_semantics, validate_completion_identity,
    validate_error_descriptor,
};
use crate::bounds::SEMANTIC_ARRAY_MAX_ITEMS;
use crate::content::{LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::digest::Digest;
use crate::effects::{
    EffectCompleted, EffectFailed, EffectInput, EffectKind, EffectOutputKind, EffectRequested,
};
use crate::entries::{
    ContextPrepared, EntryAppended, RunCompleted, RunFailed, Stage, StageCursor, StageDisposition,
    StageOutcomeRecorded,
};
use crate::records::{RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft};
use crate::refs::{Diagnostic, DiagnosticSeverity};
use crate::state::{KernelState, RunPhase, TerminalCandidate, TransitionEnv};

pub(super) fn decide(
    state: &KernelState,
    env: &TransitionEnv,
    input: KernelInput,
) -> Result<Decision, KernelError> {
    match input {
        KernelInput::AcceptRun(input) => decide_accept(state, env, &input),
        KernelInput::StageSettled(input) => decide_stage(state, env, &input),
        KernelInput::ModelSettled(input) => decide_model(state, env, &input),
        KernelInput::ExternalEffectCompleted(input) => decide_external(state, env, input),
    }
}

fn decide_accept(
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

fn decide_stage(
    state: &KernelState,
    env: &TransitionEnv,
    input: &StageSettled,
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

    let requirements = stage_id_requirements(state, input)?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            stage: Some(input.cursor),
            ..StateGrowth::default()
        },
    )?;
    validate_allocated_ids(&env.ids, requirements)?;
    let (bodies, action) = stage_bodies(state, env, input, settlement_digest)?;
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
            if matches!(cursor.stage, Stage::BeforeRun | Stage::AfterModel) =>
        {
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
        ReducerStageOutcome::Fail(_)
            if matches!(
                cursor.stage,
                Stage::BeforeRun | Stage::PrepareContext | Stage::BeforeModel | Stage::AfterModel
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
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let cursor = input.cursor;
    match &input.outcome {
        ReducerStageOutcome::Continue
            if matches!(cursor.stage, Stage::BeforeRun | Stage::AfterModel) =>
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
            prepared_context_bodies(env, cursor, messages, settlement_digest)
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
            failed_stage_bodies(state, cursor, settlement_digest, error)
        }
        _ => Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        }),
    }
}

fn prepared_context_bodies(
    env: &TransitionEnv,
    cursor: StageCursor,
    messages: &Arc<[crate::Message]>,
    settlement_digest: Digest,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let turn_id = required(env.ids.turn_ids(), 0, "turn_ids")?;
    let context = ContextPrepared::try_from_messages(cursor.cycle, turn_id, messages.to_vec())
        .map_err(|_| KernelError::ContextDigestMismatch)?;
    let context_digest = context.context_digest;
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::ContextPrepared {
                    turn_id,
                    context_digest,
                },
                settlement_digest,
            ),
            RecordBody::ContextPrepared(context),
        ],
        None,
    ))
}

fn requested_model_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    cursor: StageCursor,
    settlement_digest: Digest,
    outcome: &ReducerStageOutcome,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let ReducerStageOutcome::ModelRequestPrepared {
        request,
        component,
        output_contract,
        retry_safety,
        deadline,
    } = outcome
    else {
        return Err(KernelError::InvariantViolation);
    };
    if output_contract.kind != EffectOutputKind::ModelResponse {
        return Err(KernelError::ModelRequestContractMismatch);
    }
    let turn_id = state
        .current_turn
        .as_ref()
        .map(|turn| turn.turn_id)
        .ok_or(KernelError::InvariantViolation)?;
    let model_request_id = required(env.ids.model_request_ids(), 0, "model_request_ids")?;
    let effect_id = required(env.ids.effect_ids(), 0, "effect_ids")?;
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Model,
        None,
        component.clone(),
        None,
        output_contract.clone(),
        EffectInput::Model {
            request: request.clone(),
        },
        *retry_safety,
        *deadline,
    )
    .map_err(|_| KernelError::ModelRequestContractMismatch)?;
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::ModelRequested {
                    turn_id,
                    model_request_id,
                    effect_id,
                },
                settlement_digest,
            ),
            RecordBody::EffectRequested(requested),
        ],
        Some(PostCommitAction::ExecuteEffect { effect_id }),
    ))
}

fn accepted_finalize_bodies(
    state: &KernelState,
    cursor: StageCursor,
    settlement_digest: Digest,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let terminal = terminal_body_from_candidate(state)?;
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::FinalizeAccepted,
                settlement_digest,
            ),
            terminal,
        ],
        None,
    ))
}

fn continued_model_bodies(
    state: &KernelState,
    cursor: StageCursor,
    settlement_digest: Digest,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let next_cycle = state
        .cycle
        .checked_add(1)
        .ok_or(KernelError::CycleOverflow)?;
    Ok((
        vec![stage_record(
            cursor,
            StageDisposition::ContinueModel { next_cycle },
            settlement_digest,
        )],
        None,
    ))
}

fn failed_stage_bodies(
    state: &KernelState,
    cursor: StageCursor,
    settlement_digest: Digest,
    error: &crate::ErrorDescriptor,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    match cursor.stage {
        Stage::BeforeRun | Stage::PrepareContext | Stage::BeforeModel | Stage::AfterModel => Ok((
            vec![stage_record(
                cursor,
                StageDisposition::Failed {
                    error: error.clone(),
                },
                settlement_digest,
            )],
            None,
        )),
        Stage::BeforeFinalize => Ok((
            vec![
                stage_record(
                    cursor,
                    StageDisposition::Failed {
                        error: error.clone(),
                    },
                    settlement_digest,
                ),
                RecordBody::RunFailed(failure_from_state(state, error.clone())),
            ],
            None,
        )),
        Stage::BeforeToolBatch | Stage::AfterToolBatch => Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        }),
    }
}

fn decide_model(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ModelSettled,
) -> Result<Decision, KernelError> {
    if let ModelSettlement::Failed(failed) = &input.outcome {
        validate_error_descriptor(failed.error())?;
    }
    let settlement_digest = direct_digest(input)?;
    decide_normalized_model(state, env, input, "model_settled", settlement_digest)
}

fn validate_external_completion_input(
    input: &ExternalEffectCompletedInput,
) -> Result<(), KernelError> {
    if input.completion.completion_id.is_empty()
        || input.completion.completion_id.len() > LABEL_MAX_BYTES
        || input.completion.completion_id.as_bytes().contains(&0)
    {
        return Err(KernelError::InvalidInputPayload {
            field: "completion_id",
            reason_code: "invalid_label",
        });
    }
    if matches!(
        &input.completion.outcome,
        ExternalEffectOutcome::Completed { artifacts, .. }
            if artifacts.len() > SEMANTIC_ARRAY_MAX_ITEMS
    ) {
        return Err(KernelError::InvalidInputPayload {
            field: "artifacts",
            reason_code: "too_many_items",
        });
    }
    if let ExternalEffectOutcome::Failed { error } = &input.completion.outcome {
        validate_error_descriptor(error)?;
    }
    Ok(())
}

fn decide_external(
    state: &KernelState,
    env: &TransitionEnv,
    input: ExternalEffectCompletedInput,
) -> Result<Decision, KernelError> {
    validate_external_completion_input(&input)?;
    let settlement_digest = external_digest(&input)?;
    if let Some(decision) = classify_model_duplicate(
        state,
        Some(input.completion.completion_id.as_ref()),
        input.completion.effect_id,
        settlement_digest,
        None,
    )? {
        return Ok(decision);
    }
    if state.phase != Some(RunPhase::AwaitingExternal) {
        reject_terminal(state)?;
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "external_effect_completed",
        });
    }
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    if pending.requested.effect_id() != input.completion.effect_id {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    }
    if pending.deferred.is_none() {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    }
    let completion_id = input.completion.completion_id;
    let outcome = match input.completion.outcome {
        ExternalEffectOutcome::Completed {
            output,
            usage,
            artifacts,
        } => {
            let assistant_message = input
                .assistant_message
                .ok_or(KernelError::AssistantMessagePresenceMismatch)?;
            let completion = EffectCompleted::try_new(
                pending.requested.effect_id(),
                pending.requested.output_contract().clone(),
                output,
                usage,
                artifacts.to_vec(),
                assistant_message.provider_ids().clone(),
                Some(completion_id.as_ref()),
                None,
            )
            .map_err(|_| KernelError::ModelSettlementMismatch)?;
            ModelSettlement::Completed {
                completion,
                assistant_message,
            }
        }
        ExternalEffectOutcome::Failed { error } => {
            if input.assistant_message.is_some() {
                return Err(KernelError::AssistantMessagePresenceMismatch);
            }
            ModelSettlement::Failed(
                EffectFailed::try_new(
                    pending.requested.effect_id(),
                    pending.requested.output_contract().clone(),
                    error,
                    None,
                    Some(completion_id.as_ref()),
                )
                .map_err(|_| KernelError::ModelSettlementMismatch)?,
            )
        }
    };
    let normalized = ModelSettled {
        turn_id: pending.turn_id,
        model_request_id: pending.model_request_id,
        outcome,
    };
    decide_normalized_model(
        state,
        env,
        &normalized,
        "external_effect_completed",
        settlement_digest,
    )
}

fn decide_normalized_model(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ModelSettled,
    input_name: &'static str,
    settlement_digest: Digest,
) -> Result<Decision, KernelError> {
    let effect_id = settlement_effect_id(&input.outcome);
    if let Some(decision) = classify_model_duplicate(
        state,
        settlement_completion_id(&input.outcome),
        effect_id,
        settlement_digest,
        Some(input),
    )? {
        return Ok(decision);
    }
    reject_terminal(state)?;
    let required_phase = match input_name {
        "model_settled" => RunPhase::AwaitingModel,
        "external_effect_completed" => RunPhase::AwaitingExternal,
        _ => return Err(KernelError::InvariantViolation),
    };
    if state.phase != Some(required_phase) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: input_name,
        });
    }
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    if pending.turn_id != input.turn_id
        || pending.model_request_id != input.model_request_id
        || pending.requested.effect_id() != effect_id
    {
        return Err(KernelError::ModelSettlementMismatch);
    }
    if pending.requested.kind() != EffectKind::Model {
        return Err(KernelError::ModelSettlementMismatch);
    }

    let (requirements, bodies) =
        model_settlement_bodies(state, env, pending, input, effect_id, settlement_digest)?;
    validate_allocated_ids(&env.ids, requirements)?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn classify_model_duplicate(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
    input: Option<&ModelSettled>,
) -> Result<Option<Decision>, KernelError> {
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
    {
        return if existing.effect_id == effect_id && existing.settlement_digest == settlement_digest
        {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.model_settlements.get(&effect_id) {
        return if existing.digest == settlement_digest {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if let Some(ModelSettled {
        outcome: ModelSettlement::Deferred(deferred),
        ..
    }) = input
        && let Some(pending) = state.pending_model_effect.as_ref()
        && pending.requested.effect_id() == deferred.effect_id
        && let Some(existing) = pending.deferred.as_ref()
    {
        let existing_digest = direct_digest(&ModelSettled {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            outcome: ModelSettlement::Deferred(existing.clone()),
        })?;
        return if existing_digest == settlement_digest {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    Ok(None)
}

fn model_settlement_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    pending: &crate::PendingModelEffect,
    input: &ModelSettled,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    match &input.outcome {
        ModelSettlement::Completed {
            completion,
            assistant_message,
        } => {
            completion
                .validate_against(&pending.requested)
                .map_err(|_| KernelError::ModelSettlementMismatch)?;
            if completion.output_contract().kind != EffectOutputKind::ModelResponse {
                return Err(KernelError::ModelSettlementMismatch);
            }
            validate_completion_identity(
                state,
                completion.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            validate_assistant_semantics(env, assistant_message, completion)?;
            capacity::preflight_decision(
                state,
                StateGrowth {
                    messages: 1,
                    model: Some(effect_id),
                    completion: completion.completion_id(),
                    ..StateGrowth::default()
                },
            )?;
            let requirements = IdRequirements::new(2, 2, 0, 0, 0, 1);
            validate_allocated_ids(&env.ids, requirements)?;
            let message_id = required(env.ids.message_ids(), 0, "message_ids")?;
            validate_assistant_message_id(message_id, assistant_message)?;
            let parent_message_id = state.messages.last().map(|message| *message.id());
            Ok((
                requirements,
                vec![
                    RecordBody::EffectCompleted(completion.clone()),
                    RecordBody::EntryAppended(EntryAppended {
                        cycle: pending.cycle,
                        turn_id: pending.turn_id,
                        model_request_id: pending.model_request_id,
                        effect_id,
                        parent_message_id,
                        message: assistant_message.clone(),
                    }),
                ],
            ))
        }
        ModelSettlement::Deferred(deferred) => {
            deferred
                .validate_against(&pending.requested)
                .map_err(|_| KernelError::ModelSettlementMismatch)?;
            Ok((
                IdRequirements::new(1, 1, 0, 0, 0, 0),
                vec![RecordBody::EffectDeferred(deferred.clone())],
            ))
        }
        ModelSettlement::Failed(failed) => {
            failed
                .validate_against(&pending.requested)
                .map_err(|_| KernelError::ModelSettlementMismatch)?;
            validate_completion_identity(
                state,
                failed.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            capacity::preflight_decision(
                state,
                StateGrowth {
                    model: Some(effect_id),
                    completion: failed.completion_id(),
                    ..StateGrowth::default()
                },
            )?;
            Ok((
                IdRequirements::new(1, 1, 0, 0, 0, 0),
                vec![RecordBody::EffectFailed(failed.clone())],
            ))
        }
    }
}

fn draft_for_state(
    state: &KernelState,
    env: &TransitionEnv,
    bodies: Vec<RecordBody>,
) -> Result<Vec<RecordDraft>, KernelError> {
    let session_id = state.session_id.ok_or(KernelError::InvariantViolation)?;
    let lane_id = state.lane_id.ok_or(KernelError::InvariantViolation)?;
    let run_id = state
        .accepted
        .as_ref()
        .map(crate::run::RunAccepted::run_id)
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

fn stage_record(
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

fn terminal_body_from_candidate(state: &KernelState) -> Result<RecordBody, KernelError> {
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

fn expected_stage_cursor(state: &KernelState) -> Option<StageCursor> {
    let expected_stage = match state.phase? {
        RunPhase::BeforeRun => Stage::BeforeRun,
        RunPhase::PreparingContext => Stage::PrepareContext,
        RunPhase::BeforeModel => Stage::BeforeModel,
        RunPhase::AfterModel => Stage::AfterModel,
        RunPhase::BeforeFinalize => Stage::BeforeFinalize,
        _ => return None,
    };
    Some(StageCursor {
        cycle: state.cycle,
        stage: expected_stage,
    })
}

fn settlement_effect_id(outcome: &ModelSettlement) -> crate::EffectId {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.effect_id(),
        ModelSettlement::Deferred(deferred) => deferred.effect_id,
        ModelSettlement::Failed(failed) => failed.effect_id(),
    }
}

fn settlement_completion_id(outcome: &ModelSettlement) -> Option<&str> {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.completion_id(),
        ModelSettlement::Deferred(_) => None,
        ModelSettlement::Failed(failed) => failed.completion_id(),
    }
}

fn duplicate_decision(state: &KernelState) -> Result<Decision, KernelError> {
    let diagnostic = Diagnostic::try_new(
        "duplicate_settlement",
        "equal committed settlement was ignored",
        DiagnosticSeverity::Info,
        crate::Metadata::empty(),
    )
    .map_err(|_| KernelError::InvariantViolation)?;
    Ok(Decision::duplicate(next_sequence(state)?, diagnostic))
}

fn reject_terminal(state: &KernelState) -> Result<(), KernelError> {
    if matches!(
        state.phase,
        Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled)
    ) || state.terminal.is_some()
    {
        return Err(KernelError::TerminalStateImmutable);
    }
    Ok(())
}

fn next_sequence(state: &KernelState) -> Result<u64, KernelError> {
    state
        .last_applied_sequence
        .checked_add(1)
        .ok_or(KernelError::InvariantViolation)
}

fn required<T: Copy>(values: &[T], index: usize, kind: &'static str) -> Result<T, KernelError> {
    values
        .get(index)
        .copied()
        .ok_or(KernelError::AllocatedIdsExhausted { kind })
}
