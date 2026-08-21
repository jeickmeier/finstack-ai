//! Remote session vocabulary. Distinct from process and WIT enums (ADR-014).

use core::fmt;

use finstack_ai_kernel::{
    AcceptRun, AgentId, BundleId, CancelRequested, ContentBlock, Digest, EffectId, EventId,
    ExternalEffectCompletionCommand, Id, IdTag, InteractionResolutionCommand, KernelState, LaneId,
    Metadata, ModelRequestId, RunEvent, RunEventBody, RunEventClass, RunEventKind, RunId,
    Sensitivity, SessionId, Timestamp, ToolBatchId, ToolCallId, TurnId, label_is_valid,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::encode;
use crate::error::ProtocolError;
use crate::wire::VersionOffer;

/// Domain for authenticated remote-command digests.
const DOMAIN_REMOTE_COMMAND: &str = "remote-command";
/// Schema version embedded in the remote-command digest domain.
const REMOTE_COMMAND_DIGEST_SCHEMA_VERSION: u32 = 1;

/// Bounded pre-authentication remote messages (TDD §28.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemotePreAuth {
    /// Client hello / version offer.
    ClientHello {
        /// Supported versions, downgrade floor, and features.
        offer: VersionOffer,
    },
    /// Server hello / selected version.
    ServerHello {
        /// Selected protocol version.
        selected_version: u16,
        /// Server offer used for the selection.
        offer: VersionOffer,
    },
    /// Authentication request. Bearer tokens are transport-restricted by the server.
    Authenticate {
        /// Authentication method.
        method: RemoteAuthMethod,
    },
    /// Authentication result.
    AuthResult {
        /// Whether authentication succeeded.
        accepted: bool,
        /// Stable reason when rejected.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_code: Option<String>,
    },
    /// Close the connection.
    Close {
        /// Stable close reason.
        reason_code: String,
    },
}

/// Authentication method carried in [`RemotePreAuth::Authenticate`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteAuthMethod {
    /// Bearer secret. Rejected on plaintext TCP.
    Bearer {
        /// Opaque credential. Never written to audit events.
        token: String,
    },
    /// Loopback-only method with no bearer secret.
    Loopback,
}

impl fmt::Debug for RemoteAuthMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bearer { .. } => formatter
                .debug_struct("Bearer")
                .field("token", &"[REDACTED]")
                .finish(),
            Self::Loopback => formatter.write_str("Loopback"),
        }
    }
}

/// Post-authentication remote session messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemotePostAuth {
    /// Open or resume a session at an optional durable cursor.
    OpenSession {
        /// Optional last-known durable sequence.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_known_durable_sequence: Option<u64>,
        /// Session/lane/run locator.
        locator: RemoteLocator,
        /// Authenticated tenant scope.
        tenant_scope: String,
    },
    /// Authoritative snapshot at sequence `S`.
    Snapshot {
        /// Snapshot sequence.
        sequence: u64,
        /// Public snapshot projection.
        snapshot: Box<RemoteSnapshot>,
    },
    /// Explicit absence of a snapshot.
    NoSnapshot {
        /// Cursor the tail will start after.
        sequence: u64,
    },
    /// Verified durable tail records `S+1..=B`.
    DurableTail {
        /// Inclusive start sequence.
        from_sequence: u64,
        /// Inclusive end sequence.
        to_sequence: u64,
        /// Contiguous durable record steps, including zero-event records.
        steps: Vec<RemoteDurableStep>,
    },
    /// Sync barrier at durable sequence `B`. Live events may follow only after this.
    SyncBarrier {
        /// Barrier sequence.
        sequence: u64,
    },
    /// Public `RunEvent` projection batch.
    EventBatch {
        /// Live public events. Durable events are carried by `DurableTail`.
        #[serde(deserialize_with = "deserialize_live_events")]
        events: Vec<RemoteEventView>,
    },
    /// State-changing command.
    Command {
        /// Command body.
        command: RemoteCommand,
    },
    /// Command receipt or failure.
    CommandResult {
        /// Result body.
        result: RemoteCommandResult,
    },
    /// Credit grant for flow control.
    Grant {
        /// Item credits.
        items: u32,
        /// Byte credits.
        bytes: u32,
    },
    /// Credit acknowledgement.
    Ack {
        /// Acknowledged durable or batch cursor.
        cursor: u64,
        /// Items consumed.
        items: u32,
        /// Bytes consumed.
        bytes: u32,
    },
    /// Close the session connection.
    Close {
        /// Stable close reason.
        reason_code: String,
    },
}

