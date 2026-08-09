//! Journal drafts, envelopes, and owned record bodies (TDD §12.1–§12.2).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::agent::{FinalResultRecorded, OutputConfiguration};
use crate::bounds::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::capabilities::CapabilitiesActivated;
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectInput, EffectKind,
    EffectOutputKind, EffectRequested, InteractionCancelled, InteractionExpired,
    InteractionRequest, InteractionResolution,
};
use crate::entries::{
    ContextPrepared, EntryAppended, RetryScheduled, RunCancelled, RunCompleted, RunFailed,
    RunSuspended, StageOutcomeRecorded, TimerFired,
};
use crate::error::ErrorDescriptorError;
use crate::ids::{AppendBatchId, EventId, LaneId, RecordId, RunId, SessionId};
use crate::limits::LimitReached;
use crate::run::{
    CancellationReconciled, CancellationRequested, RunAccepted, RunError, RunRelationKind,
};
use crate::time::Timestamp;
use crate::tools::{ToolBatchClosed, ToolBatchOpened, ToolBatchOutcome, ToolCallSettled};
use crate::validation::OutputValidationFailed;

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

fn validate_versions_and_events(
    format_version: u16,
    kind_version: u16,
    actual_event_count: usize,
    body: &RecordBody,
) -> Result<(), RecordError> {
    if format_version != RECORD_FORMAT_VERSION {
        return Err(RecordError::UnsupportedFormatVersion { format_version });
    }
    if kind_version != RECORD_KIND_VERSION {
        return Err(RecordError::UnsupportedKindVersion { kind_version });
    }
    let expected = body.derived_event_count(kind_version)?;
    if actual_event_count != expected {
        return Err(RecordError::DerivedEventCount {
            expected,
            actual: actual_event_count,
        });
    }
    Ok(())
}

/// Committed durable envelope shape (digests filled by protocol/PR-039).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordEnvelope {
    /// Envelope format version.
    format_version: u16,
    /// Body kind version.
    kind_version: u16,
    /// Record id.
    record_id: RecordId,
    /// Session id.
    session_id: SessionId,
    /// Lane id.
    lane_id: LaneId,
    /// Optional run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    /// Store-assigned sequence.
    sequence: u64,
    /// Semantic timestamp.
    timestamp: Timestamp,
    /// Optional store commit time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    committed_at: Option<Timestamp>,
    /// Payload digest (protocol-computed).
    payload_digest: crate::digest::Digest,
    /// Previous checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_checksum: Option<crate::digest::Digest>,
    /// Envelope checksum (protocol-computed).
    checksum: crate::digest::Digest,
    /// Derived event ids.
    derived_event_ids: Arc<[EventId]>,
    /// Body.
    body: RecordBody,
}

impl RecordEnvelope {
    /// Construct a committed envelope with validated semantic versions and event ordinals.
    ///
    /// Digest and checksum calculation/verification remains owned by PR-039.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when versions or derived-event cardinality are invalid.
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
        payload_digest: crate::digest::Digest,
        previous_checksum: Option<crate::digest::Digest>,
        checksum: crate::digest::Digest,
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
    pub fn payload_digest(&self) -> crate::digest::Digest {
        self.payload_digest
    }

    /// Previous envelope checksum.
    #[must_use]
    pub fn previous_checksum(&self) -> Option<crate::digest::Digest> {
        self.previous_checksum
    }

