//! Runtime events with durable-derived vs transient class safety (TDD §20).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::digest::Digest;
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectOutputKind,
    EffectRequested, InteractionCancelled, InteractionExpired, InteractionRequest,
    InteractionResolution,
};
use crate::error::ErrorDescriptor;
use crate::ids::{
    CancellationRequestId, EffectId, EventId, LaneId, MessageId, ModelRequestId, RunId, SessionId,
    ToolBatchId, ToolCallId, TurnId,
};
use crate::limits::LimitDimension;
use crate::records::{RECORD_KIND_VERSION, RecordBody, RecordEnvelope};
use crate::refs::{RefsError, Sensitivity, validated_label, validated_text};
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
            text: BoundedString<TEXT_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text.into_inner()).map_err(de::Error::custom)
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
            text: BoundedString<TEXT_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text.into_inner()).map_err(de::Error::custom)
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
            message: validated_text(message.as_ref(), "message")?,
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
            message: BoundedString<TEXT_MAX_BYTES>,
            #[serde(default)]
            percent: Option<u8>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.message.into_inner(), wire.percent).map_err(de::Error::custom)
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
                Some(value) => Some(validated_text(value.as_ref(), "detail")?),
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
            provider: BoundedString<LABEL_MAX_BYTES>,
            #[serde(default)]
            detail: Option<BoundedString<TEXT_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.provider.into_inner(),
            wire.detail.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}

/// Event body variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
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

impl<'de> Deserialize<'de> for RunEventBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "snake_case")]
        enum Wire {
            RunAccepted(RunAccepted),
            EffectRequested(EffectRequested),
            EffectDeferred(EffectDeferred),
            EffectCompleted(EffectCompleted),
            EffectFailed(EffectFailed),
            EffectCancelled(EffectCancelled),
            InteractionRequested(InteractionRequest),
            InteractionResolved(InteractionResolution),
            InteractionExpired(InteractionExpired),
            InteractionCancelled(InteractionCancelled),
            MessageFinalized {
                message_id: MessageId,
            },
            ToolSettled {
                tool_call_id: ToolCallId,
            },
            LimitReached {
                dimension: LimitDimension,
            },
            RunSuspended {
                #[serde(default)]
                reason_code: Option<BoundedString<LABEL_MAX_BYTES>>,
            },
            RunCompleted {
                result_digest: Digest,
            },
            RunFailed {
                error: ErrorDescriptor,
            },
            RunCancelled {
                #[serde(default)]
                request_id: Option<CancellationRequestId>,
            },
            ModelTextDelta(ModelTextDelta),
            ReasoningDelta(ReasoningDelta),
            ToolProgress(ToolProgress),
            QueueDepthWarning(QueueDepthWarning),
            ProviderHeartbeat(ProviderHeartbeat),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::RunAccepted(value) => Self::RunAccepted(value),
            Wire::EffectRequested(value) => Self::EffectRequested(value),
            Wire::EffectDeferred(value) => Self::EffectDeferred(value),
            Wire::EffectCompleted(value) => Self::EffectCompleted(value),
            Wire::EffectFailed(value) => Self::EffectFailed(value),
            Wire::EffectCancelled(value) => Self::EffectCancelled(value),
            Wire::InteractionRequested(value) => Self::InteractionRequested(value),
            Wire::InteractionResolved(value) => Self::InteractionResolved(value),
            Wire::InteractionExpired(value) => Self::InteractionExpired(value),
            Wire::InteractionCancelled(value) => Self::InteractionCancelled(value),
            Wire::MessageFinalized { message_id } => Self::MessageFinalized { message_id },
            Wire::ToolSettled { tool_call_id } => Self::ToolSettled { tool_call_id },
            Wire::LimitReached { dimension } => Self::LimitReached { dimension },
            Wire::RunSuspended { reason_code } => Self::RunSuspended {
                reason_code: reason_code.map(|value| Arc::from(value.into_inner())),
            },
            Wire::RunCompleted { result_digest } => Self::RunCompleted { result_digest },
            Wire::RunFailed { error } => Self::RunFailed { error },
            Wire::RunCancelled { request_id } => Self::RunCancelled { request_id },
            Wire::ModelTextDelta(value) => Self::ModelTextDelta(value),
            Wire::ReasoningDelta(value) => Self::ReasoningDelta(value),
            Wire::ToolProgress(value) => Self::ToolProgress(value),
            Wire::QueueDepthWarning(value) => Self::QueueDepthWarning(value),
            Wire::ProviderHeartbeat(value) => Self::ProviderHeartbeat(value),
        })
    }
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

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EventCorrelations {
    pub model_turn: Option<TurnId>,
    pub model_request: Option<ModelRequestId>,
    pub tool_turn: Option<TurnId>,
    pub tool_batch: Option<ToolBatchId>,
    pub tool_call: Option<ToolCallId>,
}