impl<'de> Deserialize<'de> for RemotePostAuth {
    #[expect(
        clippy::too_many_lines,
        reason = "the explicit tagged-map decoder preserves non-human nested serde semantics"
    )]
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PostAuthVisitor;

        impl<'de> serde::de::Visitor<'de> for PostAuthVisitor {
            type Value = RemotePostAuth;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a canonical remote post-auth message map")
            }

            #[expect(
                clippy::too_many_lines,
                reason = "all bounded post-auth variants are decoded in one audited dispatch"
            )]
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let key = map
                    .next_key::<String>()?
                    .ok_or_else(|| serde::de::Error::custom("missing post-auth kind"))?;
                if key != "kind" {
                    return Err(serde::de::Error::custom(
                        "post-auth kind must be the first canonical field",
                    ));
                }
                let kind = map.next_value::<String>()?;
                let remainder = serde::de::value::MapAccessDeserializer::new(map);
                match kind.as_str() {
                    "open_session" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            last_known_durable_sequence: Option<u64>,
                            locator: RemoteLocator,
                            tenant_scope: String,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::OpenSession {
                            last_known_durable_sequence: body.last_known_durable_sequence,
                            locator: body.locator,
                            tenant_scope: body.tenant_scope,
                        })
                    }
                    "snapshot" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            sequence: u64,
                            snapshot: RemoteSnapshot,
                        }
                        let body = Body::deserialize(remainder)?;
                        if body.snapshot.sequence() != body.sequence {
                            return Err(serde::de::Error::custom(
                                "snapshot outer sequence mismatch",
                            ));
                        }
                        Ok(RemotePostAuth::Snapshot {
                            sequence: body.sequence,
                            snapshot: Box::new(body.snapshot),
                        })
                    }
                    "no_snapshot" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            sequence: u64,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::NoSnapshot {
                            sequence: body.sequence,
                        })
                    }
                    "durable_tail" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            from_sequence: u64,
                            to_sequence: u64,
                            steps: Vec<RemoteDurableStep>,
                        }
                        let body = Body::deserialize(remainder)?;
                        validate_durable_tail(body.from_sequence, body.to_sequence, &body.steps)
                            .map_err(serde::de::Error::custom)?;
                        Ok(RemotePostAuth::DurableTail {
                            from_sequence: body.from_sequence,
                            to_sequence: body.to_sequence,
                            steps: body.steps,
                        })
                    }
                    "sync_barrier" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            sequence: u64,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::SyncBarrier {
                            sequence: body.sequence,
                        })
                    }
                    "event_batch" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            #[serde(deserialize_with = "deserialize_live_events")]
                            events: Vec<RemoteEventView>,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::EventBatch {
                            events: body.events,
                        })
                    }
                    "command" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            command: RemoteCommand,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::Command {
                            command: body.command,
                        })
                    }
                    "command_result" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            result: RemoteCommandResult,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::CommandResult {
                            result: body.result,
                        })
                    }
                    "grant" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            items: u32,
                            bytes: u32,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::Grant {
                            items: body.items,
                            bytes: body.bytes,
                        })
                    }
                    "ack" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            cursor: u64,
                            items: u32,
                            bytes: u32,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::Ack {
                            cursor: body.cursor,
                            items: body.items,
                            bytes: body.bytes,
                        })
                    }
                    "close" => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Body {
                            reason_code: String,
                        }
                        let body = Body::deserialize(remainder)?;
                        Ok(RemotePostAuth::Close {
                            reason_code: body.reason_code,
                        })
                    }
                    _ => Err(serde::de::Error::custom("unknown post-auth kind")),
                }
            }
        }

        deserializer.deserialize_map(PostAuthVisitor)
    }
}

fn validate_durable_tail(
    from_sequence: u64,
    to_sequence: u64,
    steps: &[RemoteDurableStep],
) -> Result<(), ProtocolError> {
    if from_sequence == 0 || from_sequence > to_sequence || steps.is_empty() {
        return Err(ProtocolError::invalid_message("invalid_durable_tail_range"));
    }
    let expected_len = to_sequence
        .checked_sub(from_sequence)
        .and_then(|value| value.checked_add(1))
        .and_then(|value| usize::try_from(value).ok());
    if expected_len != Some(steps.len())
        || steps
            .iter()
            .enumerate()
            .any(|(index, step)| step.sequence() != from_sequence + index as u64)
    {
        return Err(ProtocolError::invalid_message(
            "non_contiguous_durable_tail",
        ));
    }
    Ok(())
}

fn deserialize_live_events<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<RemoteEventView>, D::Error> {
    let events = Vec::<RemoteEventView>::deserialize(deserializer)?;
    let mut previous = None;
    for event in &events {
        if event.durable_sequence().is_some()
            || previous.is_some_and(|value| event.transient_sequence() <= value)
        {
            return Err(serde::de::Error::custom("invalid live event batch"));
        }
        previous = Some(event.transient_sequence());
    }
    Ok(events)
}

