//! Runtime events with durable-derived vs transient class safety (TDD §20).

use serde::{Deserialize, Serialize};

/// Current runtime-event schema version.
pub const RUN_EVENT_SCHEMA_VERSION: u16 = 1;
/// Current runtime-event kind version.
pub const RUN_EVENT_KIND_VERSION: u16 = 1;

/// Semantic class of a [`RunEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEventClass {
    /// Derived from a committed durable record.
    DurableDerived,
    /// Transient progress (non-replay-stable ids).
    Transient,
}

/// Runtime event kind tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEventKind {
    /// Run accepted.
    RunAccepted,
    /// Effect requested.
    EffectRequested,
    /// Effect deferred.
    EffectDeferred,
    /// Effect completed.
    EffectCompleted,
    /// Effect failed.
    EffectFailed,
    /// Effect cancelled.
    EffectCancelled,
    /// Interaction requested.
    InteractionRequested,
    /// Interaction resolved.
    InteractionResolved,
    /// Interaction expired.
    InteractionExpired,
    /// Interaction cancelled.
    InteractionCancelled,
    /// Message finalized.
    MessageFinalized,
    /// Tool settled.
    ToolSettled,
    /// Limit reached (reserved payload).
    LimitReached,
    /// Run suspended (reserved payload).
    RunSuspended,
    /// Run completed.
    RunCompleted,
    /// Run failed.
    RunFailed,
    /// Run cancelled (reserved payload).
    RunCancelled,
    /// Model text delta.
    ModelTextDelta,
    /// Reasoning delta.
    ReasoningDelta,
    /// Tool progress.
    ToolProgress,
    /// Queue depth warning.
    QueueDepthWarning,
    /// Provider heartbeat.
    ProviderHeartbeat,
}

impl RunEventKind {
    /// Section 20.2.1 kind string (`snake_case` wire name).
    #[must_use]
    pub const fn kind_name(self) -> &'static str {
        match self {
            Self::RunAccepted => "run_accepted",
            Self::EffectRequested => "effect_requested",
            Self::EffectDeferred => "effect_deferred",
            Self::EffectCompleted => "effect_completed",
            Self::EffectFailed => "effect_failed",
            Self::EffectCancelled => "effect_cancelled",
            Self::InteractionRequested => "interaction_requested",
            Self::InteractionResolved => "interaction_resolved",
            Self::InteractionExpired => "interaction_expired",
            Self::InteractionCancelled => "interaction_cancelled",
            Self::MessageFinalized => "message_finalized",
            Self::ToolSettled => "tool_settled",
            Self::LimitReached => "limit_reached",
            Self::RunSuspended => "run_suspended",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
            Self::RunCancelled => "run_cancelled",
            Self::ModelTextDelta => "model_text_delta",
            Self::ReasoningDelta => "reasoning_delta",
            Self::ToolProgress => "tool_progress",
            Self::QueueDepthWarning => "queue_depth_warning",
            Self::ProviderHeartbeat => "provider_heartbeat",
        }
    }

    /// Semantic class for this kind.
    #[must_use]
    pub const fn class(self) -> RunEventClass {
        match self {
            Self::ModelTextDelta
            | Self::ReasoningDelta
            | Self::ToolProgress
            | Self::QueueDepthWarning
            | Self::ProviderHeartbeat => RunEventClass::Transient,
            _ => RunEventClass::DurableDerived,
        }
    }
}

mod body;
mod derive;
mod envelope;
mod error;
mod transient;

#[cfg(test)]
mod tests;

pub use body::RunEventBody;
pub(crate) use derive::derived_event_kind;
pub(crate) use envelope::EventCorrelations;
pub use envelope::RunEvent;
pub use error::EventError;
pub use transient::{
    ModelTextDelta, ProviderHeartbeat, QueueDepthWarning, ReasoningDelta, ToolProgress,
};