    /// Protocol-computed envelope checksum.
    #[must_use]
    pub fn checksum(&self) -> crate::digest::Digest {
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
            payload_digest: crate::digest::Digest,
            #[serde(default)]
            previous_checksum: Option<crate::digest::Digest>,
            checksum: crate::digest::Digest,
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

/// Record bodies owned through PR-012.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
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
    /// Aggregate stage outcome.
    StageOutcomeRecorded(StageOutcomeRecorded),
    /// Prepared model context.
    ContextPrepared(ContextPrepared),
    /// Final assistant message append.
    EntryAppended(EntryAppended),
    /// Complete source-ordered tool-batch plan.
    ToolBatchOpened(ToolBatchOpened),
    /// Source-order tool result finalization.
    ToolCallSettled(ToolCallSettled),
    /// Complete tool-batch closure.
    ToolBatchClosed(ToolBatchClosed),
    /// Durable cancellation intent.
    CancellationRequested(CancellationRequested),
    /// Cumulative cancellation reconciliation.
    CancellationReconciled(CancellationReconciled),
    /// Hard limit crossing.
    LimitReached(LimitReached),
    /// Semantic retry and timer intent.
    RetryScheduled(RetryScheduled),
    /// Semantic timer firing.
    TimerFired(TimerFired),
    /// Non-terminal suspension.
    RunSuspended(RunSuspended),
    /// Successful run terminal.
    RunCompleted(RunCompleted),
    /// Failed run terminal.
    RunFailed(RunFailed),
    /// Cancelled run terminal.
    RunCancelled(RunCancelled),
    /// Frozen run-level output configuration.
    OutputConfigured(OutputConfiguration),
    /// Complete immutable resolved capability plan activation.
    CapabilitiesActivated(CapabilitiesActivated),
    /// Validator-independent valid final structured result.
    FinalResultRecorded(FinalResultRecorded),
    /// Validator-independent invalid structured result and retry feedback.
    OutputValidationFailed(OutputValidationFailed),
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
        Ok(match self {
            Self::StageOutcomeRecorded(_)
            | Self::ContextPrepared(_)
            | Self::ToolBatchOpened(_)
            | Self::ToolBatchClosed(_)
            | Self::CancellationRequested(_)
            | Self::CancellationReconciled(_)
            | Self::RetryScheduled(_)
            | Self::TimerFired(_)
            | Self::OutputConfigured(_)
            | Self::CapabilitiesActivated(_)
            | Self::FinalResultRecorded(_)
            | Self::OutputValidationFailed(_) => 0,
            Self::ToolCallSettled(_) => 2,
            _ => 1,
        })
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
            Self::StageOutcomeRecorded(_) => "stage_outcome_recorded",
            Self::ContextPrepared(_) => "context_prepared",
            Self::EntryAppended(_) => "entry_appended",
            Self::ToolBatchOpened(_) => "tool_batch_opened",
            Self::ToolCallSettled(_) => "tool_call_settled",
            Self::ToolBatchClosed(_) => "tool_batch_closed",
            Self::CancellationRequested(_) => "cancellation_requested",
            Self::CancellationReconciled(_) => "cancellation_reconciled",
            Self::LimitReached(_) => "limit_reached",
            Self::RetryScheduled(_) => "retry_scheduled",
            Self::TimerFired(_) => "timer_fired",
            Self::RunSuspended(_) => "run_suspended",
            Self::RunCompleted(_) => "run_completed",
            Self::RunFailed(_) => "run_failed",
            Self::RunCancelled(_) => "run_cancelled",
            Self::OutputConfigured(_) => "output_configured",
            Self::CapabilitiesActivated(_) => "capabilities_activated",
            Self::FinalResultRecorded(_) => "final_result_recorded",
            Self::OutputValidationFailed(_) => "output_validation_failed",
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
        if records
            .iter()
            .any(|record| record.session_id() != session_id)
        {
            return Err(RecordError::RecordSessionMismatch);
        }
        validate_interaction_request_pairs(&records)?;
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
            records: BoundedVec<RecordDraft, APPEND_BATCH_MAX_RECORDS>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.batch_id,
            wire.session_id,
            wire.expected_sequence,
            wire.records.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}

fn validate_interaction_request_pairs(records: &[RecordDraft]) -> Result<(), RecordError> {
    for record in records {
        match record.body() {
            RecordBody::InteractionRequested(request) => {
                let matches = records
                    .iter()
                    .filter(|candidate| {
                        let RecordBody::EffectRequested(effect) = candidate.body() else {
                            return false;
                        };
                        interaction_request_matches_effect(request, effect)
                    })
                    .count();
                if matches != 1 {
                    return Err(RecordError::InvalidInteractionPair);
                }
            }
            RecordBody::EffectRequested(effect) if effect.kind() == EffectKind::Interaction => {
                let matches = records
                    .iter()
                    .filter(|candidate| {
                        let RecordBody::InteractionRequested(request) = candidate.body() else {
                            return false;
                        };
                        interaction_request_matches_effect(request, effect)
                    })
                    .count();
                if matches != 1 {
                    return Err(RecordError::InvalidInteractionPair);
                }
            }
            _ => {}
        }
    }
    let resolved = records
        .iter()
        .filter(|record| matches!(record.body(), RecordBody::InteractionResolved(_)))
        .count();
    let completed = records
        .iter()
        .filter(|record| {
            matches!(
                record.body(),
                RecordBody::EffectCompleted(effect)
                    if effect.output_contract().kind == EffectOutputKind::InteractionResolution
            )
        })
        .count();
    let expired = records
        .iter()
        .filter(|record| matches!(record.body(), RecordBody::InteractionExpired(_)))
        .count();
    let failed = records
        .iter()
        .filter(|record| {
            matches!(
                record.body(),
                RecordBody::EffectFailed(effect)
                    if effect.output_contract().kind == EffectOutputKind::InteractionResolution
            )
        })
        .count();
    let cancelled = records
        .iter()
        .filter(|record| matches!(record.body(), RecordBody::InteractionCancelled(_)))
        .count();
    let effect_cancelled = records
        .iter()
        .filter(|record| {
            matches!(
                record.body(),
                RecordBody::EffectCancelled(effect)
                    if effect.output_contract().kind == EffectOutputKind::InteractionResolution
            )
        })
        .count();
    if resolved != completed || expired != failed || cancelled != effect_cancelled {
        return Err(RecordError::InvalidInteractionPair);
    }
    Ok(())
}

fn interaction_request_matches_effect(
    request: &InteractionRequest,
    effect: &EffectRequested,
) -> bool {
    if effect.effect_id() != request.effect_id() || effect.kind() != EffectKind::Interaction {
        return false;
    }
    let EffectInput::Interaction {
        interaction_id,
        request_digest,
    } = effect.input()
    else {
        return false;
    };
    *interaction_id == request.interaction_id()
        && request
            .request_digest()
            .is_ok_and(|digest| digest == *request_digest)
}

fn validate_body_for_creation(body: &RecordBody) -> Result<(), RecordError> {
    if let RecordBody::RunAccepted(accepted) = body
        && !accepted.lineage_is_validated()
    {
        return Err(RecordError::UnvalidatedRunLineage);
    }
    let error = match body {
        RecordBody::StageOutcomeRecorded(outcome) => match &outcome.disposition {
            crate::StageDisposition::Failed { error } => Some(error),
            _ => None,
        },
        RecordBody::EffectFailed(failed) => Some(failed.error()),
        RecordBody::ToolCallSettled(settled) => settled.error.as_ref(),
        RecordBody::ToolBatchClosed(closed) => match &closed.outcome {
            ToolBatchOutcome::Failed { error } => Some(error),
            ToolBatchOutcome::ContinueModel | ToolBatchOutcome::Finalize => None,
        },
        RecordBody::RunFailed(failed) => Some(&failed.error),
        RecordBody::OutputValidationFailed(failed) => Some(&failed.error),
        _ => None,
    };
    if let Some(error) = error {
        error
            .validate()
            .map_err(RecordError::InvalidErrorDescriptor)?;
    }
    Ok(())
}

fn validate_record_run_id(run_id: Option<RunId>, body: &RecordBody) -> Result<(), RecordError> {
    if let RecordBody::RunAccepted(accepted) = body
        && run_id != Some(accepted.run_id())
    {
        return Err(RecordError::RecordRunMismatch);
    }
    Ok(())
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
    /// A record did not belong to the append request's session.
    #[error("append request record session does not match request session")]
    RecordSessionMismatch,
    /// Interaction request/effect records were missing, duplicated, or mismatched.
    #[error("interaction request must pair with exactly one matching interaction effect")]
    InvalidInteractionPair,
    /// A decoded non-root run was not validated against its parent.
    #[error("non-root RunAccepted lineage must be validated before record construction")]
    UnvalidatedRunLineage,
    /// Record run id did not match the run-scoped body.
    #[error("record run_id does not match RunAccepted body")]
    RecordRunMismatch,
    /// Durable failure descriptor violates semantic limits.
    #[error(transparent)]
    InvalidErrorDescriptor(ErrorDescriptorError),
    /// Child run lineage or attenuation validation failed.
    #[error(transparent)]
    Run(#[from] RunError),
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
            Self::RecordSessionMismatch => "record_session_mismatch",
            Self::InvalidInteractionPair => "invalid_interaction_pair",
            Self::UnvalidatedRunLineage => "unvalidated_run_lineage",
            Self::RecordRunMismatch => "record_run_mismatch",
            Self::InvalidErrorDescriptor(_) => "invalid_error_descriptor",
            Self::Run(inner) => inner.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Digest;
    use crate::effects::{
        EffectInput, EffectKind, EffectOutputContract, InteractionKind, InteractionRequest,
        RetrySafety,
    };
    use crate::ids::{ComponentId, EffectId, EventId, InteractionId};
    use crate::limits::RunLimits;
    use crate::raw_json::{Metadata, RawJson};
    use crate::refs::{ComponentRef, PrincipalRef, Version};
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

    fn sample_child_run_accepted(parent: &RunAccepted) -> RunAccepted {
        let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("run");
        RunAccepted::try_new(
            run,
            RunRelation::try_new(
                parent.run_id(),
                Some(parent.run_id()),
                Some(EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect")),
                RunRelationKind::ChildAgent,
                1,
                None,
                None::<&str>,
            )
            .expect("relation"),
            RunSecurityContext::try_new(
                "tenant",
                PrincipalRef::try_new("iss", "sub", Some("tenant")).expect("principal"),
                "oidc",
                "high",
                "policy",
                "decision",
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
            Digest::raw_json(br"{}"),
            Some(parent),
        )
        .expect("child accepted")
    }

    fn sample_effect_requested() -> EffectRequested {
        EffectRequested::try_new(
            EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect"),
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
        .expect("effect")
    }

    #[test]
    fn record_creation_rejects_programmatically_invalid_failure_descriptor() {
        let mut error = crate::ErrorDescriptor::new(
            "stage_failed",
            "failed",
            crate::ErrorCategory::Middleware,
            false,
        )
        .expect("descriptor");
        error.message = Arc::from("x".repeat(crate::TEXT_MAX_BYTES + 1));
        let result = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("record"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
            Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run")),
            Timestamp::from_unix_ms(0).expect("timestamp"),
            vec![],
            RecordBody::StageOutcomeRecorded(crate::StageOutcomeRecorded {
                cursor: crate::StageCursor {
                    cycle: 0,
                    stage: crate::Stage::BeforeRun,
                },
                disposition: crate::StageDisposition::Failed { error },
                settlement_digest: Digest::raw_json(b"settlement"),
            }),
        )
        .expect_err("invalid descriptor");
        assert_eq!(result.code(), "invalid_error_descriptor");
    }

    #[test]
    fn serialized_child_run_record_replays_as_validated_lineage() {
        let parent = sample_run_accepted();
        let child = sample_child_run_accepted(&parent);
        let record = RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("record"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("lane"),
            Some(child.run_id()),
            1,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            Digest::raw_json(b"payload"),
            None,
            Digest::raw_json(b"checksum"),
            vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("event")],
            RecordBody::RunAccepted(child),
        )
        .expect("record");
        let encoded = serde_json::to_vec(&record).expect("serialize record");
        let decoded: RecordEnvelope = serde_json::from_slice(&encoded).expect("decode record");
        let RecordBody::RunAccepted(decoded_child) = decoded.body() else {
            panic!("run accepted body");
        };
        assert!(decoded_child.lineage_is_validated());

        let batch = crate::CommittedBatch::try_new(
            AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789a5").expect("batch"),
            1,
            1,
            vec![decoded],
        )
        .expect("batch");
        let mut kernel = crate::Kernel::default();
        kernel.apply(&batch, 0).expect("replay child acceptance");
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

    #[test]
    fn effect_record_body_round_trips_human_json_with_raw_input() {
        let body = RecordBody::EffectRequested(sample_effect_requested());
        let json = serde_json::to_string(&body).expect("serialize");
        let round: RecordBody = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round, body);
    }

    #[test]
    fn append_request_rejects_records_from_another_session() {
        let request_session =
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session");
        let record_session =
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session");
        let draft = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("record"),
            record_session,
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("lane"),
            Some(sample_run_accepted().run_id()),
            Timestamp::from_unix_ms(0).expect("timestamp"),
            vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("event")],
            RecordBody::RunAccepted(sample_run_accepted()),
        )
        .expect("draft");
        let error = AppendRequest::try_new(
            AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("batch"),
            request_session,
            0,
            vec![draft],
        )
        .expect_err("mixed sessions");
        assert_eq!(error.code(), "record_session_mismatch");
    }

    #[test]
    fn run_accepted_record_requires_matching_run_id() {
        let accepted = sample_run_accepted();
        let error = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("record"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane"),
            None,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("event")],
            RecordBody::RunAccepted(accepted),
        )
        .expect_err("missing run id");
        assert_eq!(error.code(), "record_run_mismatch");
    }

    #[test]
    fn record_envelope_deserialization_validates_versions_and_event_count() {
        let accepted = sample_run_accepted();
        let envelope = RecordEnvelope {
            format_version: RECORD_FORMAT_VERSION + 1,
            kind_version: RECORD_KIND_VERSION,
            record_id: RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("record"),
            session_id: SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
            lane_id: LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
            run_id: Some(accepted.run_id()),
            sequence: 1,
            timestamp: Timestamp::from_unix_ms(0).expect("timestamp"),
            committed_at: None,
            payload_digest: Digest::raw_json(br"payload"),
            previous_checksum: None,
            checksum: Digest::raw_json(br"checksum"),
            derived_event_ids: Arc::from([]),
            body: RecordBody::RunAccepted(accepted),
        };
        let json = serde_json::to_string(&envelope).expect("serialize");
        assert!(
            serde_json::from_str::<RecordEnvelope>(&json).is_err(),
            "unsupported version and missing derived event id must fail"
        );
    }

    #[test]
    fn append_request_preserves_injected_record_order() {
        let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session");
        let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane");
        let run = sample_run_accepted();
        let records = [
            (
                "01234567-89ab-7cde-89ab-0123456789ad",
                "01234567-89ab-7cde-89ab-0123456789ae",
            ),
            (
                "01234567-89ab-7cde-89ab-0123456789af",
                "01234567-89ab-7cde-89ab-0123456789b0",
            ),
        ]
        .into_iter()
        .map(|(record_id, event_id)| {
            RecordDraft::try_new(
                RECORD_FORMAT_VERSION,
                RECORD_KIND_VERSION,
                RecordId::parse(record_id).expect("record"),
                session,
                lane,
                Some(run.run_id()),
                Timestamp::from_unix_ms(0).expect("timestamp"),
                vec![EventId::parse(event_id).expect("event")],
                RecordBody::RunAccepted(run.clone()),
            )
            .expect("draft")
        })
        .collect::<Vec<_>>();
        let expected = records
            .iter()
            .map(RecordDraft::record_id)
            .collect::<Vec<_>>();
        let request = AppendRequest::try_new(
            AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("batch"),
            session,
            0,
            records,
        )
        .expect("append");
        let actual = request
            .records()
            .iter()
            .map(RecordDraft::record_id)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn append_request_rejects_unpaired_interaction_request() {
        let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session");
        let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane");
        let accepted = sample_run_accepted();
        let interaction_id =
            InteractionId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("interaction");
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("effect");
        let interaction = InteractionRequest::try_new(
            1,
            interaction_id,
            effect_id,
            InteractionKind::Approval,
            vec![],
            RawJson::parse(r#"{"type":"boolean"}"#).expect("schema"),
            ComponentRef::new(
                ComponentId::parse("finstack.policy.approval").expect("component"),
                Some(Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
            ),
            Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            None,
            None,
            false,
            Metadata::empty(),
        )
        .expect("interaction");
        let interaction_digest = interaction.request_digest().expect("digest");
        let draft = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("record"),
            session,
            lane,
            Some(accepted.run_id()),
            Timestamp::from_unix_ms(0).expect("timestamp"),
            vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("event")],
            RecordBody::InteractionRequested(interaction),
        )
        .expect("draft");
        let error = AppendRequest::try_new(
            AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("batch"),
            session,
            0,
            vec![draft.clone()],
        )
        .expect_err("interaction request requires effect request sibling");
        assert_eq!(error.code(), "invalid_interaction_pair");

        let effect = EffectRequested::try_new(
            effect_id,
            EffectKind::Interaction,
            None,
            None,
            None,
            EffectOutputContract {
                kind: EffectOutputKind::InteractionResolution,
                schema_version: 1,
                schema_digest: Digest::raw_json(br"interaction-resolution"),
            },
            EffectInput::Interaction {
                interaction_id,
                request_digest: interaction_digest,
            },
            RetrySafety::AtMostOnce,
            None,
        )
        .expect("effect");
        let effect_draft = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("record"),
            session,
            lane,
            Some(accepted.run_id()),
            Timestamp::from_unix_ms(0).expect("timestamp"),
            vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789b3").expect("event")],
            RecordBody::EffectRequested(effect),
        )
        .expect("effect draft");
        AppendRequest::try_new(
            AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b4").expect("batch"),
            session,
            0,
            vec![effect_draft, draft],
        )
        .expect("paired interaction batch");
    }
}
