//! Runtime events with durable-derived vs transient class safety (TDD §20).

use serde::{Deserialize, Serialize};

/// Current runtime-event schema version.
pub const RUN_EVENT_SCHEMA_VERSION: u16 = 1;
/// Current runtime-event kind version for PR-008 surfaces.
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
pub use derive::derived_event_kind;
pub(crate) use envelope::EventCorrelations;
pub use envelope::RunEvent;
pub use error::EventError;
pub use transient::{
    ModelTextDelta, ProviderHeartbeat, QueueDepthWarning, ReasoningDelta, ToolProgress,
};
