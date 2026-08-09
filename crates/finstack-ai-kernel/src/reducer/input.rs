//! Normalized PR-009 reducer inputs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::decision::KernelError;
use crate::StageCursor;
use crate::bounds::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::effects::{
    ComponentInvocation, EffectCompleted, EffectDeferred, EffectFailed, EffectOutputContract,
    RetrySafety,
};
use crate::error::ErrorDescriptor;
use crate::ids::{EffectId, LaneId, ModelRequestId, SessionId, TurnId};
use crate::message::Message;
use crate::raw_json::RawJson;
use crate::refs::{ArtifactRef, Usage};
use crate::run::RunAccepted;
use crate::time::Timestamp;

/// Complete concrete PR-009 command vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum KernelInput {
    /// Accept a normalized run.
    AcceptRun(AcceptRun),
    /// Settle one aggregate middleware boundary.
    StageSettled(StageSettled),
    /// Settle the outstanding direct model effect.
    ModelSettled(ModelSettled),
    /// Complete the outstanding deferred model effect.
    ExternalEffectCompleted(ExternalEffectCompletedInput),
}

/// Normalized run-acceptance command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptRun {
    /// Session identity.
    pub session_id: SessionId,
    /// Lane identity.
    pub lane_id: LaneId,
    /// Fully validated accepted run payload.
    pub accepted: RunAccepted,
}

/// Aggregate stage-settlement command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageSettled {
    /// Exact expected cycle and stage.
    pub cursor: StageCursor,
    /// Normalized aggregate outcome.
    pub outcome: ReducerStageOutcome,
}

/// Aggregate outcomes accepted by PR-009 stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReducerStageOutcome {
    /// Continue to the next stage.
    Continue,
    /// Prepared model context.
    ContextPrepared {
        /// Provider-neutral context messages.
        messages: Arc<[Message]>,
    },
    /// Prepared model effect request.
    ModelRequestPrepared {
        /// Normalized provider request JSON.
        request: RawJson,
        /// Optional resolved component invocation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        component: Option<ComponentInvocation>,
        /// Non-optional output contract.
        output_contract: EffectOutputContract,
        /// Retry-safety classification.
        retry_safety: RetrySafety,
        /// Optional semantic deadline.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline: Option<Timestamp>,
    },
    /// Accept the current terminal candidate.
    FinalizeAccepted,
    /// Begin a fresh model cycle.
    ContinueModel {
        /// Optional non-semantic explanatory reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<Arc<str>>,
    },
    /// Normalize an aggregate stage failure.
    Fail(ErrorDescriptor),
}

impl<'de> Deserialize<'de> for ReducerStageOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ContextPreparedWire {
            messages: BoundedVec<Message, SEMANTIC_ARRAY_MAX_ITEMS>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ModelRequestPreparedWire {
            request: RawJson,
            #[serde(default)]
            component: Option<ComponentInvocation>,
            output_contract: EffectOutputContract,
            retry_safety: RetrySafety,
            #[serde(default)]
            deadline: Option<Timestamp>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ContinueModelWire {
            #[serde(default)]
            reason: Option<BoundedString<TEXT_MAX_BYTES>>,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire {
            Continue,
            ContextPrepared(ContextPreparedWire),
            ModelRequestPrepared(ModelRequestPreparedWire),
            FinalizeAccepted,
            ContinueModel(ContinueModelWire),
            Fail(ErrorDescriptor),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Continue => Self::Continue,
            Wire::ContextPrepared(value) => Self::ContextPrepared {
                messages: Arc::from(value.messages.into_inner()),
            },
            Wire::ModelRequestPrepared(value) => Self::ModelRequestPrepared {
                request: value.request,
                component: value.component,
                output_contract: value.output_contract,
                retry_safety: value.retry_safety,
                deadline: value.deadline,
            },
            Wire::FinalizeAccepted => Self::FinalizeAccepted,
            Wire::ContinueModel(value) => Self::ContinueModel {
                reason: value.reason.map(|reason| Arc::from(reason.into_inner())),
            },
            Wire::Fail(error) => Self::Fail(error),
        })
    }
}

