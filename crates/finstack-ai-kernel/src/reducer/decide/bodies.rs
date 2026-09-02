use std::sync::Arc;

use crate::effects::EffectRequested;
use crate::primitives::Digest;
use crate::records::RecordBody;
use crate::records::lifecycle::{ContextPrepared, Stage, StageCursor, StageDisposition};
use crate::state::{KernelState, TransitionEnv};

use super::super::decision::{KernelError, PostCommitAction};
use super::super::failure_from_state;
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
    env: &TransitionEnv,
    cursor: StageCursor,
    settlement_digest: Digest,
    turn_id: crate::TurnId,
    requested: EffectRequested,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let model_request_id = required(env.ids.model_request_ids(), 0, "model_request_ids")?;
    let effect_id = requested.effect_id();
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
    let mut bodies = vec![stage_record(
        cursor,
        StageDisposition::Failed {
            error: error.clone(),
        },
        settlement_digest,
    )];
    if cursor.stage == Stage::BeforeFinalize {
        bodies.push(RecordBody::RunFailed(failure_from_state(
            state,
            error.clone(),
        )));
    }
    (bodies, None)
}
