//! Stage, model, and tool fingerprint structs.

use serde::Serialize;

use crate::effects::{ComponentInvocation, EffectOutputContract, RetrySafety};
use crate::primitives::RawJson;
use crate::primitives::Timestamp;
use crate::primitives::{EffectId, ModelRequestId, ToolBatchId, ToolCallId, TurnId};
use crate::records::lifecycle::StageCursor;
use crate::records::tools::ToolBatchContinuation;
use crate::state::projection::{
    ArtifactSeq, AssignedToolCallProjection, EffectCompletedProjection, EffectDeferredProjection,
    EffectFailedProjection, ErrorProjection, MessageProjection, MessageSeq,
    ToolBatchOutcomeProjection, ToolCallPlanProjection, ToolResultProjection, UsageProjection,
};

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageSettlementFingerprintV1<'a> {
    Continue {
        cursor: StageCursor,
    },
    ContextPrepared {
        cursor: StageCursor,
        messages: MessageSeq<'a>,
    },
    ModelRequestPrepared(Box<ModelRequestPreparedFingerprintV1<'a>>),
    ToolBatchPrepared {
        cursor: StageCursor,
        calls: Vec<ToolCallPlanProjection<'a>>,
        continuation: ToolBatchContinuation,
    },
    FinalizeAccepted {
        cursor: StageCursor,
    },
    ContinueModel {
        cursor: StageCursor,
    },
    Retry {
        cursor: StageCursor,
        classification: crate::RetryClassification,
        backoff: crate::Duration,
        policy_version: &'a str,
    },
    Fail(Box<StageFailFingerprintV1<'a>>),
}

#[derive(Serialize)]
pub struct ModelRequestPreparedFingerprintV1<'a> {
    pub(super) cursor: StageCursor,
    pub(super) request: &'a RawJson,
    pub(super) component: Option<&'a ComponentInvocation>,
    pub(super) output_contract: &'a EffectOutputContract,
    pub(super) retry_safety: RetrySafety,
    pub(super) deadline: Option<Timestamp>,
}

#[derive(Serialize)]
pub struct StageFailFingerprintV1<'a> {
    pub(super) cursor: StageCursor,
    pub(super) error: ErrorProjection<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSettlementFingerprintV1<'a> {
    DirectCompleted(DirectModelCompletedFingerprintV1<'a>),
    DirectFailed(DirectModelFailedFingerprintV1<'a>),
    DirectDeferred(DirectModelDeferredFingerprintV1<'a>),
    ExternalCompleted(ExternalModelCompletedFingerprintV1<'a>),
    ExternalFailed(ExternalModelFailedFingerprintV1<'a>),
}

#[derive(Serialize)]
pub struct DirectModelCompletedFingerprintV1<'a> {
    pub(super) turn_id: TurnId,
    pub(super) model_request_id: ModelRequestId,
    pub(super) completion: EffectCompletedProjection<'a>,
    pub(super) assistant_message: MessageProjection<'a>,
}

#[derive(Serialize)]
pub struct DirectModelFailedFingerprintV1<'a> {
    pub(super) turn_id: TurnId,
    pub(super) model_request_id: ModelRequestId,
    pub(super) failure: EffectFailedProjection<'a>,
}

#[derive(Serialize)]
pub struct DirectModelDeferredFingerprintV1<'a> {
    pub(super) turn_id: TurnId,
    pub(super) model_request_id: ModelRequestId,
    pub(super) deferred: EffectDeferredProjection<'a>,
}

#[derive(Serialize)]
pub struct ExternalModelCompletedFingerprintV1<'a> {
    pub(super) effect_id: EffectId,
    pub(super) completion_id: &'a str,
    pub(super) output: &'a RawJson,
    pub(super) usage: Option<UsageProjection<'a>>,
    pub(super) artifacts: ArtifactSeq<'a>,
    pub(super) assistant_message: MessageProjection<'a>,
}

#[derive(Serialize)]
pub struct ExternalModelFailedFingerprintV1<'a> {
    pub(super) effect_id: EffectId,
    pub(super) completion_id: &'a str,
    pub(super) error: ErrorProjection<'a>,
}

#[derive(Serialize)]
pub struct ToolBatchPlanFingerprintV1<'a> {
    pub(super) cycle: u64,
    pub(super) turn_id: TurnId,
    pub(super) tool_batch_id: ToolBatchId,
    pub(super) source_message_id: crate::MessageId,
    pub(super) calls: Vec<AssignedToolCallProjection<'a>>,
    pub(super) continuation: ToolBatchContinuation,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSettlementFingerprintV1<'a> {
    DirectCompleted {
        tool_batch_id: ToolBatchId,
        completion: EffectCompletedProjection<'a>,
    },
    DirectFailed {
        tool_batch_id: ToolBatchId,
        failure: EffectFailedProjection<'a>,
    },
    DirectDeferred {
        tool_batch_id: ToolBatchId,
        deferred: EffectDeferredProjection<'a>,
    },
    ExternalCompleted {
        tool_batch_id: ToolBatchId,
        effect_id: EffectId,
        completion_id: &'a str,
        output: &'a RawJson,
        usage: Option<UsageProjection<'a>>,
        artifacts: ArtifactSeq<'a>,
    },
    ExternalFailed {
        tool_batch_id: ToolBatchId,
        effect_id: EffectId,
        completion_id: &'a str,
        error: ErrorProjection<'a>,
    },
    Synthetic {
        tool_batch_id: ToolBatchId,
        tool_call_id: ToolCallId,
        effect_id: EffectId,
        result: ToolResultProjection<'a>,
        error: ErrorProjection<'a>,
    },
}

#[derive(Serialize)]
pub struct ToolBatchCloseFingerprintV1<'a> {
    pub(super) cycle: u64,
    pub(super) turn_id: TurnId,
    pub(super) tool_batch_id: ToolBatchId,
    pub(super) source_message_id: crate::MessageId,
    pub(super) result_message_ids: &'a [crate::MessageId],
    pub(super) outcome: ToolBatchOutcomeProjection<'a>,
}
