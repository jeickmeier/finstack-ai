//! Tool-batch settlement digests.

use super::super::decision::KernelError;
use super::super::input::{ExternalEffectCompletedInput, ExternalEffectOutcome, ToolSettlement};
use crate::effects::{EffectCompleted, EffectFailed};
use crate::primitives::Digest;
use crate::primitives::{EffectId, ToolBatchId, ToolCallId, TurnId};
use crate::records::tools::{
    AssignedToolCall, ToolBatchContinuation, ToolBatchOpened, ToolBatchOutcome,
};
use crate::state::projection::{
    ArtifactSeq, EffectCompletedProjection, EffectDeferredProjection, EffectFailedProjection,
    ErrorProjection, UsageProjection,
};

use super::types::{
    AssignedToolCallFingerprintV1, ToolBatchCloseFingerprintV1, ToolBatchOutcomeFingerprintV1,
    ToolBatchPlanFingerprintV1, ToolResultFingerprintV1, ToolSettlementFingerprintV1,
};

pub fn tool_batch_plan_digest(
    cycle: u64,
    turn_id: TurnId,
    tool_batch_id: ToolBatchId,
    source_message_id: crate::MessageId,
    calls: &[AssignedToolCall],
    continuation: ToolBatchContinuation,
) -> Result<Digest, KernelError> {
    super::super::canonical_digest(
        "tool-batch-plan",
        &ToolBatchPlanFingerprintV1 {
            cycle,
            turn_id,
            tool_batch_id,
            source_message_id,
            calls: calls
                .iter()
                .map(AssignedToolCallFingerprintV1::from)
                .collect(),
            continuation,
        },
    )
}

pub fn opened_tool_batch_plan_digest(opened: &ToolBatchOpened) -> Result<Digest, KernelError> {
    tool_batch_plan_digest(
        opened.cycle,
        opened.turn_id,
        opened.tool_batch_id,
        opened.source_message_id,
        &opened.calls,
        opened.continuation,
    )
}

pub fn direct_tool_digest(
    tool_batch_id: ToolBatchId,
    outcome: &ToolSettlement,
) -> Result<Digest, KernelError> {
    let fingerprint = match outcome {
        ToolSettlement::Completed(completion) => ToolSettlementFingerprintV1::DirectCompleted {
            tool_batch_id,
            completion: EffectCompletedProjection::from(completion),
        },
        ToolSettlement::Deferred(deferred) => ToolSettlementFingerprintV1::DirectDeferred {
            tool_batch_id,
            deferred: EffectDeferredProjection::from(deferred),
        },
        ToolSettlement::Failed(failure) => ToolSettlementFingerprintV1::DirectFailed {
            tool_batch_id,
            failure: EffectFailedProjection::from(failure),
        },
    };
    super::super::canonical_digest("tool-settlement", &fingerprint)
}

pub fn external_tool_digest(
    tool_batch_id: ToolBatchId,
    input: &ExternalEffectCompletedInput,
) -> Result<Digest, KernelError> {
    let fingerprint = match &input.completion.outcome {
        ExternalEffectOutcome::Completed {
            output,
            usage,
            artifacts,
        } => ToolSettlementFingerprintV1::ExternalCompleted {
            tool_batch_id,
            effect_id: input.completion.effect_id,
            completion_id: &input.completion.completion_id,
            output,
            usage: usage.as_ref().map(UsageProjection::from),
            artifacts: ArtifactSeq::new(artifacts),
        },
        ExternalEffectOutcome::Failed { error } => ToolSettlementFingerprintV1::ExternalFailed {
            tool_batch_id,
            effect_id: input.completion.effect_id,
            completion_id: &input.completion.completion_id,
            error: ErrorProjection::from(error),
        },
    };
    super::super::canonical_digest("tool-settlement", &fingerprint)
}

pub fn synthetic_tool_digest(
    tool_batch_id: ToolBatchId,
    tool_call_id: ToolCallId,
    effect_id: EffectId,
    result: &crate::ToolResultBlock,
    error: &crate::ErrorDescriptor,
) -> Result<Digest, KernelError> {
    super::super::canonical_digest(
        "tool-settlement",
        &ToolSettlementFingerprintV1::Synthetic {
            tool_batch_id,
            tool_call_id,
            effect_id,
            result: ToolResultFingerprintV1::from(result),
            error: ErrorProjection::from(error),
        },
    )
}

pub fn completed_tool_record_digest(
    tool_batch_id: ToolBatchId,
    external: bool,
    completed: &EffectCompleted,
) -> Result<Digest, KernelError> {
    let fingerprint = if external {
        ToolSettlementFingerprintV1::ExternalCompleted {
            tool_batch_id,
            effect_id: completed.effect_id(),
            completion_id: completed
                .completion_id()
                .ok_or(KernelError::ToolSettlementMismatch)?,
            output: completed.output(),
            usage: completed.usage().map(UsageProjection::from),
            artifacts: ArtifactSeq::new(completed.artifacts()),
        }
    } else {
        ToolSettlementFingerprintV1::DirectCompleted {
            tool_batch_id,
            completion: EffectCompletedProjection::from(completed),
        }
    };
    super::super::canonical_digest("tool-settlement", &fingerprint)
}

pub fn failed_tool_record_digest(
    tool_batch_id: ToolBatchId,
    external: bool,
    failed: &EffectFailed,
) -> Result<Digest, KernelError> {
    let fingerprint = if external {
        ToolSettlementFingerprintV1::ExternalFailed {
            tool_batch_id,
            effect_id: failed.effect_id(),
            completion_id: failed
                .completion_id()
                .ok_or(KernelError::ToolSettlementMismatch)?,
            error: ErrorProjection::from(failed.error()),
        }
    } else {
        ToolSettlementFingerprintV1::DirectFailed {
            tool_batch_id,
            failure: EffectFailedProjection::from(failed),
        }
    };
    super::super::canonical_digest("tool-settlement", &fingerprint)
}

pub fn tool_batch_close_digest(
    cycle: u64,
    turn_id: TurnId,
    tool_batch_id: ToolBatchId,
    source_message_id: crate::MessageId,
    result_message_ids: &[crate::MessageId],
    outcome: &ToolBatchOutcome,
) -> Result<Digest, KernelError> {
    super::super::canonical_digest(
        "tool-batch-close",
        &ToolBatchCloseFingerprintV1 {
            cycle,
            turn_id,
            tool_batch_id,
            source_message_id,
            result_message_ids,
            outcome: ToolBatchOutcomeFingerprintV1::from(outcome),
        },
    )
}