impl RemotePostAuth {
    /// Stable `snake_case` kind matching the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::OpenSession { .. } => "open_session",
            Self::Snapshot { .. } => "snapshot",
            Self::NoSnapshot { .. } => "no_snapshot",
            Self::DurableTail { .. } => "durable_tail",
            Self::SyncBarrier { .. } => "sync_barrier",
            Self::EventBatch { .. } => "event_batch",
            Self::Command { .. } => "command",
            Self::CommandResult { .. } => "command_result",
            Self::Grant { .. } => "grant",
            Self::Ack { .. } => "ack",
            Self::Close { .. } => "close",
        }
    }
}

/// Public locator. Never a store-private handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[expect(
    clippy::struct_field_names,
    reason = "session_id/lane_id/run_id are the shared semantic field names"
)]
pub struct RemoteLocator {
    session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    lane_id: Option<LaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
}

impl RemoteLocator {
    /// Construct a locator from public identifiers.
    ///
    /// # Errors
    ///
    /// Rejects a run locator without its containing lane.
    pub fn try_new(
        session_id: SessionId,
        lane_id: Option<LaneId>,
        run_id: Option<RunId>,
    ) -> Result<Self, ProtocolError> {
        if run_id.is_some() && lane_id.is_none() {
            return Err(ProtocolError::invalid_message("run_requires_lane"));
        }
        Ok(Self {
            session_id,
            lane_id,
            run_id,
        })
    }

    /// Public session identifier.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Public lane identifier when present.
    #[must_use]
    pub const fn lane_id(&self) -> Option<LaneId> {
        self.lane_id
    }

    /// Public run identifier when present.
    #[must_use]
    pub const fn run_id(&self) -> Option<RunId> {
        self.run_id
    }
}

impl<'de> Deserialize<'de> for RemoteLocator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[expect(
            clippy::struct_field_names,
            reason = "these are the shared semantic locator field names"
        )]
        struct Wire {
            session_id: SessionId,
            lane_id: Option<LaneId>,
            run_id: Option<RunId>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.session_id, wire.lane_id, wire.run_id).map_err(serde::de::Error::custom)
    }
}

/// Validated authoritative kernel snapshot used for reconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteSnapshot {
    locator: RemoteLocator,
    sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_checksum: Option<Digest>,
    state_hash: Digest,
    state: KernelState,
}

impl RemoteSnapshot {
    /// Construct an authoritative snapshot after checking every projection.
    ///
    /// # Errors
    ///
    /// Rejects sequence, checksum, state-hash, or locator identity mismatches.
    pub fn try_new(
        locator: RemoteLocator,
        sequence: u64,
        head_checksum: Option<Digest>,
        state_hash: Digest,
        state: KernelState,
    ) -> Result<Self, ProtocolError> {
        if (sequence == 0) == head_checksum.is_some() {
            return Err(ProtocolError::invalid_message(
                "snapshot_checksum_sequence_mismatch",
            ));
        }
        if state.last_applied_sequence != sequence {
            return Err(ProtocolError::invalid_message(
                "snapshot_state_sequence_mismatch",
            ));
        }
        let computed = state
            .state_hash()
            .map_err(|_| ProtocolError::invalid_message("snapshot_state_invalid"))?;
        if computed != state_hash {
            return Err(ProtocolError::invalid_message(
                "snapshot_state_hash_mismatch",
            ));
        }
        let state_run_id = state
            .accepted
            .as_ref()
            .map(finstack_ai_kernel::RunAccepted::run_id);
        let locator_mismatch = state
            .session_id
            .is_some_and(|value| value != locator.session_id())
            || state
                .lane_id
                .is_some_and(|value| Some(value) != locator.lane_id())
            || state_run_id.is_some_and(|value| Some(value) != locator.run_id());
        if locator_mismatch {
            return Err(ProtocolError::invalid_message("snapshot_locator_mismatch"));
        }
        Ok(Self {
            locator,
            sequence,
            head_checksum,
            state_hash,
            state,
        })
    }

    /// Snapshot locator.
    #[must_use]
    pub const fn locator(&self) -> &RemoteLocator {
        &self.locator
    }

    /// Authoritative durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Checksum of the durable head, absent only at sequence zero.
    #[must_use]
    pub const fn head_checksum(&self) -> Option<Digest> {
        self.head_checksum
    }

    /// Canonical hash of the embedded kernel state.
    #[must_use]
    pub const fn state_hash(&self) -> Digest {
        self.state_hash
    }

    /// Complete authoritative kernel state.
    #[must_use]
    pub const fn state(&self) -> &KernelState {
        &self.state
    }
}

