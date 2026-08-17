//! Stage settlement digests.

use super::super::decision::KernelError;
use super::super::input::{ReducerStageOutcome, StageSettled};
use crate::digest::Digest;
use crate::effects::{EffectInput, EffectRequested};
use crate::entries::{ContextPrepared, StageCursor, StageDisposition, StageOutcomeRecorded};
use crate::projection::{ErrorProjection, MessageSeq};

use super::types::{
    ModelRequestPreparedFingerprintV1, StageFailFingerprintV1, StageSettlementFingerprintV1,
    ToolCallPlanFingerprintV1,
};

pub fn stage_digest(input: &StageSettled) -> Result<Digest, KernelError> {
    let fingerprint = match &input.outcome {
        ReducerStageOutcome::Continue => StageSettlementFingerprintV1::Continue {
            cursor: input.cursor,
        },
        ReducerStageOutcome::ContextPrepared { messages } => {
            StageSettlementFingerprintV1::ContextPrepared {
                cursor: input.cursor,
                messages: MessageSeq::new(messages),
            }
        }
        ReducerStageOutcome::ModelRequestPrepared {
            request,
            component,
            output_contract,
            retry_safety,
            deadline,
        } => StageSettlementFingerprintV1::ModelRequestPrepared(Box::new(
            ModelRequestPreparedFingerprintV1 {
                cursor: input.cursor,
                request,
                component: component.as_ref(),
                output_contract,
                retry_safety: *retry_safety,
                deadline: *deadline,
            },
        )),
        ReducerStageOutcome::ToolBatchPrepared {
            calls,
            continuation,
        } => StageSettlementFingerprintV1::ToolBatchPrepared {
            cursor: input.cursor,
            calls: calls.iter().map(ToolCallPlanFingerprintV1::from).collect(),
            continuation: *continuation,
        },
        ReducerStageOutcome::FinalizeAccepted => StageSettlementFingerprintV1::FinalizeAccepted {
            cursor: input.cursor,
        },
        ReducerStageOutcome::ContinueModel { .. } => StageSettlementFingerprintV1::ContinueModel {
            cursor: input.cursor,
        },
        ReducerStageOutcome::Retry(directive) => StageSettlementFingerprintV1::Retry {
            cursor: input.cursor,
            classification: directive.classification,
            backoff: directive.backoff,
            policy_version: &directive.policy_version,
        },
        ReducerStageOutcome::Fail(error) => {
            StageSettlementFingerprintV1::Fail(Box::new(StageFailFingerprintV1 {
                cursor: input.cursor,
                error: ErrorProjection::from(error),
            }))
        }
    };
    super::super::canonical_digest("stage-settlement", &fingerprint)
}

pub fn stage_record_digest(
    outcome: &StageOutcomeRecorded,
    sibling: Option<&crate::RecordEnvelope>,
) -> Result<Digest, KernelError> {
    let fingerprint = match &outcome.disposition {
        StageDisposition::Continued => StageSettlementFingerprintV1::Continue {
            cursor: outcome.cursor,
        },
        StageDisposition::ContextPrepared { .. } => {
            let Some(crate::RecordBody::ContextPrepared(ContextPrepared { messages, .. })) =
                sibling.map(crate::RecordEnvelope::body)
            else {
                return Err(KernelError::InvalidRecordOrder);
            };
            StageSettlementFingerprintV1::ContextPrepared {
                cursor: outcome.cursor,
                messages: MessageSeq::new(messages),
            }
        }
        StageDisposition::ModelRequested { .. } => {
            let Some(crate::RecordBody::EffectRequested(requested)) =
                sibling.map(crate::RecordEnvelope::body)
            else {
                return Err(KernelError::InvalidRecordOrder);
            };
            model_request_stage_fingerprint(outcome.cursor, requested)?
        }
        StageDisposition::ToolBatchPrepared { .. } => {
            let Some(crate::RecordBody::ToolBatchOpened(opened)) =
                sibling.map(crate::RecordEnvelope::body)
            else {
                return Err(KernelError::InvalidRecordOrder);
            };
            StageSettlementFingerprintV1::ToolBatchPrepared {
                cursor: outcome.cursor,
                calls: opened
                    .calls
                    .iter()
                    .map(|assigned| ToolCallPlanFingerprintV1::from(&assigned.plan))
                    .collect(),
                continuation: opened.continuation,
            }
        }
        StageDisposition::FinalizeAccepted => StageSettlementFingerprintV1::FinalizeAccepted {
            cursor: outcome.cursor,
        },
        StageDisposition::ContinueModel { next_cycle } => {
            if outcome
                .cursor
                .cycle
                .checked_add(1)
                .is_none_or(|expected| expected != *next_cycle)
            {
                return Err(KernelError::InvalidRecordOrder);
            }
            StageSettlementFingerprintV1::ContinueModel {
                cursor: outcome.cursor,
            }
        }
        StageDisposition::RetryScheduled { .. } => {
            let Some(crate::RecordBody::RetryScheduled(retry)) =
                sibling.map(crate::RecordEnvelope::body)
            else {
                return Err(KernelError::InvalidRecordOrder);
            };
            StageSettlementFingerprintV1::Retry {
                cursor: outcome.cursor,
                classification: retry.classification,
                backoff: crate::Duration::from_millis(
                    retry
                        .due_at
                        .as_unix_ms()
                        .checked_sub(
                            sibling
                                .ok_or(KernelError::InvalidRecordOrder)?
                                .timestamp()
                                .as_unix_ms(),
                        )
                        .and_then(|value| u64::try_from(value).ok())
                        .ok_or(KernelError::InvalidRecordOrder)?,
                ),
                policy_version: &retry.policy_version,
            }
        }
        StageDisposition::Failed { error } => {
            StageSettlementFingerprintV1::Fail(Box::new(StageFailFingerprintV1 {
                cursor: outcome.cursor,
                error: ErrorProjection::from(error),
            }))
        }
    };
    super::super::canonical_digest("stage-settlement", &fingerprint)
}

fn model_request_stage_fingerprint(
    cursor: StageCursor,
    requested: &EffectRequested,
) -> Result<StageSettlementFingerprintV1<'_>, KernelError> {
    let EffectInput::Model { request } = requested.input() else {
        return Err(KernelError::InvalidRecordOrder);
    };
    Ok(StageSettlementFingerprintV1::ModelRequestPrepared(
        Box::new(ModelRequestPreparedFingerprintV1 {
            cursor,
            request,
            component: requested.component(),
            output_contract: requested.output_contract(),
            retry_safety: requested.retry_safety(),
            deadline: requested.deadline(),
        }),
    ))
}
