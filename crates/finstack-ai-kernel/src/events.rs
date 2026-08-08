//! Runtime events with durable-derived vs transient class safety (TDD §20).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::digest::Digest;
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectRequested,
    InteractionCancelled, InteractionExpired, InteractionRequest, InteractionResolution,
};
use crate::error::ErrorDescriptor;
use crate::ids::{
    CancellationRequestId, EffectId, EventId, LaneId, MessageId, ModelRequestId, RunId, SessionId,
    ToolBatchId, ToolCallId, TurnId,
};
use crate::limits::LimitDimension;
use crate::records::{RECORD_KIND_VERSION, RecordBody};
use crate::refs::{RefsError, Sensitivity, validated_label};
use crate::run::RunAccepted;
use crate::time::Timestamp;

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
    /// Message finalized (reserved payload).
    MessageFinalized,
    /// Tool settled (reserved payload).
    ToolSettled,
    /// Limit reached (reserved payload).
    LimitReached,
    /// Run suspended (reserved payload).
    RunSuspended,
    /// Run completed (reserved payload).
    RunCompleted,
    /// Run failed (reserved payload).
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

/// Model text delta body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelTextDelta {
    text: Arc<str>,
}

impl ModelTextDelta {
    /// Construct a text delta.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when text is empty, oversized, or NUL-bearing.
    pub fn try_new(text: impl AsRef<str>) -> Result<Self, EventError> {
        let text = text.as_ref();
        if text.is_empty()
            || text.len() > crate::content::TEXT_MAX_BYTES
            || text.as_bytes().contains(&0)
        {
            return Err(EventError::InvalidLabel { field: "text" });
        }
        Ok(Self {
            text: Arc::<str>::from(text),
        })
    }

    /// Borrow text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'de> Deserialize<'de> for ModelTextDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            text: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text).map_err(de::Error::custom)
    }
}

/// Reasoning delta body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReasoningDelta {
    text: Arc<str>,
}

impl ReasoningDelta {
    /// Construct a reasoning delta.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when text is invalid.
    pub fn try_new(text: impl AsRef<str>) -> Result<Self, EventError> {
        Ok(Self {
            text: ModelTextDelta::try_new(text)?.text,
        })
    }

    /// Borrow text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'de> Deserialize<'de> for ReasoningDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            text: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text).map_err(de::Error::custom)
    }
}

/// Tool progress body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolProgress {
    message: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    percent: Option<u8>,
}

impl ToolProgress {
    /// Construct tool progress.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when message is invalid or percent > 100.
    pub fn try_new(message: impl AsRef<str>, percent: Option<u8>) -> Result<Self, EventError> {
        if percent.is_some_and(|value| value > 100) {
            return Err(EventError::InvalidPercent);
        }
        Ok(Self {
            message: validated_label(message.as_ref(), "message")?,
            percent,
        })
    }
}

impl<'de> Deserialize<'de> for ToolProgress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            message: String,
            #[serde(default)]
            percent: Option<u8>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.message, wire.percent).map_err(de::Error::custom)
    }
}

/// Queue depth warning body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueDepthWarning {
    /// Current depth.
    pub depth: u32,
    /// Configured limit.
    pub limit: u32,
}

/// Provider heartbeat body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderHeartbeat {
    provider: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<Arc<str>>,
}

impl ProviderHeartbeat {
    /// Construct a heartbeat.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when labels fail validation.
    pub fn try_new(
        provider: impl AsRef<str>,
        detail: Option<impl AsRef<str>>,
    ) -> Result<Self, EventError> {
        Ok(Self {
            provider: validated_label(provider.as_ref(), "provider")?,
            detail: match detail {
                Some(value) => Some(validated_label(value.as_ref(), "detail")?),
                None => None,
            },
        })
    }
}

impl<'de> Deserialize<'de> for ProviderHeartbeat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            provider: String,
            #[serde(default)]
            detail: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.provider, wire.detail).map_err(de::Error::custom)
    }
}

/// Event body variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
    /// Reserved message finalized.
    MessageFinalized {
        /// Message id.
        message_id: MessageId,
    },
    /// Reserved tool settled.
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason_code: Option<Arc<str>>,
    },
    /// Reserved run completed.
    RunCompleted {
        /// Result digest.
        result_digest: Digest,
    },
    /// Reserved run failed.
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

/// Public runtime event envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunEvent {
    schema_version: u16,
    kind_version: u16,
    event_id: EventId,
    kind: RunEventKind,
    session_id: SessionId,
    lane_id: LaneId,
    run_id: RunId,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_request_id: Option<ModelRequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_batch_id: Option<ToolBatchId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_id: Option<EffectId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<ToolCallId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durable_sequence: Option<u64>,
    transient_sequence: u64,
    timestamp: Timestamp,
    sensitivity: Sensitivity,
    body: RunEventBody,
}

