//! Journal drafts, envelopes, and owned record bodies (TDD §12.1–§12.2).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::effects::{
    EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectRequested,
    InteractionCancelled, InteractionExpired, InteractionRequest, InteractionResolution,
};
use crate::ids::{AppendBatchId, EventId, LaneId, RecordId, RunId, SessionId};
use crate::run::RunAccepted;
use crate::time::Timestamp;

/// V1 atomic append batch record-count ceiling (TDD §6.5).
pub const APPEND_BATCH_MAX_RECORDS: usize = 256;
/// Current record envelope format version.
pub const RECORD_FORMAT_VERSION: u16 = 1;
/// Current kind version for PR-008-owned bodies.
pub const RECORD_KIND_VERSION: u16 = 1;

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
    /// # Errors
    ///
    /// Returns [`RecordError`] when format/kind versions are unsupported or
    /// `derived_event_ids` length does not match the ordinal table.
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
        if format_version != RECORD_FORMAT_VERSION {
            return Err(RecordError::UnsupportedFormatVersion { format_version });
        }
        if kind_version != RECORD_KIND_VERSION {
            return Err(RecordError::UnsupportedKindVersion { kind_version });
        }
        let expected = body.derived_event_count(kind_version)?;
        if derived_event_ids.len() != expected {
            return Err(RecordError::DerivedEventCount {
                expected,
                actual: derived_event_ids.len(),
            });
        }
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
            derived_event_ids: Vec<EventId>,
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
            wire.derived_event_ids,
            wire.body,
        )
        .map_err(de::Error::custom)
    }
}

/// Committed durable envelope shape (digests filled by protocol/PR-039).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordEnvelope {
    /// Envelope format version.
    pub format_version: u16,
    /// Body kind version.
    pub kind_version: u16,
    /// Record id.
    pub record_id: RecordId,
    /// Session id.
    pub session_id: SessionId,
    /// Lane id.
    pub lane_id: LaneId,
    /// Optional run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Store-assigned sequence.
    pub sequence: u64,
    /// Semantic timestamp.
    pub timestamp: Timestamp,
    /// Optional store commit time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<Timestamp>,
    /// Payload digest (protocol-computed).
    pub payload_digest: crate::digest::Digest,
    /// Previous checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_checksum: Option<crate::digest::Digest>,
    /// Envelope checksum (protocol-computed).
    pub checksum: crate::digest::Digest,
    /// Derived event ids.
    pub derived_event_ids: Arc<[EventId]>,
    /// Body.
    pub body: RecordBody,
}

/// PR-008-owned record bodies only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordBody {
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
}

impl RecordBody {
    /// Number of derived durable events for `kind_version`.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError::UnsupportedKindVersion`] when unknown.
    pub fn derived_event_count(&self, kind_version: u16) -> Result<usize, RecordError> {
        if kind_version != RECORD_KIND_VERSION {
            return Err(RecordError::UnsupportedKindVersion { kind_version });
        }
        Ok(1)
    }

    /// Stable body kind name.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::RunAccepted(_) => "run_accepted",
            Self::EffectRequested(_) => "effect_requested",
            Self::EffectDeferred(_) => "effect_deferred",
            Self::EffectCompleted(_) => "effect_completed",
            Self::EffectFailed(_) => "effect_failed",
            Self::EffectCancelled(_) => "effect_cancelled",
            Self::InteractionRequested(_) => "interaction_requested",
            Self::InteractionResolved(_) => "interaction_resolved",
            Self::InteractionExpired(_) => "interaction_expired",
            Self::InteractionCancelled(_) => "interaction_cancelled",
        }
    }
}

/// Append request with expected-sequence precondition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppendRequest {
    batch_id: AppendBatchId,
    session_id: SessionId,
    expected_sequence: u64,
    records: Arc<[RecordDraft]>,
}