struct ResolvedEventCorrelations {
    turn: Option<TurnId>,
    model_request: Option<ModelRequestId>,
    tool_batch: Option<ToolBatchId>,
    effect: Option<EffectId>,
    tool_call: Option<ToolCallId>,
}

impl RunEvent {
    /// Construct a durable-derived event.
    ///
    /// Correlation values supplied here are event data, not proof of source
    /// authenticity. Runtime code derives authoritative model-effect events
    /// through [`crate::Kernel::apply`].
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
        validate_event_versions(schema_version, kind_version)?;
        let kind = body.kind();
        if kind.class() != RunEventClass::DurableDerived {
            return Err(EventError::ClassMismatch {
                expected: RunEventClass::DurableDerived,
                actual: kind.class(),
            });
        }
        validate_event_correlations(run_id, model_request_id, effect_id, tool_call_id, &body)?;
        validate_event_policy(
            turn_id,
            model_request_id,
            tool_batch_id,
            effect_id,
            tool_call_id,
            sensitivity,
            &body,
        )?;
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
    /// Runtime producers must source model correlations from the outstanding
    /// [`crate::PendingModelEffect`]; this value constructor does not authenticate
    /// caller-supplied identifiers.
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
        validate_event_versions(schema_version, kind_version)?;
        let kind = body.kind();
        if kind.class() != RunEventClass::Transient {
            return Err(EventError::ClassMismatch {
                expected: RunEventClass::Transient,
                actual: kind.class(),
            });
        }
        validate_event_correlations(run_id, model_request_id, effect_id, tool_call_id, &body)?;
        validate_event_policy(
            turn_id,
            model_request_id,
            tool_batch_id,
            effect_id,
            tool_call_id,
            sensitivity,
            &body,
        )?;
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

    /// Construct a durable-derived event from its committed source record and ordinal.
    ///
    /// This reuses the replay-stable event id persisted on the record.
    ///
    /// Model and tool effect records require authoritative state correlations
    /// and are therefore derived only by [`crate::Kernel::apply`]. Other record
    /// bodies carry all required correlations in the record.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when the record is not run-scoped, the ordinal is
    /// unsupported, or record/body correlations are invalid.
    pub fn try_from_record(
        record: &RecordEnvelope,
        ordinal: usize,
        transient_sequence: u64,
    ) -> Result<Self, EventError> {
        Self::try_from_record_with_correlations(
            record,
            ordinal,
            transient_sequence,
            EventCorrelations::default(),
        )
    }