impl<'de> Deserialize<'de> for RemoteSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            locator: RemoteLocator,
            sequence: u64,
            head_checksum: Option<Digest>,
            state_hash: Digest,
            state: KernelState,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.locator,
            wire.sequence,
            wire.head_checksum,
            wire.state_hash,
            wire.state,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Validated full runtime event used on the socket.
///
/// Credential-classified events are rejected at this boundary. The server must
/// apply its configured maximum-sensitivity policy before construction.
#[repr(transparent)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEventView(RunEvent);

/// One durable journal step and all public events derived from that record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteDurableStep {
    sequence: u64,
    events: Vec<RemoteEventView>,
}

impl RemoteDurableStep {
    /// Construct a step after checking every event's durable sequence.
    ///
    /// # Errors
    ///
    /// Rejects sequence zero and events attributed to another record.
    pub fn try_new(sequence: u64, events: Vec<RemoteEventView>) -> Result<Self, ProtocolError> {
        if sequence == 0 {
            return Err(ProtocolError::invalid_message("zero_durable_sequence"));
        }
        if events
            .iter()
            .any(|event| event.durable_sequence() != Some(sequence))
        {
            return Err(ProtocolError::invalid_message(
                "durable_event_sequence_mismatch",
            ));
        }
        Ok(Self { sequence, events })
    }

    /// Durable journal sequence represented by this step.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Events derived from the record, possibly empty.
    #[must_use]
    pub fn events(&self) -> &[RemoteEventView] {
        &self.events
    }

    /// Consume the step and return its derived events.
    #[must_use]
    pub fn into_events(self) -> Vec<RemoteEventView> {
        self.events
    }
}

impl<'de> Deserialize<'de> for RemoteDurableStep {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            sequence: u64,
            events: Vec<RemoteEventView>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.sequence, wire.events).map_err(serde::de::Error::custom)
    }
}

impl RemoteEventView {
    /// Construct a remote event after enforcing the credential boundary.
    ///
    /// # Errors
    ///
    /// Rejects credential-classified events.
    pub fn try_new(event: RunEvent) -> Result<Self, ProtocolError> {
        if event.sensitivity() == Sensitivity::Credential {
            return Err(ProtocolError::invalid_message("credential_event_forbidden"));
        }
        Ok(Self(event))
    }

    /// Public event identity.
    #[must_use]
    pub fn event_id(&self) -> EventId {
        self.0.event_id()
    }

    /// Typed public event kind.
    #[must_use]
    pub fn kind(&self) -> RunEventKind {
        self.0.kind()
    }

    /// Durable sequence when the event is durable-derived.
    #[must_use]
    pub fn durable_sequence(&self) -> Option<u64> {
        self.0.durable_sequence()
    }

    /// Transient sequence.
    #[must_use]
    pub fn transient_sequence(&self) -> u64 {
        self.0.transient_sequence()
    }

    /// Borrow the complete validated kernel event.
    #[must_use]
    pub const fn event(&self) -> &RunEvent {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RemoteEventView {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
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
            turn_id: Option<TurnId>,
            model_request_id: Option<ModelRequestId>,
            tool_batch_id: Option<ToolBatchId>,
            effect_id: Option<EffectId>,
            tool_call_id: Option<ToolCallId>,
            durable_sequence: Option<u64>,
            transient_sequence: u64,
            timestamp: i64,
            sensitivity: Sensitivity,
            body: RunEventBody,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.body.kind() != wire.kind {
            return Err(serde::de::Error::custom("run event kind/body mismatch"));
        }
        let timestamp =
            Timestamp::from_unix_ms(wire.timestamp).map_err(serde::de::Error::custom)?;
        let event = match wire.kind.class() {
            RunEventClass::DurableDerived => RunEvent::try_durable(
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
                wire.durable_sequence
                    .ok_or_else(|| serde::de::Error::custom("missing durable sequence"))?,
                wire.transient_sequence,
                timestamp,
                wire.sensitivity,
                wire.body,
            ),
            RunEventClass::Transient => {
                if wire.durable_sequence.is_some() {
                    return Err(serde::de::Error::custom(
                        "transient event has durable sequence",
                    ));
                }
                RunEvent::try_transient(
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
                    timestamp,
                    wire.sensitivity,
                    wire.body,
                )
            }
        }
        .map_err(serde::de::Error::custom)?;
        Self::try_new(event).map_err(serde::de::Error::custom)
    }
}

impl Serialize for RemoteEventView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
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
            timestamp: i64,
            sensitivity: Sensitivity,
            body: &'a RunEventBody,
        }
        Wire {
            schema_version: self.0.schema_version(),
            kind_version: self.0.kind_version(),
            event_id: self.0.event_id(),
            kind: self.0.kind(),
            session_id: self.0.session_id(),
            lane_id: self.0.lane_id(),
            run_id: self.0.run_id(),
            turn_id: self.0.turn_id(),
            model_request_id: self.0.model_request_id(),
            tool_batch_id: self.0.tool_batch_id(),
            effect_id: self.0.effect_id(),
            tool_call_id: self.0.tool_call_id(),
            durable_sequence: self.0.durable_sequence(),
            transient_sequence: self.0.transient_sequence(),
            timestamp: self.0.timestamp().as_unix_ms(),
            sensitivity: self.0.sensitivity(),
            body: self.0.body(),
        }
        .serialize(serializer)
    }
}

