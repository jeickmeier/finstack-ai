//! Model settlement digests.

use super::super::decision::KernelError;
use super::super::input::{
    ExternalEffectCompletedInput, ExternalEffectOutcome, ModelSettled, ModelSettlement,
};
use crate::conversation::Message;
use crate::effects::{EffectCompleted, EffectFailed};
use crate::primitives::Digest;
use crate::state::PendingModelEffect;
use crate::state::projection::{
    ArtifactSeq, EffectCompletedProjection, EffectDeferredProjection, EffectFailedProjection,
    ErrorProjection, MessageProjection, UsageProjection,
};

use super::types::{
    DirectModelCompletedFingerprintV1, DirectModelDeferredFingerprintV1,
    DirectModelFailedFingerprintV1, ExternalModelCompletedFingerprintV1,
    ExternalModelFailedFingerprintV1, ModelSettlementFingerprintV1,
};

pub(in crate::reducer) fn direct_digest(input: &ModelSettled) -> Result<Digest, KernelError> {
    let fingerprint = match &input.outcome {
        ModelSettlement::Completed {
            completion,
            assistant_message,
        } => ModelSettlementFingerprintV1::DirectCompleted(DirectModelCompletedFingerprintV1 {
            turn_id: input.turn_id,
            model_request_id: input.model_request_id,
            completion: EffectCompletedProjection::from(completion),
            assistant_message: MessageProjection::from(assistant_message),
        }),
        ModelSettlement::Failed(failure) => {
            ModelSettlementFingerprintV1::DirectFailed(DirectModelFailedFingerprintV1 {
                turn_id: input.turn_id,
                model_request_id: input.model_request_id,
                failure: EffectFailedProjection::from(failure),
            })
        }
        ModelSettlement::Deferred(deferred) => {
            ModelSettlementFingerprintV1::DirectDeferred(DirectModelDeferredFingerprintV1 {
                turn_id: input.turn_id,
                model_request_id: input.model_request_id,
                deferred: EffectDeferredProjection::from(deferred),
            })
        }
    };
    digest(&fingerprint)
}

pub(in crate::reducer) fn external_digest(
    input: &ExternalEffectCompletedInput,
) -> Result<Digest, KernelError> {
    // Failed outcomes always project ExternalFailed so completion-identity
    // classification (contract section 11.3.3 steps 3–4) can run before assistant-presence
    // checks (step 6). Completed outcomes require the assistant in the projection.
    let fingerprint = match (&input.completion.outcome, &input.assistant_message) {
        (
            ExternalEffectOutcome::Completed {
                output,
                usage,
                artifacts,
            },
            Some(assistant_message),
        ) => ModelSettlementFingerprintV1::ExternalCompleted(ExternalModelCompletedFingerprintV1 {
            effect_id: input.completion.effect_id,
            completion_id: &input.completion.completion_id,
            output,
            usage: usage.as_ref().map(UsageProjection::from),
            artifacts: ArtifactSeq::new(artifacts),
            assistant_message: MessageProjection::from(assistant_message),
        }),
        (ExternalEffectOutcome::Completed { .. }, None) => {
            return Err(KernelError::AssistantMessagePresenceMismatch);
        }
        (ExternalEffectOutcome::Failed { error }, _) => {
            ModelSettlementFingerprintV1::ExternalFailed(ExternalModelFailedFingerprintV1 {
                effect_id: input.completion.effect_id,
                completion_id: &input.completion.completion_id,
                error: ErrorProjection::from(error),
            })
        }
    };
    digest(&fingerprint)
}

pub(in crate::reducer) fn completed_compaction_digest(
    pending: &PendingModelEffect,
    completed: &EffectCompleted,
) -> Result<Digest, KernelError> {
    super::super::canonical_digest(
        "compaction-model-settlement",
        &(
            pending.turn_id,
            pending.model_request_id,
            EffectCompletedProjection::from(completed),
        ),
    )
}

pub(in crate::reducer) fn completed_record_digest(
    pending: &PendingModelEffect,
    completed: &EffectCompleted,
    assistant_message: &Message,
) -> Result<Digest, KernelError> {
    let fingerprint = if pending.deferred.is_some() {
        if completed.reservation_id().is_some() {
            return Err(KernelError::ModelSettlementMismatch);
        }
        let completion_id = completed
            .completion_id()
            .ok_or(KernelError::ModelSettlementMismatch)?;
        ModelSettlementFingerprintV1::ExternalCompleted(ExternalModelCompletedFingerprintV1 {
            effect_id: completed.effect_id(),
            completion_id,
            output: completed.output(),
            usage: completed.usage().map(UsageProjection::from),
            artifacts: ArtifactSeq::new(completed.artifacts()),
            assistant_message: MessageProjection::from(assistant_message),
        })
    } else {
        ModelSettlementFingerprintV1::DirectCompleted(DirectModelCompletedFingerprintV1 {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            completion: EffectCompletedProjection::from(completed),
            assistant_message: MessageProjection::from(assistant_message),
        })
    };
    digest(&fingerprint)
}

pub(in crate::reducer) fn failed_record_digest(
    pending: &PendingModelEffect,
    failed: &EffectFailed,
) -> Result<Digest, KernelError> {
    let fingerprint = if pending.deferred.is_some() {
        if failed.usage().is_some() {
            return Err(KernelError::ModelSettlementMismatch);
        }
        let completion_id = failed
            .completion_id()
            .ok_or(KernelError::ModelSettlementMismatch)?;
        ModelSettlementFingerprintV1::ExternalFailed(ExternalModelFailedFingerprintV1 {
            effect_id: failed.effect_id(),
            completion_id,
            error: ErrorProjection::from(failed.error()),
        })
    } else {
        ModelSettlementFingerprintV1::DirectFailed(DirectModelFailedFingerprintV1 {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            failure: EffectFailedProjection::from(failed),
        })
    };
    digest(&fingerprint)
}

fn digest(fingerprint: &ModelSettlementFingerprintV1<'_>) -> Result<Digest, KernelError> {
    super::super::canonical_digest("model-settlement", fingerprint)
}