    pub(crate) fn try_from_record_with_correlations(
        record: &RecordEnvelope,
        ordinal: usize,
        transient_sequence: u64,
        correlations: EventCorrelations,
    ) -> Result<Self, EventError> {
        let expected_kind = derived_event_kind(record.body(), record.kind_version(), ordinal)?;
        let event_id = record
            .derived_event_ids()
            .get(ordinal)
            .copied()
            .ok_or(EventError::UnsupportedOrdinal { ordinal })?;
        let body = run_event_body_from_record(record.body(), ordinal)?;
        if body.kind() != expected_kind {
            return Err(EventError::CorrelationMismatch {
                reason: "derived event ordinal/body mismatch",
            });
        }
        let run_id = record.run_id().ok_or(EventError::CorrelationMismatch {
            reason: "PR-008 durable event source must be run-scoped",
        })?;
        let resolved = record_correlations(record.body(), correlations, effect_id_for_body(&body));
        if is_model_effect_record(record.body())
            && (resolved.turn.is_none() || resolved.model_request.is_none())
        {
            return Err(EventError::CorrelationMismatch {
                reason: "model effect event requires authoritative model correlations",
            });
        }
        if is_tool_effect_record(record.body())
            && (resolved.turn.is_none()
                || resolved.tool_batch.is_none()
                || resolved.tool_call.is_none())
        {
            return Err(EventError::CorrelationMismatch {
                reason: "tool effect event requires authoritative tool correlations",
            });
        }
        let sensitivity = derived_event_sensitivity(record.body());
        Self::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            record.kind_version(),
            event_id,
            record.session_id(),
            record.lane_id(),
            run_id,
            resolved.turn,
            resolved.model_request,
            resolved.tool_batch,
            resolved.effect,
            resolved.tool_call,
            record.sequence(),
            transient_sequence,
            record.timestamp(),
            sensitivity,
            body,
        )
    }

    /// Event class.
    #[must_use]
    pub fn class(&self) -> RunEventClass {
        self.kind.class()
    }

    /// Event schema version.
    #[must_use]
    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Event kind version.
    #[must_use]
    pub fn kind_version(&self) -> u16 {
        self.kind_version
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

    /// Session id.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Lane id.
    #[must_use]
    pub fn lane_id(&self) -> LaneId {
        self.lane_id
    }

    /// Run id.
    #[must_use]
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Turn id correlation.
    #[must_use]
    pub fn turn_id(&self) -> Option<TurnId> {
        self.turn_id
    }

    /// Model request correlation.
    #[must_use]
    pub fn model_request_id(&self) -> Option<ModelRequestId> {
        self.model_request_id
    }

    /// Tool batch correlation.
    #[must_use]
    pub fn tool_batch_id(&self) -> Option<ToolBatchId> {
        self.tool_batch_id
    }

    /// Effect correlation.
    #[must_use]
    pub fn effect_id(&self) -> Option<EffectId> {
        self.effect_id
    }

    /// Tool call correlation.
    #[must_use]
    pub fn tool_call_id(&self) -> Option<ToolCallId> {
        self.tool_call_id
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

    /// Semantic event timestamp.
    #[must_use]
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Event sensitivity.
    #[must_use]
    pub fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    /// Body.
    #[must_use]
    pub fn body(&self) -> &RunEventBody {
        &self.body
    }
}

fn validate_event_policy(
    turn_id: Option<TurnId>,
    model_request_id: Option<ModelRequestId>,
    tool_batch_id: Option<ToolBatchId>,
    effect_id: Option<EffectId>,
    tool_call_id: Option<ToolCallId>,
    sensitivity: Sensitivity,
    body: &RunEventBody,
) -> Result<(), EventError> {
    match body {
        RunEventBody::MessageFinalized { .. }
            if turn_id.is_none()
                || effect_id.is_none()
                || (model_request_id.is_none()
                    && (tool_batch_id.is_none() || tool_call_id.is_none()))
                || sensitivity != Sensitivity::Internal =>
        {
            return Err(EventError::CorrelationMismatch {
                reason: "message event requires model or tool correlations and internal sensitivity",
            });
        }
        RunEventBody::ToolSettled { .. }
            if turn_id.is_none()
                || tool_batch_id.is_none()
                || effect_id.is_none()
                || tool_call_id.is_none()
                || sensitivity != Sensitivity::Internal =>
        {
            return Err(EventError::CorrelationMismatch {
                reason: "tool-settled event requires turn, batch, effect, call, and internal sensitivity",
            });
        }
        RunEventBody::RunCompleted { .. }
            if turn_id.is_none()
                || model_request_id.is_none()
                || effect_id.is_none()
                || sensitivity != Sensitivity::Internal =>
        {
            return Err(EventError::CorrelationMismatch {
                reason: "run-completed event requires model correlations and internal sensitivity",
            });
        }
        RunEventBody::RunFailed { .. } if sensitivity != Sensitivity::Internal => {
            return Err(EventError::CorrelationMismatch {
                reason: "run failed event requires internal sensitivity",
            });
        }
        _ => {}
    }
    let model_event = match body {
        RunEventBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Model,
        RunEventBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ModelResponse
        }
        RunEventBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RunEventBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RunEventBody::ModelTextDelta(_) | RunEventBody::ReasoningDelta(_) => true,
        _ => false,
    };
    if model_event
        && (turn_id.is_none()
            || model_request_id.is_none()
            || effect_id.is_none()
            || sensitivity != Sensitivity::Confidential)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "model event requires turn_id, model_request_id, effect_id, and confidential sensitivity",
        });
    }
    let tool_event = match body {
        RunEventBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Tool,
        RunEventBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ToolResult
        }
        RunEventBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RunEventBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RunEventBody::ToolProgress(_) => true,
        _ => false,
    };
    if tool_event
        && (turn_id.is_none()
            || tool_batch_id.is_none()
            || effect_id.is_none()
            || tool_call_id.is_none()
            || sensitivity != Sensitivity::Confidential)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "tool event requires turn_id, tool_batch_id, effect_id, tool_call_id, and confidential sensitivity",
        });
    }
    Ok(())
}