/// Exact resolved agent identity carried by a remote start command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteAgentRef {
    /// Resolved agent identity.
    pub agent_id: AgentId,
    /// Bundle supplying the agent, when bundle-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<BundleId>,
    /// Canonical resolved specification digest.
    pub spec_digest: Digest,
}

/// Complete normalized request for a remote run start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteStartRequest {
    /// Validated kernel acceptance input.
    pub accept: AcceptRun,
    /// Exact resolved agent.
    pub agent: RemoteAgentRef,
    /// Child input blocks.
    pub input: Vec<ContentBlock>,
    /// Bounded non-authoritative metadata.
    pub metadata: Metadata,
    /// Optional non-secret delegation reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation_id: Option<String>,
}

impl RemoteStartRequest {
    /// Construct a bounded normalized start request.
    ///
    /// # Errors
    ///
    /// Rejects an invalid delegation label.
    pub fn try_new(
        accept: AcceptRun,
        agent: RemoteAgentRef,
        input: Vec<ContentBlock>,
        metadata: Metadata,
        delegation_id: Option<String>,
    ) -> Result<Self, ProtocolError> {
        if delegation_id
            .as_deref()
            .is_some_and(|value| !label_is_valid(value))
        {
            return Err(ProtocolError::invalid_message("invalid_delegation_id"));
        }
        Ok(Self {
            accept,
            agent,
            input,
            metadata,
            delegation_id,
        })
    }
}

impl<'de> Deserialize<'de> for RemoteStartRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            accept: AcceptRun,
            agent: RemoteAgentRef,
            input: Vec<ContentBlock>,
            metadata: Metadata,
            delegation_id: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.accept,
            wire.agent,
            wire.input,
            wire.metadata,
            wire.delegation_id,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Data-bearing commands accepted by the remote session boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCommandPayload {
    /// Start a run from a complete normalized request.
    Start(Box<RemoteStartRequest>),
    /// Request durable cancellation.
    Cancel(Box<CancelRequested>),
    /// Resolve one exact outstanding interaction.
    Resolve(Box<InteractionResolutionCommand>),
    /// Complete one exact externally deferred effect.
    Complete(Box<ExternalEffectCompletionCommand>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RemoteCommandTag {}

impl IdTag for RemoteCommandTag {
    const NAME: &'static str = "remote-command";
}

/// Validated `UUIDv7` identity for one logical remote command.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteCommandId(Id<RemoteCommandTag>);

impl RemoteCommandId {
    /// Parse a canonical lowercase `UUIDv7` string.
    ///
    /// # Errors
    ///
    /// Rejects malformed, non-canonical, non-RFC-variant, or non-v7 UUIDs.
    pub fn parse(value: &str) -> Result<Self, ProtocolError> {
        let id = Id::<RemoteCommandTag>::parse(value)
            .map_err(|_| ProtocolError::invalid_message("invalid_command_id"))?;
        if id.to_canonical_string() != value {
            return Err(ProtocolError::invalid_message("non_canonical_command_id"));
        }
        let bytes = id.as_bytes();
        if bytes[6] >> 4 != 7 || bytes[8] >> 6 != 2 {
            return Err(ProtocolError::invalid_message("command_id_not_uuidv7"));
        }
        Ok(Self(id))
    }

    /// Canonical lowercase hyphenated UUID text.
    #[must_use]
    pub fn to_canonical_string(self) -> String {
        self.0.to_canonical_string()
    }
}

impl fmt::Display for RemoteCommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

impl fmt::Debug for RemoteCommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RemoteCommandId")
            .field(&self.0)
            .finish()
    }
}

impl Serialize for RemoteCommandId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RemoteCommandId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Authenticated remote command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteCommand {
    command_id: RemoteCommandId,
    locator: RemoteLocator,
    tenant_scope: String,
    expected_durable_sequence: u64,
    digest: Digest,
    payload: RemoteCommandPayload,
}

