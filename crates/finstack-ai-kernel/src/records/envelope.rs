//! Committed durable record envelopes.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{EventId, LaneId, RecordId, RunId, SessionId};

use super::body::RecordBody;
use super::error::RecordError;
use super::validate::validate_body_for_creation;
use super::validate::{validate_record_run_id, validate_versions_and_events};

/// Committed durable envelope shape (digests filled by protocol/PR-039).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordEnvelope {
    /// Envelope format version.
    pub(super) format_version: u16,
    /// Body kind version.
    pub(super) kind_version: u16,
    /// Record id.
    pub(super) record_id: RecordId,
    /// Session id.
    pub(super) session_id: SessionId,
    /// Lane id.
    pub(super) lane_id: LaneId,
    /// Optional run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) run_id: Option<RunId>,
    /// Store-assigned sequence.
    pub(super) sequence: u64,
    /// Semantic timestamp.
    pub(super) timestamp: Timestamp,
    /// Optional store commit time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) committed_at: Option<Timestamp>,
    /// Payload digest (protocol-computed).
    pub(super) payload_digest: crate::primitives::Digest,
    /// Previous checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) previous_checksum: Option<crate::primitives::Digest>,
    /// Envelope checksum (protocol-computed).
    pub(super) checksum: crate::primitives::Digest,
    /// Derived event ids.
    pub(super) derived_event_ids: Arc<[EventId]>,
    /// Body.
    pub(super) body: RecordBody,
}

impl RecordEnvelope {
    /// Construct a committed envelope with validated semantic versions and event ordinals.
    ///
    /// Digest and checksum calculation/verification remains owned by PR-039.
    ///
    /// # Arguments
    ///
    /// * `format_version` - Envelope format version. Must be the supported v1 value.
    /// * `kind_version` - Body kind version. Must be the supported v1 value.
    /// * `record_id` - Stable record identity.
    /// * `session_id` - Session that owns the record.
    /// * `lane_id` - Lane that owns the record.
    /// * `run_id` - Optional run scope; required for run-scoped bodies.
    /// * `sequence` - Store-assigned session sequence.
    /// * `timestamp` - Semantic event time.
    /// * `committed_at` - Optional store commit time; `None` when unknown.
    /// * `payload_digest` - Protocol-computed payload digest.
    /// * `previous_checksum` - Previous envelope checksum; `None` for the first record.
    /// * `checksum` - Protocol-computed envelope checksum.
    /// * `derived_event_ids` - Derived event identities; length must match the
    ///   ordinal table for `body`.
    /// * `body` - Durable record payload.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when versions or derived-event cardinality are invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     Digest, LaneCreated, LaneId, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
    ///     RecordEnvelope, RecordId, SessionId, Timestamp,
    /// };
    ///
    /// # let record_id = RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let session_id = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("id");
    /// # let lane_id = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("id");
    /// let envelope = RecordEnvelope::try_new(
    ///     RECORD_FORMAT_VERSION,
    ///     RECORD_KIND_VERSION,
    ///     record_id,
    ///     session_id,
    ///     lane_id,
    ///     None,
    ///     1,
    ///     Timestamp::from_unix_ms(0).expect("ts"),
    ///     None,
    ///     Digest::raw_json(b"payload"),
    ///     None,
    ///     Digest::raw_json(b"checksum"),
    ///     vec![],
    ///     RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
    /// )
    /// .expect("envelope");
    /// assert_eq!(envelope.sequence(), 1);
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        format_version: u16,
        kind_version: u16,
        record_id: RecordId,
        session_id: SessionId,
        lane_id: LaneId,
        run_id: Option<RunId>,
        sequence: u64,
        timestamp: Timestamp,
        committed_at: Option<Timestamp>,
        payload_digest: crate::primitives::Digest,
        previous_checksum: Option<crate::primitives::Digest>,
        checksum: crate::primitives::Digest,
        derived_event_ids: Vec<EventId>,
        body: RecordBody,
    ) -> Result<Self, RecordError> {
        validate_versions_and_events(format_version, kind_version, derived_event_ids.len(), &body)?;
        validate_record_run_id(run_id, &body)?;
        validate_body_for_creation(&body)?;
        Ok(Self {
            format_version,
            kind_version,
            record_id,
            session_id,
            lane_id,
            run_id,
            sequence,
            timestamp,
            committed_at,
            payload_digest,
            previous_checksum,
            checksum,
            derived_event_ids: derived_event_ids.into(),
            body,
        })
    }

    /// Envelope format version.
    #[must_use]
    pub fn format_version(&self) -> u16 {
        self.format_version
    }

    /// Body kind version.
    #[must_use]
    pub fn kind_version(&self) -> u16 {
        self.kind_version
    }

    /// Record id.
    #[must_use]
    pub fn record_id(&self) -> RecordId {
        self.record_id
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

    /// Optional run id.
    #[must_use]
    pub fn run_id(&self) -> Option<RunId> {
        self.run_id
    }

    /// Store-assigned sequence.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Semantic timestamp.
    #[must_use]
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Optional diagnostic commit timestamp.
    #[must_use]
    pub fn committed_at(&self) -> Option<Timestamp> {
        self.committed_at
    }

    /// Protocol-computed payload digest.
    #[must_use]
    pub fn payload_digest(&self) -> crate::primitives::Digest {
        self.payload_digest
    }

    /// Previous envelope checksum.
    #[must_use]
    pub fn previous_checksum(&self) -> Option<crate::primitives::Digest> {
        self.previous_checksum
    }

    /// Protocol-computed envelope checksum.
    #[must_use]
    pub fn checksum(&self) -> crate::primitives::Digest {
        self.checksum
    }

    /// Replay-stable derived event ids.
    #[must_use]
    pub fn derived_event_ids(&self) -> &[EventId] {
        &self.derived_event_ids
    }

    /// Record body.
    #[must_use]
    pub fn body(&self) -> &RecordBody {
        &self.body
    }
}

