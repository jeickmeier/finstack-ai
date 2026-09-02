//! Normalized reducer inputs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::decision::KernelError;
use crate::RecordExternalCommandRejected;
use crate::RetryDirective;
use crate::StageCursor;
use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::conversation::Message;
use crate::effects::{
    ComponentInvocation, EffectCompleted, EffectDeferred, EffectFailed, EffectOutputContract,
    EffectRelation, InteractionCancelled, InteractionExpired, InteractionRequest,
    InteractionResolution, RetrySafety,
};
use crate::primitives::ErrorDescriptor;
use crate::primitives::RawJson;
use crate::primitives::Timestamp;
use crate::primitives::{ArtifactRef, Usage};
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{
    CancellationRequestId, EffectId, LaneId, ModelRequestId, SessionId, ToolBatchId, TurnId,
};
use crate::records::run::{CancellationInitiator, RunAccepted};
use crate::records::tools::{ToolBatchContinuation, ToolCallPlan};
use crate::{CapabilitiesActivated, OutputConfiguration, OutputValidated};

/// Complete concrete command vocabulary.
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
    /// Settle one direct call in the active tool batch.
    ToolBatchSettled(ToolBatchSettled),
    /// Request durable cancellation intent.
    CancelRequested(CancelRequested),
    /// Submit bounded cancellation reconciliation progress.
    CancellationReconciled(CancellationReconciledInput),
    /// Fire the exact pending semantic timer.
    TimerFired(TimerFiredInput),
    /// Freeze the run-level final-output contract before execution begins.
    ConfigureOutput(OutputConfiguration),
    /// Activate a complete immutable capability plan at a safe checkpoint.
    CapabilitiesActivated(CapabilitiesActivated),
    /// Commit a normalized external structured-output validation result.
    OutputValidated(OutputValidated),
    /// Record a known authorized external command rejected by semantic validation.
    RecordExternalCommandRejected(RecordExternalCommandRejected),
    /// Request one typed interaction without consuming the current stage cursor.
    RequestInteraction(RequestInteraction),
    /// Settle the outstanding interaction by resolution, expiry, or cancellation.
    InteractionSettled(InteractionSettled),
    /// Request one runtime-owned compaction-summary model effect without consuming `BeforeModel`.
    RequestCompactionModel(RequestCompactionModel),
    /// Request one durable context-provider or middleware effect without consuming its stage.
    RequestExtensionEffect(RequestExtensionEffect),
    /// Settle the outstanding context-provider or middleware effect.
    ExtensionEffectSettled(ExtensionEffectSettled),
}

/// Durable request for one context-provider or middleware invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestExtensionEffect {
    /// Fully normalized effect request. Its input contains the exact stage cursor.
    pub requested: crate::EffectRequested,
}

/// Settlement of the outstanding context-provider or middleware invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionEffectSettled {
    /// Exact stage cursor held by the outstanding request.
    pub cursor: StageCursor,
    /// Normalized terminal outcome.
    pub outcome: ExtensionSettlement,
}

/// Terminal context-provider or middleware effect outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "the public terminal settlement variants preserve frozen effect payloads by value"
)]
pub enum ExtensionSettlement {
    /// Successful effect completion.
    Completed(EffectCompleted),
    /// Failed effect completion.
    Failed(EffectFailed),
}

impl ExtensionSettlement {
    /// Effect identity shared by every settlement variant.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        match self {
            Self::Completed(value) => value.effect_id(),
            Self::Failed(value) => value.effect_id(),
        }
    }

    pub(crate) fn completion_id(&self) -> Option<&str> {
        match self {
            Self::Completed(value) => value.completion_id(),
            Self::Failed(value) => value.completion_id(),
        }
    }

    pub(crate) fn validate_against(
        &self,
        requested: &crate::EffectRequested,
    ) -> Result<(), crate::EffectError> {
        match self {
            Self::Completed(value) => value.validate_against(requested),
            Self::Failed(value) => value.validate_against(requested),
        }
    }
}

/// Runtime-owned compaction-summary model request (ADR-042).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestCompactionModel {
    /// Canonical model-request draft.
    pub request: RawJson,
    /// Optional resolved component invocation for the child model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentInvocation>,
    /// Parent linkage; purpose must be `CompactionSummary`.
    pub relation: EffectRelation,
    /// Non-optional output contract.
    pub output_contract: EffectOutputContract,
    /// Whether the host may retry the effect.
    pub retry_safety: RetrySafety,
    /// Optional semantic deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<Timestamp>,
}

/// Normalized interaction request submitted at a live middleware stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestInteraction {
    /// Versioned interaction request already allocated by the runtime.
    pub request: InteractionRequest,
}

/// Terminal settlement for the outstanding interaction effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum InteractionSettled {
    /// Schema-valid resolution, including approval denial.
    Resolved(InteractionResolution),
    /// Deadline expiry of the outstanding request.
    Expired(InteractionExpired),
    /// Principal- or framework-authored cancellation while waiting.
    Cancelled(InteractionCancelled),
}

/// Normalized cancellation request; the request ID is allocated by `TransitionEnv`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelRequested {
    /// Authenticated cancellation source.
    pub initiator: CancellationInitiator,
    /// Optional safe bounded reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Arc<str>>,
}

