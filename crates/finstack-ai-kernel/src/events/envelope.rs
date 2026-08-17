//! Public runtime event envelope.

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::Sensitivity;
use crate::primitives::Timestamp;
use crate::primitives::{
    EffectId, EventId, LaneId, ModelRequestId, RunId, SessionId, ToolBatchId, ToolCallId, TurnId,
};
use crate::records::RecordEnvelope;

use super::body::RunEventBody;
use super::derive::{
    derived_event_kind, derived_event_sensitivity, effect_id_for_body, is_model_effect_record,
    is_tool_effect_record, record_correlations, run_event_body_from_record,
    validate_event_correlations, validate_event_policy, validate_event_versions,
};
use super::{EventError, RUN_EVENT_SCHEMA_VERSION, RunEventClass, RunEventKind};

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

pub(super) struct ResolvedEventCorrelations {
    pub(super) turn: Option<TurnId>,
    pub(super) model_request: Option<ModelRequestId>,
    pub(super) tool_batch: Option<ToolBatchId>,
    pub(super) effect: Option<EffectId>,
    pub(super) tool_call: Option<ToolCallId>,
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
    ///
    /// # Arguments
    ///
    /// * `record` - Committed source envelope. Must be run-scoped.
    /// * `ordinal` - Zero-based derived-event ordinal on that record.
    /// * `transient_sequence` - Runtime sequencer value assigned to this event.
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
