//! Runtime event body variants.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectRequested,
    InteractionCancelled, InteractionExpired, InteractionRequest, InteractionResolution,
};
use crate::primitives::Digest;
use crate::primitives::ErrorDescriptor;
use crate::primitives::{CancellationRequestId, MessageId, ToolCallId};
use crate::records::policy::LimitDimension;
use crate::records::run::RunAccepted;

use super::{
    ModelTextDelta, ProviderHeartbeat, QueueDepthWarning, ReasoningDelta, RunEventKind,
    ToolProgress,
};

/// Event body variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum RunEventBody {
    /// Run accepted.
    RunAccepted(RunAccepted),
    /// Effect requested.
    EffectRequested(EffectRequested),
    /// Effect deferred.
    EffectDeferred(EffectDeferred),
    /// Effect completed.
    EffectCompleted(EffectCompleted),
    /// Effect failed.
    EffectFailed(EffectFailed),
    /// Effect cancelled.
    EffectCancelled(EffectCancelled),
    /// Interaction requested.
    InteractionRequested(InteractionRequest),
    /// Interaction resolved.
    InteractionResolved(InteractionResolution),
    /// Interaction expired.
    InteractionExpired(InteractionExpired),
    /// Interaction cancelled.
    InteractionCancelled(InteractionCancelled),
    /// Message finalized.
    MessageFinalized {
        /// Message id.
        message_id: MessageId,
    },
    /// Tool settled.
    ToolSettled {
        /// Tool call id.
        tool_call_id: ToolCallId,
    },
    /// Reserved limit reached.
    LimitReached {
        /// Dimension.
        dimension: LimitDimension,
    },
    /// Reserved run suspended.
    RunSuspended {
        /// Optional reason code.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_reason_code"
        )]
        reason_code: Option<Arc<str>>,
    },
    /// Run completed.
    RunCompleted {
        /// Result digest.
        result_digest: Digest,
    },
    /// Run failed.
    RunFailed {
        /// Error.
        error: ErrorDescriptor,
    },
    /// Reserved run cancelled.
    RunCancelled {
        /// Optional cancellation request id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<CancellationRequestId>,
    },
    /// Model text delta.
    ModelTextDelta(ModelTextDelta),
    /// Reasoning delta.
    ReasoningDelta(ReasoningDelta),
    /// Tool progress.
    ToolProgress(ToolProgress),
    /// Queue depth warning.
    QueueDepthWarning(QueueDepthWarning),
    /// Provider heartbeat.
    ProviderHeartbeat(ProviderHeartbeat),
}

fn deserialize_reason_code<'de, D>(deserializer: D) -> Result<Option<Arc<str>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<BoundedString<LABEL_MAX_BYTES>>::deserialize(deserializer)
        .map(|code| code.map(|value| Arc::from(value.into_inner())))
}

impl RunEventBody {
    /// Matching kind tag.
    #[must_use]
    pub const fn kind(&self) -> RunEventKind {
        match self {
            Self::RunAccepted(_) => RunEventKind::RunAccepted,
            Self::EffectRequested(_) => RunEventKind::EffectRequested,
            Self::EffectDeferred(_) => RunEventKind::EffectDeferred,
            Self::EffectCompleted(_) => RunEventKind::EffectCompleted,
            Self::EffectFailed(_) => RunEventKind::EffectFailed,
            Self::EffectCancelled(_) => RunEventKind::EffectCancelled,
            Self::InteractionRequested(_) => RunEventKind::InteractionRequested,
            Self::InteractionResolved(_) => RunEventKind::InteractionResolved,
            Self::InteractionExpired(_) => RunEventKind::InteractionExpired,
            Self::InteractionCancelled(_) => RunEventKind::InteractionCancelled,
            Self::MessageFinalized { .. } => RunEventKind::MessageFinalized,
            Self::ToolSettled { .. } => RunEventKind::ToolSettled,
            Self::LimitReached { .. } => RunEventKind::LimitReached,
            Self::RunSuspended { .. } => RunEventKind::RunSuspended,
            Self::RunCompleted { .. } => RunEventKind::RunCompleted,
            Self::RunFailed { .. } => RunEventKind::RunFailed,
            Self::RunCancelled { .. } => RunEventKind::RunCancelled,
            Self::ModelTextDelta(_) => RunEventKind::ModelTextDelta,
            Self::ReasoningDelta(_) => RunEventKind::ReasoningDelta,
            Self::ToolProgress(_) => RunEventKind::ToolProgress,
            Self::QueueDepthWarning(_) => RunEventKind::QueueDepthWarning,
            Self::ProviderHeartbeat(_) => RunEventKind::ProviderHeartbeat,
        }
    }
}