impl<'de> Deserialize<'de> for RecordEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            format_version: u16,
            kind_version: u16,
            record_id: RecordId,
            session_id: SessionId,
            lane_id: LaneId,
            #[serde(default)]
            run_id: Option<RunId>,
            sequence: u64,
            timestamp: Timestamp,
            #[serde(default)]
            committed_at: Option<Timestamp>,
            payload_digest: crate::primitives::Digest,
            #[serde(default)]
            previous_checksum: Option<crate::primitives::Digest>,
            checksum: crate::primitives::Digest,
            derived_event_ids: BoundedVec<EventId, SEMANTIC_ARRAY_MAX_ITEMS>,
            body: RecordBody,
        }

        let wire = Wire::deserialize(deserializer)?;
        let derived_event_ids = wire.derived_event_ids.into_inner();
        validate_versions_and_events(
            wire.format_version,
            wire.kind_version,
            derived_event_ids.len(),
            &wire.body,
        )
        .map_err(de::Error::custom)?;
        validate_record_run_id(wire.run_id, &wire.body).map_err(de::Error::custom)?;
        let body = match wire.body {
            RecordBody::RunAccepted(accepted) => {
                RecordBody::RunAccepted(accepted.mark_persisted_lineage_validated())
            }
            body => body,
        };
        Ok(Self {
            format_version: wire.format_version,
            kind_version: wire.kind_version,
            record_id: wire.record_id,
            session_id: wire.session_id,
            lane_id: wire.lane_id,
            run_id: wire.run_id,
            sequence: wire.sequence,
            timestamp: wire.timestamp,
            committed_at: wire.committed_at,
            payload_digest: wire.payload_digest,
            previous_checksum: wire.previous_checksum,
            checksum: wire.checksum,
            derived_event_ids: derived_event_ids.into(),
            body,
        })
    }
}