impl RunEvent {
    /// Construct a durable-derived event.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::ClassMismatch`] when the body is transient or
    /// `durable_sequence` is missing.
    #[allow(clippy::too_many_arguments)]
    pub fn try_durable(
        schema_version: u16,
        kind_version: u16,
        event_id: EventId,
        session_id: SessionId,
        lane_id: LaneId,
        run_id: RunId,
        turn_id: Option<TurnId>,
        model_request_id: Option<ModelRequestId>,
        tool_batch_id: Option<ToolBatchId>,
        effect_id: Option<EffectId>,
        tool_call_id: Option<ToolCallId>,
        durable_sequence: u64,
        transient_sequence: u64,
        timestamp: Timestamp,
        sensitivity: Sensitivity,
        body: RunEventBody,
    ) -> Result<Self, EventError> {
        let kind = body.kind();
        if kind.class() != RunEventClass::DurableDerived {
            return Err(EventError::ClassMismatch {
                expected: RunEventClass::DurableDerived,
                actual: kind.class(),
            });
        }
        Ok(Self {
            schema_version,
            kind_version,
            event_id,
            kind,
            session_id,
            lane_id,
            run_id,
            turn_id,
            model_request_id,
            tool_batch_id,
            effect_id,
            tool_call_id,
            durable_sequence: Some(durable_sequence),
            transient_sequence,
            timestamp,
            sensitivity,
            body,
        })
    }

    /// Construct a transient event.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::ClassMismatch`] when the body is durable-derived.
    #[allow(clippy::too_many_arguments)]
    pub fn try_transient(
        schema_version: u16,
        kind_version: u16,
        event_id: EventId,
        session_id: SessionId,
        lane_id: LaneId,
        run_id: RunId,
        turn_id: Option<TurnId>,
        model_request_id: Option<ModelRequestId>,
        tool_batch_id: Option<ToolBatchId>,
        effect_id: Option<EffectId>,
        tool_call_id: Option<ToolCallId>,
        transient_sequence: u64,
        timestamp: Timestamp,
        sensitivity: Sensitivity,
        body: RunEventBody,
    ) -> Result<Self, EventError> {
        let kind = body.kind();
        if kind.class() != RunEventClass::Transient {
            return Err(EventError::ClassMismatch {
                expected: RunEventClass::Transient,
                actual: kind.class(),
            });
        }
        Ok(Self {
            schema_version,
            kind_version,
            event_id,
            kind,
            session_id,
            lane_id,
            run_id,
            turn_id,
            model_request_id,
            tool_batch_id,
            effect_id,
            tool_call_id,
            durable_sequence: None,
            transient_sequence,
            timestamp,
            sensitivity,
            body,
        })
    }

    /// Event class.
    #[must_use]
    pub fn class(&self) -> RunEventClass {
        self.kind.class()
    }

    /// Kind.
    #[must_use]
    pub fn kind(&self) -> RunEventKind {
        self.kind
    }

    /// Event id.
    #[must_use]
    pub fn event_id(&self) -> EventId {
        self.event_id
    }

    /// Durable sequence.
    #[must_use]
    pub fn durable_sequence(&self) -> Option<u64> {
        self.durable_sequence
    }

    /// Transient sequence.
    #[must_use]
    pub fn transient_sequence(&self) -> u64 {
        self.transient_sequence
    }

    /// Body.
    #[must_use]
    pub fn body(&self) -> &RunEventBody {
        &self.body
    }
}

impl<'de> Deserialize<'de> for RunEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema_version: u16,
            kind_version: u16,
            event_id: EventId,
            kind: RunEventKind,
            session_id: SessionId,
            lane_id: LaneId,
            run_id: RunId,
            #[serde(default)]
            turn_id: Option<TurnId>,
            #[serde(default)]
            model_request_id: Option<ModelRequestId>,
            #[serde(default)]
            tool_batch_id: Option<ToolBatchId>,
            #[serde(default)]
            effect_id: Option<EffectId>,
            #[serde(default)]
            tool_call_id: Option<ToolCallId>,
            #[serde(default)]
            durable_sequence: Option<u64>,
            transient_sequence: u64,
            timestamp: Timestamp,
            sensitivity: Sensitivity,
            body: RunEventBody,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.body.kind() != wire.kind {
            return Err(de::Error::custom("run event kind/body mismatch"));
        }
        match wire.kind.class() {
            RunEventClass::DurableDerived => {
                let Some(sequence) = wire.durable_sequence else {
                    return Err(de::Error::custom(
                        "durable-derived events require durable_sequence",
                    ));
                };
                Self::try_durable(
                    wire.schema_version,
                    wire.kind_version,
                    wire.event_id,
                    wire.session_id,
                    wire.lane_id,
                    wire.run_id,
                    wire.turn_id,
                    wire.model_request_id,
                    wire.tool_batch_id,
                    wire.effect_id,
                    wire.tool_call_id,
                    sequence,
                    wire.transient_sequence,
                    wire.timestamp,
                    wire.sensitivity,
                    wire.body,
                )
                .map_err(de::Error::custom)
            }
            RunEventClass::Transient => {
                if wire.durable_sequence.is_some() {
                    return Err(de::Error::custom(
                        "transient events must omit durable_sequence",
                    ));
                }
                Self::try_transient(
                    wire.schema_version,
                    wire.kind_version,
                    wire.event_id,
                    wire.session_id,
                    wire.lane_id,
                    wire.run_id,
                    wire.turn_id,
                    wire.model_request_id,
                    wire.tool_batch_id,
                    wire.effect_id,
                    wire.tool_call_id,
                    wire.transient_sequence,
                    wire.timestamp,
                    wire.sensitivity,
                    wire.body,
                )
                .map_err(de::Error::custom)
            }
        }
    }
}