fn validate_event_versions(schema_version: u16, kind_version: u16) -> Result<(), EventError> {
    if schema_version != RUN_EVENT_SCHEMA_VERSION {
        return Err(EventError::UnsupportedSchemaVersion { schema_version });
    }
    if kind_version != RUN_EVENT_KIND_VERSION {
        return Err(EventError::UnsupportedKindVersion { kind_version });
    }
    Ok(())
}

fn validate_event_correlations(
    run_id: RunId,
    model_request_id: Option<ModelRequestId>,
    effect_id: Option<EffectId>,
    tool_call_id: Option<ToolCallId>,
    body: &RunEventBody,
) -> Result<(), EventError> {
    if let RunEventBody::RunAccepted(accepted) = body {
        if !accepted.lineage_is_validated() {
            return Err(EventError::CorrelationMismatch {
                reason: "run-accepted lineage has not been validated",
            });
        }
        if accepted.run_id() != run_id {
            return Err(EventError::CorrelationMismatch {
                reason: "run-accepted event run_id does not match body",
            });
        }
    }
    if let Some(expected) = effect_id_for_body(body)
        && effect_id != Some(expected)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "effect correlation does not match body",
        });
    }
    if let RunEventBody::ToolSettled {
        tool_call_id: expected,
    } = body
        && tool_call_id != Some(*expected)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "tool-call correlation does not match body",
        });
    }
    if matches!(
        body,
        RunEventBody::ModelTextDelta(_) | RunEventBody::ReasoningDelta(_)
    ) && model_request_id.is_none()
    {
        return Err(EventError::CorrelationMismatch {
            reason: "model delta requires model_request_id",
        });
    }
    if matches!(body, RunEventBody::ToolProgress(_)) && tool_call_id.is_none() {
        return Err(EventError::CorrelationMismatch {
            reason: "tool progress requires tool_call_id",
        });
    }
    Ok(())
}

fn effect_id_for_body(body: &RunEventBody) -> Option<EffectId> {
    match body {
        RunEventBody::EffectRequested(value) => Some(value.effect_id()),
        RunEventBody::EffectDeferred(value) => Some(value.effect_id),
        RunEventBody::EffectCompleted(value) => Some(value.effect_id()),
        RunEventBody::EffectFailed(value) => Some(value.effect_id()),
        RunEventBody::EffectCancelled(value) => Some(value.effect_id()),
        RunEventBody::InteractionRequested(value) => Some(value.effect_id()),
        _ => None,
    }
}