/// Direct model-effect settlement command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSettled {
    /// Outstanding turn identity.
    pub turn_id: TurnId,
    /// Outstanding model request identity.
    pub model_request_id: ModelRequestId,
    /// Normalized model settlement.
    pub outcome: ModelSettlement,
}

/// Direct model-effect outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ModelSettlement {
    /// Successful model completion and final assistant message.
    Completed {
        /// Normalized effect completion.
        completion: EffectCompleted,
        /// Final assistant message.
        assistant_message: Message,
    },
    /// Deferred external work under the original effect identity.
    Deferred(EffectDeferred),
    /// Failed model completion.
    Failed(EffectFailed),
}

/// Deferred effect completion command and optional successful assistant message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalEffectCompletedInput {
    /// External effect completion.
    pub completion: ExternalEffectCompletion,
    /// Required only for a successful model outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_message: Option<Message>,
}

/// Completion of a previously deferred effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalEffectCompletion {
    /// Original effect identity.
    pub effect_id: EffectId,
    /// Non-empty external idempotency identity.
    pub completion_id: Arc<str>,
    /// Normalized external outcome.
    pub outcome: ExternalEffectOutcome,
}

impl ExternalEffectCompletion {
    /// Construct a bounded external completion identity and outcome.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInputPayload`] for an invalid completion ID
    /// or an oversized artifact collection.
    pub fn try_new(
        effect_id: EffectId,
        completion_id: impl AsRef<str>,
        outcome: ExternalEffectOutcome,
    ) -> Result<Self, KernelError> {
        let completion_id = completion_id.as_ref();
        if completion_id.is_empty()
            || completion_id.len() > LABEL_MAX_BYTES
            || completion_id.as_bytes().contains(&0)
        {
            return Err(KernelError::InvalidInputPayload {
                field: "completion_id",
                reason_code: "invalid_label",
            });
        }
        if matches!(
            &outcome,
            ExternalEffectOutcome::Completed { artifacts, .. }
                if artifacts.len() > SEMANTIC_ARRAY_MAX_ITEMS
        ) {
            return Err(KernelError::InvalidInputPayload {
                field: "artifacts",
                reason_code: "too_many_items",
            });
        }
        Ok(Self {
            effect_id,
            completion_id: Arc::from(completion_id),
            outcome,
        })
    }
}

impl<'de> Deserialize<'de> for ExternalEffectCompletion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            effect_id: EffectId,
            completion_id: BoundedString<LABEL_MAX_BYTES>,
            outcome: ExternalEffectOutcome,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.effect_id,
            wire.completion_id.into_inner(),
            wire.outcome,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// External completion outcome supported at the generic effect boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalEffectOutcome {
    /// Successful normalized output.
    Completed {
        /// Normalized output JSON.
        output: RawJson,
        /// Optional normalized usage.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// Staged replay-required artifacts.
        artifacts: Arc<[ArtifactRef]>,
    },
    /// Failed external work.
    Failed {
        /// Safe failure descriptor.
        error: ErrorDescriptor,
    },
}

impl<'de> Deserialize<'de> for ExternalEffectOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CompletedWire {
            output: RawJson,
            #[serde(default)]
            usage: Option<Usage>,
            artifacts: BoundedVec<ArtifactRef, SEMANTIC_ARRAY_MAX_ITEMS>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FailedWire {
            error: ErrorDescriptor,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire {
            Completed(CompletedWire),
            Failed(FailedWire),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Completed(value) => Self::Completed {
                output: value.output,
                usage: value.usage,
                artifacts: Arc::from(value.artifacts.into_inner()),
            },
            Wire::Failed(value) => Self::Failed { error: value.error },
        })
    }
}
