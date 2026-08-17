//! Stage, model, and tool fingerprint structs.

use serde::Serialize;

use crate::effects::{ComponentInvocation, EffectOutputContract, RetrySafety};
use crate::lifecycle::StageCursor;
use crate::primitives::RawJson;
use crate::primitives::Timestamp;
use crate::primitives::{EffectId, ModelRequestId, ToolBatchId, ToolCallId, TurnId};
use crate::state::projection::{
    ArtifactSeq, ContentSeq, EffectCompletedProjection, EffectDeferredProjection,
    EffectFailedProjection, ErrorProjection, MessageProjection, MessageSeq, UsageProjection,
};
use crate::tools::{AssignedToolCall, ToolBatchContinuation, ToolBatchOutcome, ToolCallPlan};

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
        calls: Vec<ToolCallPlanFingerprintV1<'a>>,
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
    pub(super) calls: Vec<AssignedToolCallFingerprintV1<'a>>,
    pub(super) continuation: ToolBatchContinuation,
}

#[derive(Serialize)]
pub struct AssignedToolCallFingerprintV1<'a> {
    pub(super) source_index: u32,
    pub(super) group_index: u32,
    pub(super) effect_id: EffectId,
    pub(super) plan: ToolCallPlanFingerprintV1<'a>,
}

impl<'a> From<&'a AssignedToolCall> for AssignedToolCallFingerprintV1<'a> {
    fn from(value: &'a AssignedToolCall) -> Self {
        Self {
            source_index: value.source_index,
            group_index: value.group_index,
            effect_id: value.effect_id,
            plan: ToolCallPlanFingerprintV1::from(&value.plan),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallPlanFingerprintV1<'a> {
    Execute {
        call: &'a crate::ToolCallBlock,
        tool_id: &'a crate::ToolId,
        component: Option<&'a ComponentInvocation>,
        output_contract: &'a EffectOutputContract,
        retry_safety: RetrySafety,
        deadline: Option<Timestamp>,
        execution: crate::ToolExecutionMode,
        failure_policy: crate::ToolFailurePolicy,
    },
    SyntheticClosure {
        call: &'a crate::ToolCallBlock,
        execution: crate::ToolExecutionMode,
        failure_policy: crate::ToolFailurePolicy,
        error: Box<ErrorProjection<'a>>,
    },
}

impl<'a> From<&'a ToolCallPlan> for ToolCallPlanFingerprintV1<'a> {
    fn from(value: &'a ToolCallPlan) -> Self {
        match value {
            ToolCallPlan::Execute(call) => Self::Execute {
                call: &call.call,
                tool_id: &call.tool_id,
                component: call.component.as_ref(),
                output_contract: &call.output_contract,
                retry_safety: call.retry_safety,
                deadline: call.deadline,
                execution: call.execution,
                failure_policy: call.failure_policy,
            },
            ToolCallPlan::SyntheticClosure(closure) => Self::SyntheticClosure {
                call: &closure.call,
                execution: closure.execution,
                failure_policy: closure.failure_policy,
                error: Box::new(ErrorProjection::from(&closure.error)),
            },
        }
    }
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
        result: ToolResultFingerprintV1<'a>,
        error: ErrorProjection<'a>,
    },
}

#[derive(Serialize)]
pub struct ToolResultFingerprintV1<'a> {
    pub(super) tool_call_id: ToolCallId,
    pub(super) content: ContentSeq<'a>,
    pub(super) is_error: bool,
}

impl<'a> From<&'a crate::ToolResultBlock> for ToolResultFingerprintV1<'a> {
    fn from(value: &'a crate::ToolResultBlock) -> Self {
        Self {
            tool_call_id: *value.tool_call_id(),
            content: ContentSeq::new(value.content()),
            is_error: value.is_error(),
        }
    }
}

#[derive(Serialize)]
pub struct ToolBatchCloseFingerprintV1<'a> {
    pub(super) cycle: u64,
    pub(super) turn_id: TurnId,
    pub(super) tool_batch_id: ToolBatchId,
    pub(super) source_message_id: crate::MessageId,
    pub(super) result_message_ids: &'a [crate::MessageId],
    pub(super) outcome: ToolBatchOutcomeFingerprintV1<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolBatchOutcomeFingerprintV1<'a> {
    ContinueModel,
    Finalize,
    Failed { error: Box<ErrorProjection<'a>> },
}

impl<'a> From<&'a ToolBatchOutcome> for ToolBatchOutcomeFingerprintV1<'a> {
    fn from(value: &'a ToolBatchOutcome) -> Self {
        match value {
            ToolBatchOutcome::ContinueModel => Self::ContinueModel,
            ToolBatchOutcome::Finalize => Self::Finalize,
            ToolBatchOutcome::Failed { error } => Self::Failed {
                error: Box::new(ErrorProjection::from(error)),
            },
        }
    }
}