fn run_event_body_from_record(
    body: &RecordBody,
    ordinal: usize,
) -> Result<RunEventBody, EventError> {
    let event = match body {
        RecordBody::RunAccepted(value) => RunEventBody::RunAccepted(value.clone()),
        RecordBody::EffectRequested(value) => RunEventBody::EffectRequested(value.clone()),
        RecordBody::EffectDeferred(value) => RunEventBody::EffectDeferred(value.clone()),
        RecordBody::EffectCompleted(value) => RunEventBody::EffectCompleted(value.clone()),
        RecordBody::EffectFailed(value) => RunEventBody::EffectFailed(value.clone()),
        RecordBody::EffectCancelled(value) => RunEventBody::EffectCancelled(value.clone()),
        RecordBody::InteractionRequested(value) => {
            RunEventBody::InteractionRequested(value.clone())
        }
        RecordBody::InteractionResolved(value) => RunEventBody::InteractionResolved(value.clone()),
        RecordBody::InteractionExpired(value) => RunEventBody::InteractionExpired(value.clone()),
        RecordBody::InteractionCancelled(value) => {
            RunEventBody::InteractionCancelled(value.clone())
        }
        RecordBody::EntryAppended(value) => RunEventBody::MessageFinalized {
            message_id: *value.message.id(),
        },
        RecordBody::ToolCallSettled(value) if ordinal == 0 => RunEventBody::MessageFinalized {
            message_id: *value.message.id(),
        },
        RecordBody::ToolCallSettled(value) if ordinal == 1 => RunEventBody::ToolSettled {
            tool_call_id: value.tool_call_id,
        },
        RecordBody::RunCompleted(value) => RunEventBody::RunCompleted {
            result_digest: value.result_digest,
        },
        RecordBody::RunFailed(value) => RunEventBody::RunFailed {
            error: value.error.clone(),
        },
        RecordBody::LimitReached(value) => RunEventBody::LimitReached {
            dimension: value.dimension.clone(),
        },
        RecordBody::RunSuspended(value) => RunEventBody::RunSuspended {
            reason_code: Some(Arc::from(value.reason_code.as_str())),
        },
        RecordBody::RunCancelled(value) => RunEventBody::RunCancelled {
            request_id: Some(value.request_id),
        },
        RecordBody::StageOutcomeRecorded(_)
        | RecordBody::ContextPrepared(_)
        | RecordBody::ToolBatchOpened(_)
        | RecordBody::ToolBatchClosed(_)
        | RecordBody::CancellationRequested(_)
        | RecordBody::CancellationReconciled(_)
        | RecordBody::RetryScheduled(_)
        | RecordBody::TimerFired(_)
        | RecordBody::OutputConfigured(_)
        | RecordBody::CapabilitiesActivated(_)
        | RecordBody::FinalResultRecorded(_)
        | RecordBody::OutputValidationFailed(_)
        | RecordBody::ExternalCommandRejected(_)
        | RecordBody::ChildRunPrepared(_)
        | RecordBody::BudgetReservationRequested(_)
        | RecordBody::BudgetReservationSettled(_)
        | RecordBody::BudgetChargeRecorded(_)
        | RecordBody::BudgetReservationReleased(_)
        | RecordBody::SessionCreated(_)
        | RecordBody::LaneCreated(_)
        | RecordBody::LaneMoved(_)
        | RecordBody::SnapshotWritten(_)
        | RecordBody::ConversationEntry(_)
        | RecordBody::ToolCallSettled(_) => {
            return Err(EventError::UnsupportedOrdinal { ordinal });
        }
    };
    Ok(event)
}

fn record_correlations(
    body: &RecordBody,
    correlations: EventCorrelations,
    body_effect_id: Option<EffectId>,
) -> ResolvedEventCorrelations {
    match body {
        RecordBody::EntryAppended(value) => ResolvedEventCorrelations {
            turn: Some(value.turn_id),
            model_request: Some(value.model_request_id),
            tool_batch: None,
            effect: Some(value.effect_id),
            tool_call: None,
        },
        RecordBody::ToolCallSettled(value) => ResolvedEventCorrelations {
            turn: Some(value.turn_id),
            model_request: None,
            tool_batch: Some(value.tool_batch_id),
            effect: Some(value.effect_id),
            tool_call: Some(value.tool_call_id),
        },
        RecordBody::RunCompleted(value) => ResolvedEventCorrelations {
            turn: Some(value.turn_id),
            model_request: Some(value.model_request_id),
            tool_batch: None,
            effect: Some(value.effect_id),
            tool_call: None,
        },
        RecordBody::RunFailed(value) => ResolvedEventCorrelations {
            turn: value.turn_id,
            model_request: value.model_request_id,
            tool_batch: None,
            effect: value.effect_id,
            tool_call: None,
        },
        RecordBody::RunCancelled(_) | RecordBody::RunSuspended(_) | RecordBody::LimitReached(_) => {
            ResolvedEventCorrelations {
                turn: correlations.model_turn,
                model_request: correlations.model_request,
                tool_batch: correlations.tool_batch,
                effect: body_effect_id,
                tool_call: correlations.tool_call,
            }
        }
        RecordBody::EffectRequested(_)
        | RecordBody::EffectDeferred(_)
        | RecordBody::EffectCompleted(_)
        | RecordBody::EffectFailed(_)
        | RecordBody::EffectCancelled(_)
            if is_tool_effect_record(body) =>
        {
            ResolvedEventCorrelations {
                turn: correlations.tool_turn,
                model_request: None,
                tool_batch: correlations.tool_batch,
                effect: body_effect_id,
                tool_call: correlations.tool_call,
            }
        }
        RecordBody::EffectRequested(_)
        | RecordBody::EffectDeferred(_)
        | RecordBody::EffectCompleted(_)
        | RecordBody::EffectFailed(_)
        | RecordBody::EffectCancelled(_) => ResolvedEventCorrelations {
            turn: correlations.model_turn,
            model_request: correlations.model_request,
            tool_batch: None,
            effect: body_effect_id,
            tool_call: None,
        },
        _ => ResolvedEventCorrelations {
            turn: None,
            model_request: None,
            tool_batch: None,
            effect: body_effect_id,
            tool_call: None,
        },
    }
}

