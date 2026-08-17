//! Pre-commit semantic record drafts.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{EventId, LaneId, RecordId, RunId, SessionId};
use crate::records::run::{RunAccepted, RunRelationKind};

use super::body::RecordBody;
use super::error::RecordError;
use super::validate::{
    validate_body_for_creation, validate_record_run_id, validate_versions_and_events,
};

/// Pre-commit semantic record draft (no sequence/digests/checksums).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordDraft {
    format_version: u16,
    kind_version: u16,
    record_id: RecordId,
    session_id: SessionId,
    lane_id: LaneId,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    timestamp: Timestamp,
    derived_event_ids: Arc<[EventId]>,
    body: RecordBody,
}

impl RecordDraft {
    /// Construct a draft with validated derived-event ordinal cardinality.
    ///
    /// # Arguments
    ///
    /// * `format_version` - Envelope format version. Must be the supported v1 value.
    /// * `kind_version` - Body kind version. Must be the supported v1 value.
    /// * `record_id` - Stable record identity allocated for this draft.
    /// * `session_id` - Session that will own the committed record.
    /// * `lane_id` - Lane that will own the committed record.
    /// * `run_id` - Optional run scope; required for run-scoped bodies.
    /// * `timestamp` - Semantic event time.
    /// * `derived_event_ids` - Derived event identities; length must match the
    ///   ordinal table for `body`.
    /// * `body` - Durable record payload.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when format/kind versions are unsupported or
    /// `derived_event_ids` length does not match the ordinal table.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     LaneCreated, LaneId, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft,
    ///     RecordId, SessionId, Timestamp,
    /// };
    ///
    /// # let record_id = RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let session_id = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("id");
    /// # let lane_id = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("id");
    /// let draft = RecordDraft::try_new(
    ///     RECORD_FORMAT_VERSION,
    ///     RECORD_KIND_VERSION,
    ///     record_id,
    ///     session_id,
    ///     lane_id,
    ///     None,
    ///     Timestamp::from_unix_ms(0).expect("ts"),
    ///     vec![],
    ///     RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
    /// )
    /// .expect("draft");
    /// assert!(draft.run_id().is_none());
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        format_version: u16,
        kind_version: u16,
        record_id: RecordId,
        session_id: SessionId,
        lane_id: LaneId,
        run_id: Option<RunId>,
        timestamp: Timestamp,
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
            timestamp,
            derived_event_ids: derived_event_ids.into(),
            body,
        })
    }

    /// Format version.
    #[must_use]
    pub fn format_version(&self) -> u16 {
        self.format_version
    }

    /// Kind version.
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

    /// Run id.
    #[must_use]
    pub fn run_id(&self) -> Option<RunId> {
        self.run_id
    }

    /// Semantic timestamp.
    #[must_use]
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Derived event ids.
    #[must_use]
    pub fn derived_event_ids(&self) -> &[EventId] {
        &self.derived_event_ids
    }

    /// Body.
    #[must_use]
    pub fn body(&self) -> &RecordBody {
        &self.body
    }

    /// Validate a structurally decoded child `RunAccepted` body against its parent.
    ///
    /// Other record bodies and root runs are returned unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError::Run`] when child lineage or attenuation is invalid.
    pub fn validate_run_lineage(mut self, parent: &RunAccepted) -> Result<Self, RecordError> {
        self.body = match self.body {
            RecordBody::RunAccepted(accepted)
                if accepted.relation().kind() != RunRelationKind::Root =>
            {
                RecordBody::RunAccepted(accepted.validate_against_parent(parent)?)
            }
            body => body,
        };
        Ok(self)
    }
}

impl<'de> Deserialize<'de> for RecordDraft {
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
            timestamp: Timestamp,
            derived_event_ids: BoundedVec<EventId, SEMANTIC_ARRAY_MAX_ITEMS>,
            body: RecordBody,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.format_version,
            wire.kind_version,
            wire.record_id,
            wire.session_id,
            wire.lane_id,
            wire.run_id,
            wire.timestamp,
            wire.derived_event_ids.into_inner(),
            wire.body,
        )
        .map_err(de::Error::custom)
    }
}