impl RemoteCommand {
    /// Construct a command and compute its digest from the normalized fields.
    ///
    /// # Errors
    ///
    /// Returns codec failures while hashing the normalized command.
    pub fn try_new(
        command_id: impl AsRef<str>,
        locator: RemoteLocator,
        tenant_scope: impl Into<String>,
        expected_durable_sequence: u64,
        payload: RemoteCommandPayload,
    ) -> Result<Self, ProtocolError> {
        let command_id = RemoteCommandId::parse(command_id.as_ref())?;
        let tenant_scope = tenant_scope.into();
        validate_command_payload(&locator, &tenant_scope, &payload)?;
        let digest = command_digest(
            command_id,
            &locator,
            &tenant_scope,
            expected_durable_sequence,
            &payload,
        )?;
        Ok(Self {
            command_id,
            locator,
            tenant_scope,
            expected_durable_sequence,
            digest,
            payload,
        })
    }

    /// `UUIDv7` command identity.
    #[must_use]
    pub const fn command_id(&self) -> RemoteCommandId {
        self.command_id
    }

    /// Public locator.
    #[must_use]
    pub const fn locator(&self) -> &RemoteLocator {
        &self.locator
    }

    /// Authenticated tenant scope.
    #[must_use]
    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    /// Normalized command digest.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Durable cursor that must still be current for a new command.
    #[must_use]
    pub const fn expected_durable_sequence(&self) -> u64 {
        self.expected_durable_sequence
    }

    /// Complete normalized command payload.
    #[must_use]
    pub const fn payload(&self) -> &RemoteCommandPayload {
        &self.payload
    }
}

impl<'de> Deserialize<'de> for RemoteCommand {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            command_id: RemoteCommandId,
            locator: RemoteLocator,
            tenant_scope: String,
            expected_durable_sequence: u64,
            digest: Digest,
            payload: RemoteCommandPayload,
        }

        let wire = Wire::deserialize(deserializer)?;
        validate_command_payload(&wire.locator, &wire.tenant_scope, &wire.payload)
            .map_err(serde::de::Error::custom)?;
        let expected = command_digest(
            wire.command_id,
            &wire.locator,
            &wire.tenant_scope,
            wire.expected_durable_sequence,
            &wire.payload,
        )
        .map_err(serde::de::Error::custom)?;
        if expected != wire.digest {
            return Err(serde::de::Error::custom("remote command digest mismatch"));
        }
        Ok(Self {
            command_id: wire.command_id,
            locator: wire.locator,
            tenant_scope: wire.tenant_scope,
            expected_durable_sequence: wire.expected_durable_sequence,
            digest: wire.digest,
            payload: wire.payload,
        })
    }
}

fn validate_command_payload(
    locator: &RemoteLocator,
    tenant_scope: &str,
    payload: &RemoteCommandPayload,
) -> Result<(), ProtocolError> {
    if !label_is_valid(tenant_scope) {
        return Err(ProtocolError::invalid_message("invalid_tenant_scope"));
    }
    let matches_operation = |operation: &finstack_ai_kernel::OperationLocator| {
        operation.tenant_scope.as_ref() == tenant_scope
            && operation.session_id == locator.session_id()
            && Some(operation.lane_id) == locator.lane_id()
            && Some(operation.run_id) == locator.run_id()
    };
    let valid = match payload {
        RemoteCommandPayload::Start(request) => {
            request.accept.session_id == locator.session_id()
                && Some(request.accept.lane_id) == locator.lane_id()
                && Some(request.accept.accepted.run_id()) == locator.run_id()
                && request.accept.accepted.security().tenant_scope() == tenant_scope
                && request.accept.accepted.resolved_agent_lock_digest() == request.agent.spec_digest
        }
        RemoteCommandPayload::Cancel(_) => true,
        RemoteCommandPayload::Resolve(command) => matches_operation(&command.locator),
        RemoteCommandPayload::Complete(command) => matches_operation(&command.locator),
    };
    if !valid {
        return Err(ProtocolError::invalid_message("command_locator_mismatch"));
    }
    Ok(())
}

/// Command receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteCommandResult {
    command_id: RemoteCommandId,
    digest: Digest,
    durable_sequence: u64,
    accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
}

impl RemoteCommandResult {
    /// Construct a command result.
    ///
    /// # Errors
    ///
    /// Rejects accepted results with a reason and rejected results without one.
    pub fn try_new(
        command_id: RemoteCommandId,
        digest: Digest,
        durable_sequence: u64,
        accepted: bool,
        reason_code: Option<String>,
    ) -> Result<Self, ProtocolError> {
        if accepted == reason_code.is_some() {
            return Err(ProtocolError::invalid_message(
                "invalid_command_result_status",
            ));
        }
        Ok(Self {
            command_id,
            digest,
            durable_sequence,
            accepted,
            reason_code,
        })
    }

    /// Command identity.
    #[must_use]
    pub const fn command_id(&self) -> RemoteCommandId {
        self.command_id
    }