fn derived_event_sensitivity(body: &RecordBody) -> Sensitivity {
    if is_model_effect_record(body) || is_tool_effect_record(body) {
        Sensitivity::Confidential
    } else {
        Sensitivity::Internal
    }
}

fn is_tool_effect_record(body: &RecordBody) -> bool {
    match body {
        RecordBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Tool,
        RecordBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ToolResult
        }
        RecordBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RecordBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RecordBody::EffectCancelled(cancelled) => {
            cancelled.output_contract().kind == EffectOutputKind::ToolResult
        }
        _ => false,
    }
}

fn is_model_effect_record(body: &RecordBody) -> bool {
    match body {
        RecordBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Model,
        RecordBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ModelResponse
        }
        RecordBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RecordBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RecordBody::EffectCancelled(cancelled) => {
            cancelled.output_contract().kind == EffectOutputKind::ModelResponse
        }
        _ => false,
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
    if let RecordBody::ToolCallSettled(_) = body {
        return match ordinal {
            0 => Ok(RunEventKind::MessageFinalized),
            1 => Ok(RunEventKind::ToolSettled),
            _ => Err(EventError::UnsupportedOrdinal { ordinal }),
        };
    }
    if ordinal != 0 {
        return Err(EventError::UnsupportedOrdinal { ordinal });
    }
    let kind = match body {
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
        RecordBody::EntryAppended(_) => RunEventKind::MessageFinalized,
        RecordBody::ToolCallSettled(_) => unreachable!("handled above"),
        RecordBody::RunCompleted(_) => RunEventKind::RunCompleted,
        RecordBody::RunFailed(_) => RunEventKind::RunFailed,
        RecordBody::LimitReached(_) => RunEventKind::LimitReached,
        RecordBody::RunSuspended(_) => RunEventKind::RunSuspended,
        RecordBody::RunCancelled(_) => RunEventKind::RunCancelled,
        RecordBody::StageOutcomeRecorded(_)
        | RecordBody::ContextPrepared(_)
        | RecordBody::ToolBatchOpened(_)
        | RecordBody::ToolBatchClosed(_)
        | RecordBody::CancellationRequested(_)
        | RecordBody::CancellationReconciled(_)
        | RecordBody::RetryScheduled(_)
        | RecordBody::TimerFired(_)
        | RecordBody::OutputConfigured(_)
        | RecordBody::CapabilitiesActivated(_)
        | RecordBody::FinalResultRecorded(_)
        | RecordBody::OutputValidationFailed(_)
        | RecordBody::ExternalCommandRejected(_)
        | RecordBody::ChildRunPrepared(_)
        | RecordBody::BudgetReservationRequested(_)
        | RecordBody::BudgetReservationSettled(_)
        | RecordBody::BudgetChargeRecorded(_)
        | RecordBody::BudgetReservationReleased(_)
        | RecordBody::SessionCreated(_)
        | RecordBody::LaneCreated(_)
        | RecordBody::LaneMoved(_)
        | RecordBody::SnapshotWritten(_)
        | RecordBody::ConversationEntry(_) => {
            return Err(EventError::UnsupportedOrdinal { ordinal });
        }
    };
    Ok(kind)
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
    /// Unsupported event schema version.
    #[error("unsupported event schema_version {schema_version}")]
    UnsupportedSchemaVersion {
        /// Version.
        schema_version: u16,
    },
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
    /// Event envelope correlation did not match its body/source record.
    #[error("run event correlation mismatch: {reason}")]
    CorrelationMismatch {
        /// Reason.
        reason: &'static str,
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
            Self::UnsupportedSchemaVersion { .. } => "unsupported_schema_version",
            Self::UnsupportedKindVersion { .. } => "unsupported_kind_version",
            Self::UnsupportedOrdinal { .. } => "unsupported_ordinal",
            Self::CorrelationMismatch { .. } => "event_correlation_mismatch",
            Self::Refs(inner) => inner.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{
        EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, RetrySafety,
    };
    use crate::ids::{EventId, LaneId, RecordId, RunId, SessionId};
    use crate::raw_json::RawJson;
    use crate::records::{RECORD_FORMAT_VERSION, RecordEnvelope};
    use crate::time::Timestamp;

    #[test]
    fn durable_and_transient_constructors_are_class_safe() {
        let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("e");
        let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("s");
        let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("l");
        let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("r");
        let turn = TurnId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("turn");
        let model_request =
            ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("request");
        let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("effect");
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
        RunEvent::try_transient(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session,
            lane,
            run,
            Some(turn),
            Some(model_request),
            None,
            Some(effect),
            None,
            0,
            Timestamp::from_unix_ms(0).expect("ts"),
            Sensitivity::Internal,
            RunEventBody::ModelTextDelta(delta.clone()),
        )
        .expect_err("model deltas require confidential sensitivity");
        let transient = RunEvent::try_transient(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session,
            lane,
            run,
            Some(turn),
            Some(model_request),
            None,
            Some(effect),
            None,
            0,
            Timestamp::from_unix_ms(0).expect("ts"),
            Sensitivity::Confidential,
            RunEventBody::ModelTextDelta(delta),
        )
        .expect("transient");
        assert_eq!(transient.class(), RunEventClass::Transient);
        assert_eq!(transient.durable_sequence(), None);
        assert_eq!(transient.schema_version(), RUN_EVENT_SCHEMA_VERSION);
        assert_eq!(transient.session_id(), session);
        assert!(transient.model_request_id().is_some());
        assert_eq!(transient.sensitivity(), Sensitivity::Confidential);

        let error = RunEvent::try_transient(
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
            Timestamp::from_unix_ms(0).expect("ts"),
            Sensitivity::Internal,
            RunEventBody::ModelTextDelta(ModelTextDelta::try_new("missing").expect("delta")),
        )
        .expect_err("model delta requires model request correlation");
        assert_eq!(error.code(), "event_correlation_mismatch");
    }

    #[test]
    fn durable_effect_event_round_trips_human_json_with_raw_input() {
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
        let requested = EffectRequested::try_new(
            effect_id,
            EffectKind::Model,
            None,
            None,
            None,
            EffectOutputContract {
                kind: EffectOutputKind::ModelResponse,
                schema_version: 1,
                schema_digest: Digest::raw_json(br#"{"schema":1}"#),
            },
            EffectInput::Model {
                request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("request");
        let turn_id = TurnId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("turn");
        let model_request_id =
            ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("request");
        RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
            RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
            None,
            None,
            None,
            Some(effect_id),
            None,
            1,
            0,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            Sensitivity::Internal,
            RunEventBody::EffectRequested(requested.clone()),
        )
        .expect_err("model effect events require correlations and confidential sensitivity");
        let event = RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
            RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
            Some(turn_id),
            Some(model_request_id),
            None,
            Some(effect_id),
            None,
            1,
            0,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            Sensitivity::Confidential,
            RunEventBody::EffectRequested(requested),
        )
        .expect("event");
        let json = serde_json::to_string(&event).expect("serialize");
        let round: RunEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round, event);
    }

    #[test]
    fn event_constructors_reject_unknown_versions() {
        let error = RunEvent::try_transient(
            RUN_EVENT_SCHEMA_VERSION + 1,
            RUN_EVENT_KIND_VERSION,
            EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
            RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
            None,
            None,
            None,
            None,
            None,
            0,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            Sensitivity::Internal,
            RunEventBody::ModelTextDelta(ModelTextDelta::try_new("delta").expect("delta")),
        )
        .expect_err("unknown schema version");
        assert_eq!(error.code(), "unsupported_schema_version");
    }

    #[test]
    fn effect_events_require_matching_effect_correlation() {
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
        let requested = EffectRequested::try_new(
            effect_id,
            EffectKind::Model,
            None,
            None,
            None,
            EffectOutputContract {
                kind: EffectOutputKind::ModelResponse,
                schema_version: 1,
                schema_digest: Digest::raw_json(br#"{"schema":1}"#),
            },
            EffectInput::Model {
                request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("request");
        let error = RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
            RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
            None,
            None,
            None,
            None,
            None,
            1,
            0,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            Sensitivity::Internal,
            RunEventBody::EffectRequested(requested),
        )
        .expect_err("missing effect correlation");
        assert_eq!(error.code(), "event_correlation_mismatch");
    }

    #[test]
    fn durable_event_from_record_reuses_persisted_ordinal_id() {
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
        let requested = EffectRequested::try_new(
            effect_id,
            EffectKind::Model,
            None,
            None,
            None,
            EffectOutputContract {
                kind: EffectOutputKind::ModelResponse,
                schema_version: 1,
                schema_digest: Digest::raw_json(br#"{"schema":1}"#),
            },
            EffectInput::Model {
                request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("request");
        let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event");
        let record = RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("record"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("lane"),
            Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("run")),
            7,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            Digest::raw_json(br"payload"),
            None,
            Digest::raw_json(br"checksum"),
            vec![event_id],
            RecordBody::EffectRequested(requested),
        )
        .expect("record");
        let missing = RunEvent::try_from_record(&record, 0, 3);
        assert!(matches!(
            missing,
            Err(EventError::CorrelationMismatch { reason })
                if reason.contains("authoritative model correlations")
        ));
    }

    #[test]
    fn non_model_effect_record_derives_without_model_correlations() {
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
        let requested = EffectRequested::try_new(
            effect_id,
            EffectKind::Context,
            None,
            None,
            None,
            EffectOutputContract {
                kind: EffectOutputKind::ContextContribution,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"schema"),
            },
            EffectInput::Context {
                request: RawJson::parse("{}").expect("request"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("request");
        let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event");
        let record = RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("record"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("lane"),
            Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("run")),
            7,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            Digest::raw_json(b"payload"),
            None,
            Digest::raw_json(b"checksum"),
            vec![event_id],
            RecordBody::EffectRequested(requested),
        )
        .expect("record");

        let event = RunEvent::try_from_record(&record, 0, 3).expect("derived context event");
        assert_eq!(event.event_id(), event_id);
        assert_eq!(event.effect_id(), Some(effect_id));
        assert_eq!(event.turn_id(), None);
        assert_eq!(event.model_request_id(), None);
        assert_eq!(event.sensitivity(), Sensitivity::Internal);
    }

    #[test]
    fn finalized_message_requires_model_correlations_and_internal_sensitivity() {
        let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event");
        let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session");
        let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane");
        let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run");
        let message = MessageId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("message");
        let turn = TurnId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("turn");
        let request =
            ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("request");
        let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("effect");
        let body = || RunEventBody::MessageFinalized {
            message_id: message,
        };

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
            Timestamp::from_unix_ms(0).expect("timestamp"),
            Sensitivity::Internal,
            body(),
        )
        .expect_err("message finalized requires model correlations");
        RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session,
            lane,
            run,
            Some(turn),
            Some(request),
            None,
            Some(effect),
            None,
            1,
            0,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            Sensitivity::Confidential,
            body(),
        )
        .expect_err("message finalized must be internal");
        RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session,
            lane,
            run,
            Some(turn),
            Some(request),
            None,
            Some(effect),
            None,
            1,
            0,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            Sensitivity::Internal,
            body(),
        )
        .expect("valid message finalized");
    }
}