impl AppendRequest {
    /// Construct an append request enforcing the batch record-count ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError::BatchTooLarge`] when over [`APPEND_BATCH_MAX_RECORDS`].
    pub fn try_new(
        batch_id: AppendBatchId,
        session_id: SessionId,
        expected_sequence: u64,
        records: Vec<RecordDraft>,
    ) -> Result<Self, RecordError> {
        if records.len() > APPEND_BATCH_MAX_RECORDS {
            return Err(RecordError::BatchTooLarge {
                len: records.len(),
                max: APPEND_BATCH_MAX_RECORDS,
            });
        }
        Ok(Self {
            batch_id,
            session_id,
            expected_sequence,
            records: records.into(),
        })
    }

    /// Batch id.
    #[must_use]
    pub fn batch_id(&self) -> AppendBatchId {
        self.batch_id
    }

    /// Session id.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Expected sequence precondition.
    #[must_use]
    pub fn expected_sequence(&self) -> u64 {
        self.expected_sequence
    }

    /// Records.
    #[must_use]
    pub fn records(&self) -> &[RecordDraft] {
        &self.records
    }
}

impl<'de> Deserialize<'de> for AppendRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            batch_id: AppendBatchId,
            session_id: SessionId,
            expected_sequence: u64,
            records: Vec<RecordDraft>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.batch_id,
            wire.session_id,
            wire.expected_sequence,
            wire.records,
        )
        .map_err(de::Error::custom)
    }
}

/// Record construction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecordError {
    /// Unsupported envelope format version.
    #[error("unsupported record format_version {format_version}")]
    UnsupportedFormatVersion {
        /// Version.
        format_version: u16,
    },
    /// Unsupported body kind version.
    #[error("unsupported record kind_version {kind_version}")]
    UnsupportedKindVersion {
        /// Version.
        kind_version: u16,
    },
    /// Derived event id count mismatch.
    #[error("derived_event_ids len {actual} != expected {expected}")]
    DerivedEventCount {
        /// Expected.
        expected: usize,
        /// Actual.
        actual: usize,
    },
    /// Append batch exceeded record count.
    #[error("append batch has {len} records; max {max}")]
    BatchTooLarge {
        /// Length.
        len: usize,
        /// Max.
        max: usize,
    },
}

impl RecordError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedFormatVersion { .. } => "unsupported_format_version",
            Self::UnsupportedKindVersion { .. } => "unsupported_kind_version",
            Self::DerivedEventCount { .. } => "derived_event_count",
            Self::BatchTooLarge { .. } => "batch_too_large",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Digest;
    use crate::ids::EventId;
    use crate::limits::RunLimits;
    use crate::refs::PrincipalRef;
    use crate::run::{
        BudgetPropagation, CancellationPropagation, DeadlinePropagation, PrincipalPropagation,
        RunPropagationPolicy, RunRelation, RunSecurityContext,
    };

    fn sample_run_accepted() -> RunAccepted {
        let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run");
        RunAccepted::try_new(
            run,
            RunRelation::root(run).expect("rel"),
            RunSecurityContext::try_new(
                "tenant",
                PrincipalRef::try_new("iss", "sub", Some("tenant")).expect("p"),
                "oidc",
                "high",
                "policy",
                "decision",
                None,
            )
            .expect("sec"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br"{}"),
            None,
        )
        .expect("accepted")
    }

    #[test]
    fn draft_requires_one_derived_event() {
        let record_id = RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("r");
        let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("s");
        let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("l");
        let event = EventId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("e");
        let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run");
        let body = RecordBody::RunAccepted(sample_run_accepted());
        assert!(
            RecordDraft::try_new(
                RECORD_FORMAT_VERSION,
                RECORD_KIND_VERSION,
                record_id,
                session,
                lane,
                Some(run),
                Timestamp::from_unix_ms(0).expect("ts"),
                vec![],
                body.clone(),
            )
            .is_err()
        );
        let draft = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            record_id,
            session,
            lane,
            Some(run),
            Timestamp::from_unix_ms(0).expect("ts"),
            vec![event],
            body,
        )
        .expect("draft");
        assert_eq!(draft.derived_event_ids().len(), 1);
    }
}