/// Map a record body to derived event kinds by ordinal (`kind_version` = 1).
///
/// # Errors
///
/// Returns [`EventError::UnsupportedOrdinal`] when the ordinal is out of range.
pub fn derived_event_kind(
    body: &RecordBody,
    kind_version: u16,
    ordinal: usize,
) -> Result<RunEventKind, EventError> {
    if kind_version != RECORD_KIND_VERSION {
        return Err(EventError::UnsupportedKindVersion { kind_version });
    }
    if ordinal != 0 {
        return Err(EventError::UnsupportedOrdinal { ordinal });
    }
    Ok(match body {
        RecordBody::RunAccepted(_) => RunEventKind::RunAccepted,
        RecordBody::EffectRequested(_) => RunEventKind::EffectRequested,
        RecordBody::EffectDeferred(_) => RunEventKind::EffectDeferred,
        RecordBody::EffectCompleted(_) => RunEventKind::EffectCompleted,
        RecordBody::EffectFailed(_) => RunEventKind::EffectFailed,
        RecordBody::EffectCancelled(_) => RunEventKind::EffectCancelled,
        RecordBody::InteractionRequested(_) => RunEventKind::InteractionRequested,
        RecordBody::InteractionResolved(_) => RunEventKind::InteractionResolved,
        RecordBody::InteractionExpired(_) => RunEventKind::InteractionExpired,
        RecordBody::InteractionCancelled(_) => RunEventKind::InteractionCancelled,
    })
}

/// Event construction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventError {
    /// Durable/transient class mismatch.
    #[error("run event class mismatch: expected {expected:?}, got {actual:?}")]
    ClassMismatch {
        /// Expected class.
        expected: RunEventClass,
        /// Actual class.
        actual: RunEventClass,
    },
    /// Invalid label.
    #[error("invalid {field} label")]
    InvalidLabel {
        /// Field.
        field: &'static str,
    },
    /// Percent out of range.
    #[error("tool progress percent must be <= 100")]
    InvalidPercent,
    /// Unsupported kind version.
    #[error("unsupported event kind_version {kind_version}")]
    UnsupportedKindVersion {
        /// Version.
        kind_version: u16,
    },
    /// Unsupported ordinal.
    #[error("unsupported derived-event ordinal {ordinal}")]
    UnsupportedOrdinal {
        /// Ordinal.
        ordinal: usize,
    },
    /// Refs error.
    #[error(transparent)]
    Refs(#[from] RefsError),
}

impl EventError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ClassMismatch { .. } => "event_class_mismatch",
            Self::InvalidLabel { .. } => "invalid_label",
            Self::InvalidPercent => "invalid_percent",
            Self::UnsupportedKindVersion { .. } => "unsupported_kind_version",
            Self::UnsupportedOrdinal { .. } => "unsupported_ordinal",
            Self::Refs(inner) => inner.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EventId, LaneId, RunId, SessionId};
    use crate::time::Timestamp;

    #[test]
    fn durable_and_transient_constructors_are_class_safe() {
        let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("e");
        let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("s");
        let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("l");
        let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("r");
        let delta = ModelTextDelta::try_new("hi").expect("delta");
        assert!(
            RunEvent::try_durable(
                RUN_EVENT_SCHEMA_VERSION,
                RUN_EVENT_KIND_VERSION,
                event_id,
                session,
                lane,
                run,
                None,
                None,
                None,
                None,
                None,
                1,
                0,
                Timestamp::from_unix_ms(0).expect("ts"),
                Sensitivity::Internal,
                RunEventBody::ModelTextDelta(delta.clone()),
            )
            .is_err()
        );
        let transient = RunEvent::try_transient(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session,
            lane,
            run,
            None,
            None,
            None,
            None,
            None,
            0,
            Timestamp::from_unix_ms(0).expect("ts"),
            Sensitivity::Internal,
            RunEventBody::ModelTextDelta(delta),
        )
        .expect("transient");
        assert_eq!(transient.class(), RunEventClass::Transient);
        assert_eq!(transient.durable_sequence(), None);
    }
}
