use std::sync::Arc;

use crate::digest::Digest;
use crate::effects::{EffectInput, EffectKind, EffectOutputKind, EffectRequested};
use crate::entries::{ContextPrepared, Stage, StageCursor, StageDisposition};
use crate::records::RecordBody;
use crate::state::{KernelState, TransitionEnv};

use super::super::decision::{KernelError, PostCommitAction};
use super::super::failure_from_state;
use super::super::input::ReducerStageOutcome;
use super::required;
use super::shared::{stage_record, terminal_body_from_candidate};

pub(super) fn prepared_context_bodies(
    env: &TransitionEnv,
    cursor: StageCursor,
    messages: &Arc<[crate::Message]>,
    settlement_digest: Digest,
    context_digest: Digest,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let turn_id = required(env.ids.turn_ids(), 0, "turn_ids")?;
    // The messages are already a shared slice and already canonicalized; taking
    // a `Vec` here and collecting back into an `Arc` copied the whole context
    // twice for no change in value.
    let context =
        ContextPrepared::from_shared(cursor.cycle, turn_id, Arc::clone(messages), context_digest);
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

pub(super) fn requested_model_bodies(
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

pub(super) fn accepted_finalize_bodies(
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

pub(super) fn continued_model_bodies(
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

pub(super) fn failed_stage_bodies(
    state: &KernelState,
    cursor: StageCursor,
    settlement_digest: Digest,
    error: &crate::ErrorDescriptor,
) -> (Vec<RecordBody>, Option<PostCommitAction>) {
    match cursor.stage {
        Stage::BeforeRun
        | Stage::PrepareContext
        | Stage::BeforeModel
        | Stage::AfterModel
        | Stage::BeforeToolBatch
        | Stage::AfterToolBatch => (
            vec![stage_record(
                cursor,
                StageDisposition::Failed {
                    error: error.clone(),
                },
                settlement_digest,
            )],
            None,
        ),
        Stage::BeforeFinalize => (
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
        ),
    }
}