    /// Command digest echoed on the receipt.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Durable cursor after acceptance, rejection, or exact replay.
    #[must_use]
    pub const fn durable_sequence(&self) -> u64 {
        self.durable_sequence
    }

    /// Whether the command was accepted or replayed.
    #[must_use]
    pub const fn accepted(&self) -> bool {
        self.accepted
    }

    /// Stable failure reason.
    #[must_use]
    pub fn reason_code(&self) -> Option<&str> {
        self.reason_code.as_deref()
    }
}

impl<'de> Deserialize<'de> for RemoteCommandResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            command_id: RemoteCommandId,
            digest: Digest,
            durable_sequence: u64,
            accepted: bool,
            reason_code: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.command_id,
            wire.digest,
            wire.durable_sequence,
            wire.accepted,
            wire.reason_code,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Domain-separated digest of the normalized command excluding the digest field.
///
/// # Errors
///
/// Returns codec or digest-domain failures.
fn command_digest(
    command_id: RemoteCommandId,
    locator: &RemoteLocator,
    tenant_scope: &str,
    expected_durable_sequence: u64,
    payload: &RemoteCommandPayload,
) -> Result<Digest, ProtocolError> {
    #[derive(Serialize)]
    struct View<'a> {
        command_id: RemoteCommandId,
        locator: &'a RemoteLocator,
        tenant_scope: &'a str,
        expected_durable_sequence: u64,
        payload: &'a RemoteCommandPayload,
    }
    let bytes = encode(&View {
        command_id,
        locator,
        tenant_scope,
        expected_durable_sequence,
        payload,
    })?;
    Digest::domain_separated(
        DOMAIN_REMOTE_COMMAND,
        REMOTE_COMMAND_DIGEST_SCHEMA_VERSION,
        &bytes,
    )
    .map_err(|err| ProtocolError::codec(err.to_string()))
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{
        AcceptRun, BudgetPropagation, CancellationPropagation, DeadlinePropagation, Digest,
        EventId, KernelState, Metadata, PrincipalPropagation, PrincipalRef, QueueDepthWarning,
        RunAccepted, RunEvent, RunEventBody, RunLimits, RunPropagationPolicy, RunRelation,
        RunSecurityContext, Sensitivity, UNIX_EPOCH,
    };

    use super::{
        RemoteAgentRef, RemoteAuthMethod, RemoteCommand, RemoteCommandPayload, RemoteEventView,
        RemoteLocator, RemotePostAuth, RemotePreAuth, RemoteSnapshot, RemoteStartRequest,
    };
    use crate::wire::{PayloadFamily, VersionOffer, encode_envelope};
    use crate::{decode, encode};

    fn locator() -> RemoteLocator {
        RemoteLocator::try_new(
            "01234567-89ab-7cde-89ab-0123456789ab"
                .parse()
                .expect("session id"),
            Some(
                "11234567-89ab-7cde-89ab-0123456789ab"
                    .parse()
                    .expect("lane id"),
            ),
            Some(
                "21234567-89ab-7cde-89ab-0123456789ab"
                    .parse()
                    .expect("run id"),
            ),
        )
        .expect("locator")
    }

    fn start_payload() -> RemoteCommandPayload {
        let locator = locator();
        let run_id = locator.run_id().expect("run id");
        let spec_digest = Digest::raw_json(br#"{"agent":"fixture"}"#);
        let accepted = RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("root"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
                "loopback",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            spec_digest,
            None,
        )
        .expect("accepted");
        RemoteCommandPayload::Start(Box::new(
            RemoteStartRequest::try_new(
                AcceptRun {
                    session_id: locator.session_id(),
                    lane_id: locator.lane_id().expect("lane id"),
                    accepted,
                },
                RemoteAgentRef {
                    agent_id: finstack_ai_kernel::AgentId::parse("agent.fixture").expect("agent"),
                    bundle_id: None,
                    spec_digest,
                },
                Vec::new(),
                Metadata::empty(),
                None,
            )
            .expect("start"),
        ))
    }

    #[test]
    fn unknown_post_auth_variant_is_rejected() {
        #[derive(serde::Serialize)]
        struct Unknown {
            kind: &'static str,
        }
        let bytes = encode(&Unknown { kind: "store_page" }).expect("encode");
        assert!(decode::<RemotePostAuth>(&bytes).is_err());
    }

    #[test]
    fn command_digest_is_stable() {
        let first = RemoteCommand::try_new(
            "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
            locator(),
            "tenant-a",
            0,
            start_payload(),
        )
        .expect("first");
        let second = RemoteCommand::try_new(
            "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
            locator(),
            "tenant-a",
            0,
            start_payload(),
        )
        .expect("second");
        assert_eq!(first.digest(), second.digest());
    }

    #[test]
    fn locator_requires_a_lane_for_a_run() {
        let session_id = "01234567-89ab-7cde-89ab-0123456789ab"
            .parse()
            .expect("session id");
        let run_id = "11234567-89ab-7cde-89ab-0123456789ab"
            .parse()
            .expect("run id");
        assert_eq!(
            RemoteLocator::try_new(session_id, None, Some(run_id))
                .expect_err("missing lane")
                .code(),
            "run_requires_lane"
        );
    }

    #[test]
    fn credential_events_are_rejected_without_redaction() {
        let locator = locator();
        let event = RunEvent::try_transient(
            1,
            1,
            EventId::from_bytes([9; 16]),
            locator.session_id(),
            locator.lane_id().expect("lane"),
            locator.run_id().expect("run"),
            None,
            None,
            None,
            None,
            None,
            1,
            UNIX_EPOCH,
            Sensitivity::Credential,
            RunEventBody::QueueDepthWarning(QueueDepthWarning { depth: 1, limit: 2 }),
        )
        .expect("kernel event");
        assert_eq!(
            RemoteEventView::try_new(event)
                .expect_err("credential")
                .code(),
            "credential_event_forbidden"
        );
    }

    #[test]
    fn full_snapshot_round_trips_through_post_auth() {
        let state = KernelState::default();
        let state_hash = state.state_hash().expect("state hash");
        let snapshot = RemoteSnapshot::try_new(
            RemoteLocator::try_new(
                "01234567-89ab-7cde-89ab-0123456789ab"
                    .parse()
                    .expect("session"),
                None,
                None,
            )
            .expect("locator"),
            0,
            None,
            state_hash,
            state,
        )
        .expect("snapshot");
        let message = RemotePostAuth::Snapshot {
            sequence: 0,
            snapshot: Box::new(snapshot),
        };
        let bytes = encode(&message).expect("encode");
        let decoded = decode::<RemotePostAuth>(&bytes).expect("decode");
        assert_eq!(decoded, message);
    }

    #[test]
    fn durable_tail_rejects_empty_and_non_contiguous_ranges() {
        let empty = RemotePostAuth::DurableTail {
            from_sequence: 1,
            to_sequence: 1,
            steps: Vec::new(),
        };
        assert!(decode::<RemotePostAuth>(&encode(&empty).expect("encode")).is_err());
    }

    #[test]
    fn command_decode_rejects_non_v7_ids_and_forged_digests() {
        assert_eq!(
            super::RemoteCommandId::parse("0192e0f6-7c3a-6c11-8a4d-2b6e9c1d0a11")
                .expect_err("v6")
                .code(),
            "command_id_not_uuidv7"
        );
        let command = RemoteCommand::try_new(
            "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
            locator(),
            "tenant-a",
            0,
            start_payload(),
        )
        .expect("command");
        let mut value = crate::decode_value(&encode(&command).expect("encode")).expect("tree");
        let crate::CanonicalValue::Map(entries) = &mut value else {
            panic!("command map");
        };
        for (key, value) in entries {
            if *key == crate::CanonicalValue::Text("expected_durable_sequence".into()) {
                *value = crate::CanonicalValue::Unsigned(1);
            }
        }
        assert!(decode::<RemoteCommand>(&crate::encode_value(&value).expect("tamper")).is_err());
    }

    #[test]
    fn bearer_debug_is_redacted_at_every_nesting_level() {
        let secret = "top-secret-token";
        let method = RemoteAuthMethod::Bearer {
            token: secret.into(),
        };
        let direct = format!("{method:?}");
        let nested = format!("{:?}", RemotePreAuth::Authenticate { method });
        assert!(!direct.contains(secret));
        assert!(!nested.contains(secret));
        assert!(direct.contains("[REDACTED]"));
        assert!(nested.contains("[REDACTED]"));
    }

    #[test]
    fn process_hello_body_is_not_a_remote_command() {
        let offer = VersionOffer::try_new(vec![1], 1, vec!["auth".into()]).expect("offer");
        let process = crate::process::ProcessPreAuth::ProcessClientHello { offer };
        let body = encode(&process).expect("body");
        assert!(decode::<RemotePostAuth>(&body).is_err());
        assert!(decode::<RemotePreAuth>(&body).is_err());
        let _ = RemoteAuthMethod::Loopback;
        let _ = encode_envelope(PayloadFamily::Process, 1, &process).expect("env");
    }

    #[test]
    fn post_auth_kind_matches_serde_tag() {
        assert_eq!(
            RemotePostAuth::NoSnapshot { sequence: 0 }.kind(),
            "no_snapshot"
        );
        assert_eq!(
            RemotePostAuth::Close {
                reason_code: "x".into()
            }
            .kind(),
            "close"
        );
    }
}