impl<'de> Deserialize<'de> for CancelRequested {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            initiator: CancellationInitiator,
            #[serde(default)]
            reason: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            initiator: wire.initiator,
            reason: wire.reason.map(|value| Arc::from(value.into_inner())),
        })
    }
}

/// Cumulative cancellation reconciliation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancellationReconciledInput {
    /// Winning request identity.
    pub request_id: CancellationRequestId,
    /// Effects completed before cancellation closure.
    pub completed_effects: Arc<[EffectId]>,
    /// Effects acknowledged cancelled.
    pub cancelled_effects: Arc<[EffectId]>,
    /// Effects with uncertain external outcome.
    pub uncertain_effects: Arc<[EffectId]>,
}

/// Runtime-supplied semantic timer firing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimerFiredInput {
    /// Matching timer effect.
    pub effect_id: EffectId,
    /// Frozen due time.
    pub due_at: Timestamp,
    /// Runtime-observed firing time.
    pub fired_at: Timestamp,
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

/// Aggregate outcomes accepted by stages.
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
    /// Prepared source-ordered tool batch plan.
    ToolBatchPrepared {
        /// Exactly one plan per assistant source call.
        calls: Arc<[ToolCallPlan]>,
        /// Continuation after complete batch closure.
        continuation: ToolBatchContinuation,
    },
    /// Accept the current terminal candidate.
    FinalizeAccepted,
    /// Begin a fresh model cycle.
    ContinueModel {
        /// Optional non-semantic explanatory reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<Arc<str>>,
    },
    /// Schedule one bounded whole-run semantic retry.
    Retry(RetryDirective),
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
        struct ToolBatchPreparedWire {
            calls: BoundedVec<ToolCallPlan, SEMANTIC_ARRAY_MAX_ITEMS>,
            continuation: ToolBatchContinuation,
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
            ToolBatchPrepared(ToolBatchPreparedWire),
            FinalizeAccepted,
            ContinueModel(ContinueModelWire),
            Retry(RetryDirective),
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
            Wire::ToolBatchPrepared(value) => Self::ToolBatchPrepared {
                calls: Arc::from(value.calls.into_inner()),
                continuation: value.continuation,
            },
            Wire::FinalizeAccepted => Self::FinalizeAccepted,
            Wire::ContinueModel(value) => Self::ContinueModel {
                reason: value.reason.map(|reason| Arc::from(reason.into_inner())),
            },
            Wire::Retry(value) => Self::Retry(value),
            Wire::Fail(error) => Self::Fail(error),
        })
    }
}

/// Direct settlement of one call in an active tool batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBatchSettled {
    /// Active batch identity.
    pub tool_batch_id: ToolBatchId,
    /// One direct tool outcome.
    pub outcome: ToolSettlement,
}

/// Direct tool-effect outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "the frozen public settlement variants store effect payloads by value"
)]
pub enum ToolSettlement {
    /// Successful tool completion.
    Completed(EffectCompleted),
    /// Deferred external work under the original effect identity.
    Deferred(EffectDeferred),
    /// Failed tool completion.
    Failed(EffectFailed),
}

impl ToolSettlement {
    /// Effect identity shared by every settlement variant.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        match self {
            Self::Completed(value) => value.effect_id(),
            Self::Deferred(value) => value.effect_id,
            Self::Failed(value) => value.effect_id(),
        }
    }

    /// Provider completion identity, if the variant recorded one.
    #[must_use]
    pub fn completion_id(&self) -> Option<&str> {
        match self {
            Self::Completed(value) => value.completion_id(),
            Self::Deferred(_) => None,
            Self::Failed(value) => value.completion_id(),
        }
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

impl ModelSettlement {
    /// Effect identity shared by every settlement variant.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        match self {
            Self::Completed { completion, .. } => completion.effect_id(),
            Self::Deferred(deferred) => deferred.effect_id,
            Self::Failed(failed) => failed.effect_id(),
        }
    }

    /// Provider completion identity, if the variant recorded one.
    #[must_use]
    pub fn completion_id(&self) -> Option<&str> {
        match self {
            Self::Completed { completion, .. } => completion.completion_id(),
            Self::Deferred(_) => None,
            Self::Failed(failed) => failed.completion_id(),
        }
    }
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
    /// # Arguments
    ///
    /// * `effect_id` - Effect being completed by the external host.
    /// * `completion_id` - Non-empty provider completion label.
    /// * `outcome` - Completed, failed, or deferred external outcome.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInputPayload`] for an invalid completion ID
    /// or an oversized artifact collection.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     EffectId, ErrorCategory, ErrorDescriptor, ExternalEffectCompletion, ExternalEffectOutcome,
    /// };
    ///
    /// let completion = ExternalEffectCompletion::try_new(
    ///     EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
    ///     "completion-1",
    ///     ExternalEffectOutcome::Failed {
    ///         error: ErrorDescriptor::new(
    ///             "provider_failed",
    ///             "provider failed",
    ///             ErrorCategory::Model,
    ///             true,
    ///         )
    ///         .expect("error"),
    ///     },
    /// )
    /// .expect("completion");
    /// assert_eq!(completion.completion_id.as_ref(), "completion-1");
    /// ```
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
#[allow(
    clippy::large_enum_variant,
    reason = "the frozen external-completion API stores its error descriptor by value"
)]
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
        #[allow(
            clippy::large_enum_variant,
            reason = "the private wire mirror preserves the frozen JSON shape"
        )]
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
