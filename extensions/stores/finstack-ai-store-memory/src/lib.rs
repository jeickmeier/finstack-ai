//! Bounded, explicitly non-durable in-memory [`JournalStore`] implementation.
//!
//! Envelopes are encoded and verified through `finstack-ai-protocol`. Health
//! remains non-durable (`durable = false`).

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use finstack_ai_kernel::{
    AppendBatchId, AppendRequest, CommittedBatch, Digest, Metadata, RecordDraft, RecordEnvelope,
    RecordId, SessionId,
};
use finstack_ai_protocol::{
    ProtocolError, commit_records, decode_opaque_snapshot, encode_snapshot, verify_chain,
    verify_chain_from,
};
use finstack_ai_runtime::{
    AcceleratedRestore, JournalStore, LoadFromRequest, LoadRequest, LoadWindow, LoadedSession,
    MetadataReceipt, OpaqueSnapshot, PortFuture, PruneReceipt, PruneRequest, SCAN_PAGE_MAX_RECORDS,
    ScanPage, ScanRequest, SnapshotReceipt, SnapshotRequest, StateSnapshotRequest, StoreError,
    StoreHealth, StoreLimits, WriteMetadataRequest,
};

/// Required resource ceilings for [`MemoryJournalStore`].
pub type MemoryStoreLimits = StoreLimits;

/// Mutex-protected ordered-map journal intended for tests and embedded ephemeral use.
pub struct MemoryJournalStore {
    limits: MemoryStoreLimits,
    inner: Mutex<Inner>,
}

impl MemoryJournalStore {
    /// Construct a store with explicit resource ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] when any ceiling is zero.
    pub fn try_new(limits: MemoryStoreLimits) -> Result<Self, StoreError> {
        Ok(Self {
            limits: limits.validate()?,
            inner: Mutex::new(Inner::default()),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Unavailable {
            reason_code: "memory_store_lock_poisoned",
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the store contract requires one visible ordered idempotency, conflict, limit, and atomic-commit sequence"
    )]
    fn append_sync(&self, request: AppendRequest) -> Result<CommittedBatch, StoreError> {
        let mut inner = self.lock()?;

        if let Some(existing) = inner.batches_by_id.get(&request.batch_id()) {
            return if existing.request == request {
                Ok(existing.committed.clone())
            } else {
                Err(StoreError::Corruption {
                    reason_code: "append_batch_id_reuse",
                })
            };
        }

        if request.records().is_empty() {
            return Err(StoreError::InvalidRequest {
                reason_code: "empty_append_batch",
            });
        }

        let reused = request
            .records()
            .iter()
            .filter_map(|record| inner.records_by_id.get(&record.record_id()))
            .collect::<Vec<_>>();
        if !reused.is_empty() {
            if reused.len() != request.records().len() {
                return Err(StoreError::Corruption {
                    reason_code: "mixed_record_id_reuse",
                });
            }
            let original_batch_id = reused[0].batch_id;
            if reused
                .iter()
                .any(|entry| entry.batch_id != original_batch_id)
            {
                return Err(StoreError::Corruption {
                    reason_code: "mixed_record_batch_reuse",
                });
            }
            let existing =
                inner
                    .batches_by_id
                    .get(&original_batch_id)
                    .ok_or(StoreError::Integrity {
                        reason_code: "missing_record_batch_index",
                    })?;
            let exact_records = existing.request.session_id() == request.session_id()
                && existing.request.expected_sequence() == request.expected_sequence()
                && existing.request.records() == request.records();
            if exact_records {
                return Ok(existing.committed.clone());
            }
            return Err(StoreError::Corruption {
                reason_code: "record_id_reuse",
            });
        }

        let current_head = inner
            .sessions
            .get(&request.session_id())
            .map_or(0, |session| session.head_sequence);
        let actual_next_sequence = current_head.checked_add(1).ok_or(StoreError::Integrity {
            reason_code: "sequence_exhausted",
        })?;
        if request.expected_sequence() != actual_next_sequence {
            return Err(StoreError::Conflict {
                expected_sequence: request.expected_sequence(),
                actual_next_sequence,
            });
        }

        let session_is_new = !inner.sessions.contains_key(&request.session_id());
        if session_is_new && inner.sessions.len() >= self.limits.sessions {
            return Err(StoreError::LimitExceeded {
                resource: "sessions",
                limit: self.limits.sessions,
            });
        }
        if let Some(session) = inner.sessions.get(&request.session_id()) {
            if session.batches.len() >= self.limits.batches_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "batches_per_session",
                    limit: self.limits.batches_per_session,
                });
            }
            if session.records + request.records().len() > self.limits.records_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "records_per_session",
                    limit: self.limits.records_per_session,
                });
            }
        } else if request.records().len() > self.limits.records_per_session {
            return Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: self.limits.records_per_session,
            });
        }

        let previous_checksum = inner
            .sessions
            .get(&request.session_id())
            .and_then(|session| session.head_checksum);
        let committed = build_committed_batch(&request, previous_checksum)?;
        let last_sequence = committed.last_sequence;
        let head_checksum = committed.records.last().map(RecordEnvelope::checksum);
        let batch_id = request.batch_id();
        let session_id = request.session_id();
        let record_entries = request
            .records()
            .iter()
            .map(|record| {
                (
                    record.record_id(),
                    RecordIndexEntry {
                        batch_id,
                        draft: record.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();

        let session = inner.sessions.entry(session_id).or_default();
        session.head_sequence = last_sequence;
        session.head_checksum = head_checksum;
        session.records += request.records().len();
        session.batches.push(committed.clone());
        session.mark_verified();
        for (record_id, entry) in record_entries {
            inner.records_by_id.insert(record_id, entry);
        }
        inner.batches_by_id.insert(
            batch_id,
            BatchIndexEntry {
                request,
                committed: committed.clone(),
            },
        );
        Ok(committed)
    }

    fn load_sync(&self, request: LoadRequest) -> Result<LoadedSession, StoreError> {
        let inner = self.lock()?;
        let Some(session) = inner.sessions.get(&request.session_id) else {
            return Ok(LoadedSession::empty(request.session_id));
        };
        verify_session(session)?;
        let snapshot = session.snapshot.clone();
        Ok(LoadedSession {
            session_id: request.session_id,
            head_sequence: session.head_sequence,
            head_checksum: session.head_checksum,
            metadata: session.metadata.clone(),
            committed_batches: session.batches.clone().into(),
            snapshot: snapshot.clone(),
            accelerated: snapshot.as_ref().and_then(accelerated_from),
        })
    }

    fn load_from_sync(&self, request: LoadFromRequest) -> Result<LoadedSession, StoreError> {
        match request.window {
            LoadWindow::Full => self.load_sync(LoadRequest {
                session_id: request.session_id,
            }),
            LoadWindow::FromSequence {
                from_sequence,
                prior_checksum,
            } => self.load_from_sequence(request.session_id, from_sequence, prior_checksum),
            LoadWindow::SnapshotPlusTail => self.load_snapshot_plus_tail(request.session_id),
        }
    }

    fn load_from_sequence(
        &self,
        session_id: SessionId,
        from_sequence: u64,
        prior_checksum: Digest,
    ) -> Result<LoadedSession, StoreError> {
        let start = if from_sequence == 0 { 1 } else { from_sequence };
        if start <= 1 {
            return self.load_sync(LoadRequest { session_id });
        }
        let inner = self.lock()?;
        let Some(session) = inner.sessions.get(&session_id) else {
            return Err(StoreError::Integrity {
                reason_code: "load_from_sequence_gap",
            });
        };
        loaded_from_batches(
            session_id,
            session,
            start,
            prior_checksum,
            "load_from_sequence_gap",
            "load_from_splits_batch",
            "load_from_prior_checksum_mismatch",
        )
    }

    fn load_snapshot_plus_tail(&self, session_id: SessionId) -> Result<LoadedSession, StoreError> {
        let inner = self.lock()?;
        let Some(session) = inner.sessions.get(&session_id) else {
            return Ok(LoadedSession::empty(session_id));
        };
        let Some(snapshot) = session.snapshot.as_ref() else {
            drop(inner);
            return self.load_sync(LoadRequest { session_id });
        };
        let Some(accelerated) = accelerated_from(snapshot) else {
            return Err(StoreError::Integrity {
                reason_code: "snapshot_state_invalid",
            });
        };
        let start = accelerated.sequence.saturating_add(1);
        loaded_from_batches(
            session_id,
            session,
            start,
            accelerated.head_checksum,
            "snapshot_missing_record",
            "snapshot_splits_batch",
            "snapshot_checksum_mismatch",
        )
    }

    fn scan_sync(&self, request: ScanRequest) -> Result<ScanPage, StoreError> {
        if request.limit == 0 {
            return Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_zero",
            });
        }
        if request.limit > SCAN_PAGE_MAX_RECORDS {
            return Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_exceeded",
            });
        }
        let mut inner = self.lock()?;
        let Some(session) = inner.sessions.get_mut(&request.session_id) else {
            return Ok(ScanPage {
                session_id: request.session_id,
                records: Arc::from([]),
                next_sequence: None,
            });
        };
        if !session.head_is_verified() {
            verify_session(session)?;
            session.mark_verified();
        }
        let start = if request.from_sequence == 0 {
            1
        } else {
            request.from_sequence
        };
        let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
        let (records, next_sequence) = slice_records(session, start, limit);
        Ok(ScanPage {
            session_id: request.session_id,
            records: records.into(),
            next_sequence,
        })
    }

    fn write_metadata_sync(
        &self,
        request: WriteMetadataRequest,
    ) -> Result<MetadataReceipt, StoreError> {
        let mut inner = self.lock()?;
        let session =
            inner
                .sessions
                .get_mut(&request.session_id)
                .ok_or(StoreError::InvalidRequest {
                    reason_code: "metadata_session_not_found",
                })?;
        if session.head_checksum != request.expected_head_checksum {
            return Err(StoreError::InvalidRequest {
                reason_code: "metadata_cas_mismatch",
            });
        }
        session.metadata = request.metadata.clone();
        Ok(MetadataReceipt {
            session_id: request.session_id,
            head_checksum: session.head_checksum,
            metadata: request.metadata,
        })
    }

    fn write_snapshot_sync(&self, request: SnapshotRequest) -> Result<SnapshotReceipt, StoreError> {
        if request.snapshot.bytes().len() > self.limits.snapshot_bytes {
            return Err(StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: self.limits.snapshot_bytes,
            });
        }
        let mut inner = self.lock()?;
        let session =
            inner
                .sessions
                .get_mut(&request.session_id)
                .ok_or(StoreError::InvalidRequest {
                    reason_code: "snapshot_session_not_found",
                })?;
        if request.snapshot.sequence() > session.head_sequence {
            return Err(StoreError::InvalidRequest {
                reason_code: "snapshot_ahead_of_journal",
            });
        }
        if session
            .snapshot
            .as_ref()
            .is_some_and(|current| current.sequence() > request.snapshot.sequence())
        {
            return Err(StoreError::InvalidRequest {
                reason_code: "snapshot_sequence_regression",
            });
        }
        let receipt = SnapshotReceipt {
            session_id: request.session_id,
            sequence: request.snapshot.sequence(),
            digest: request.snapshot.digest(),
            bytes: request.snapshot.bytes().len(),
        };
        session.snapshot = Some(request.snapshot);
        Ok(receipt)
    }

    fn write_state_snapshot_sync(
        &self,
        request: &StateSnapshotRequest,
    ) -> Result<SnapshotReceipt, StoreError> {
        let snapshot = encode_state_request(request, self.limits.snapshot_bytes)?;
        self.write_snapshot_sync(SnapshotRequest {
            session_id: request.session_id,
            snapshot,
        })
    }

    fn prune_sync(&self, request: PruneRequest) -> Result<PruneReceipt, StoreError> {
        let mut inner = self.lock()?;
        let session =
            inner
                .sessions
                .get_mut(&request.session_id)
                .ok_or(StoreError::InvalidRequest {
                    reason_code: "prune_session_not_found",
                })?;
        let snapshot = session
            .snapshot
            .as_ref()
            .ok_or(StoreError::InvalidRequest {
                reason_code: "prune_requires_snapshot",
            })?;
        if snapshot.sequence() == 0 || snapshot.sequence() > session.head_sequence {
            return Err(StoreError::InvalidRequest {
                reason_code: "prune_snapshot_not_aligned",
            });
        }
        let aligned = session
            .batches
            .iter()
            .any(|batch| batch.last_sequence == snapshot.sequence());
        if !aligned {
            return Err(StoreError::InvalidRequest {
                reason_code: "prune_not_batch_aligned",
            });
        }
        let accelerated = accelerated_from(snapshot).ok_or(StoreError::Integrity {
            reason_code: "prune_snapshot_undecodable",
        })?;
        let retained_outstanding = outstanding_count(&accelerated);
        let retained_tombstones = tombstone_count(&accelerated);
        let pruned_through_sequence = snapshot.sequence();
        session
            .batches
            .retain(|batch| batch.last_sequence >= pruned_through_sequence);
        session.records = session
            .batches
            .iter()
            .map(|batch| batch.records.len())
            .sum();
        session.verified_head = None;
        verify_session(session)?;
        session.mark_verified();
        let _ = request.horizon;
        Ok(PruneReceipt {
            pruned_through_sequence,
            retained_outstanding,
            retained_tombstones,
        })
    }

    /// Drop the disposable snapshot cache for one session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] when the session is missing.
    pub fn discard_snapshot(&self, session_id: SessionId) -> Result<(), StoreError> {
        let mut inner = self.lock()?;
        let session = inner
            .sessions
            .get_mut(&session_id)
            .ok_or(StoreError::InvalidRequest {
                reason_code: "snapshot_session_not_found",
            })?;
        session.snapshot = None;
        Ok(())
    }
}

impl JournalStore for MemoryJournalStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let result = self.append_sync(request);
        Box::pin(async move { result })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let result = self.load_sync(request);
        Box::pin(async move { result })
    }

    fn load_from(&self, request: LoadFromRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let result = self.load_from_sync(request);
        Box::pin(async move { result })
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let result = self.write_snapshot_sync(request);
        Box::pin(async move { result })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("memory_non_durable"),
            })
        })
    }

    fn scan(&self, request: ScanRequest) -> PortFuture<Result<ScanPage, StoreError>> {
        let result = self.scan_sync(request);
        Box::pin(async move { result })
    }

    fn write_metadata(
        &self,
        request: WriteMetadataRequest,
    ) -> PortFuture<Result<MetadataReceipt, StoreError>> {
        let result = self.write_metadata_sync(request);
        Box::pin(async move { result })
    }

    fn write_state_snapshot(
        &self,
        request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let result = self.write_state_snapshot_sync(&request);
        Box::pin(async move { result })
    }

    fn prune(&self, request: PruneRequest) -> PortFuture<Result<PruneReceipt, StoreError>> {
        let result = self.prune_sync(request);
        Box::pin(async move { result })
    }
}

#[derive(Default)]
struct Inner {
    sessions: BTreeMap<SessionId, SessionData>,
    batches_by_id: BTreeMap<AppendBatchId, BatchIndexEntry>,
    records_by_id: BTreeMap<RecordId, RecordIndexEntry>,
}

#[derive(Default)]
struct SessionData {
    head_sequence: u64,
    head_checksum: Option<Digest>,
    metadata: Metadata,
    records: usize,
    batches: Vec<CommittedBatch>,
    snapshot: Option<OpaqueSnapshot>,
    verified_head: Option<(u64, Option<Digest>)>,
}

impl SessionData {
    fn head_is_verified(&self) -> bool {
        self.verified_head == Some((self.head_sequence, self.head_checksum))
    }

    fn mark_verified(&mut self) {
        self.verified_head = Some((self.head_sequence, self.head_checksum));
    }
}

struct BatchIndexEntry {
    request: AppendRequest,
    committed: CommittedBatch,
}

struct RecordIndexEntry {
    batch_id: AppendBatchId,
    #[allow(dead_code)]
    draft: RecordDraft,
}

fn encode_state_request(
    request: &StateSnapshotRequest,
    max_bytes: usize,
) -> Result<OpaqueSnapshot, StoreError> {
    let (bytes, digest) = encode_snapshot(
        &request.state,
        request.state.last_applied_sequence,
        request.head_checksum,
        request.pending_timer_scheduled_at,
        request.last_model_continuation.clone(),
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "snapshot_encode_failed",
    })?;
    OpaqueSnapshot::try_new(
        request.state.last_applied_sequence,
        digest,
        bytes,
        max_bytes,
    )
}

fn accelerated_from(snapshot: &OpaqueSnapshot) -> Option<AcceleratedRestore> {
    let decoded =
        decode_opaque_snapshot(snapshot.sequence(), snapshot.digest(), snapshot.bytes()).ok()?;
    Some(AcceleratedRestore {
        sequence: decoded.sequence,
        head_checksum: decoded.head_checksum,
        pending_timer_scheduled_at: decoded.pending_timer_scheduled_at,
        last_model_continuation: decoded.last_model_continuation,
        state: decoded.state,
    })
}

fn build_committed_batch(
    request: &AppendRequest,
    previous_checksum: Option<Digest>,
) -> Result<CommittedBatch, StoreError> {
    let records = commit_records(
        request.records(),
        request.expected_sequence(),
        previous_checksum,
        None,
    )
    .map_err(|error| protocol_error(&error))?;
    let last_sequence = request
        .expected_sequence()
        .checked_add(
            u64::try_from(records.len() - 1).map_err(|_| StoreError::Integrity {
                reason_code: "record_count_overflow",
            })?,
        )
        .ok_or(StoreError::Integrity {
            reason_code: "sequence_exhausted",
        })?;
    CommittedBatch::try_new(
        request.batch_id(),
        request.expected_sequence(),
        last_sequence,
        records,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "committed_batch_invalid",
    })
}

fn verify_session(session: &SessionData) -> Result<(), StoreError> {
    let records = flatten_records(session);
    let head = match records.first() {
        Some(first) if first.sequence() > 1 => {
            verify_chain_from(&records, first.previous_checksum(), Some(first.sequence()))
                .map_err(|error| protocol_error(&error))?
        }
        _ => verify_chain(&records).map_err(|error| protocol_error(&error))?,
    };
    if head != session.head_checksum {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    Ok(())
}

fn outstanding_count(restored: &AcceleratedRestore) -> u64 {
    let pending_model = u64::from(restored.state.pending_model_effect.is_some());
    let pending_interaction = u64::from(restored.state.pending_interaction.is_some());
    pending_model.saturating_add(pending_interaction)
}

fn tombstone_count(restored: &AcceleratedRestore) -> u64 {
    u64::try_from(
        restored
            .state
            .completion_identities
            .len()
            .saturating_add(restored.state.resolution_identities.len())
            .saturating_add(restored.state.model_settlements.len())
            .saturating_add(restored.state.tool_settlements.len()),
    )
    .unwrap_or(u64::MAX)
}

fn loaded_from_batches(
    session_id: SessionId,
    session: &SessionData,
    start: u64,
    prior_checksum: Digest,
    gap_code: &'static str,
    split_code: &'static str,
    checksum_code: &'static str,
) -> Result<LoadedSession, StoreError> {
    if start > session.head_sequence.saturating_add(1) {
        return Err(StoreError::Integrity {
            reason_code: gap_code,
        });
    }
    if start == session.head_sequence.saturating_add(1) {
        if session.head_checksum != Some(prior_checksum) {
            return Err(StoreError::Integrity {
                reason_code: checksum_code,
            });
        }
        let snapshot = session.snapshot.clone();
        return Ok(LoadedSession {
            session_id,
            head_sequence: session.head_sequence,
            head_checksum: session.head_checksum,
            metadata: session.metadata.clone(),
            committed_batches: Arc::from([]),
            snapshot: snapshot.clone(),
            accelerated: snapshot.as_ref().and_then(accelerated_from),
        });
    }
    let start_index = session
        .batches
        .iter()
        .position(|batch| batch.first_sequence >= start)
        .ok_or(StoreError::Integrity {
            reason_code: gap_code,
        })?;
    if session.batches[start_index].first_sequence != start {
        return Err(StoreError::Integrity {
            reason_code: split_code,
        });
    }
    let first = session.batches[start_index]
        .records
        .first()
        .ok_or(StoreError::Integrity {
            reason_code: gap_code,
        })?;
    if first.previous_checksum() != Some(prior_checksum) {
        return Err(StoreError::Integrity {
            reason_code: checksum_code,
        });
    }
    let tail = session.batches[start_index..].to_vec();
    let records = tail
        .iter()
        .flat_map(|batch| batch.records.iter().cloned())
        .collect::<Vec<_>>();
    let head = verify_chain_from(&records, Some(prior_checksum), Some(start))
        .map_err(|error| protocol_error(&error))?;
    if head != session.head_checksum {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    let snapshot = session.snapshot.clone();
    Ok(LoadedSession {
        session_id,
        head_sequence: session.head_sequence,
        head_checksum: session.head_checksum,
        metadata: session.metadata.clone(),
        committed_batches: tail.into(),
        snapshot: snapshot.clone(),
        accelerated: snapshot.as_ref().and_then(accelerated_from),
    })
}

fn flatten_records(session: &SessionData) -> Vec<RecordEnvelope> {
    session
        .batches
        .iter()
        .flat_map(|batch| batch.records.iter().cloned())
        .collect()
}

fn slice_records(
    session: &SessionData,
    start: u64,
    limit: usize,
) -> (Vec<RecordEnvelope>, Option<u64>) {
    let mut records = Vec::new();
    let mut saw_more = false;
    'batches: for batch in &session.batches {
        for record in batch.records.iter() {
            if record.sequence() < start {
                continue;
            }
            if records.len() == limit {
                saw_more = true;
                break 'batches;
            }
            records.push(record.clone());
        }
    }
    let next_sequence = if saw_more {
        records
            .last()
            .and_then(|record| record.sequence().checked_add(1))
    } else {
        None
    };
    (records, next_sequence)
}

fn protocol_error(error: &ProtocolError) -> StoreError {
    match error {
        ProtocolError::LimitExceeded { resource, limit } => StoreError::LimitExceeded {
            resource,
            limit: *limit,
        },
        ProtocolError::Integrity { reason_code } | ProtocolError::InvalidCbor { reason_code } => {
            StoreError::Integrity { reason_code }
        }
        ProtocolError::Codec { .. } => StoreError::Integrity {
            reason_code: "canonical_codec",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Barrier};
    use std::task::{Context, Poll, Waker};
    use std::thread;

    use finstack_ai_kernel::{
        AuthorizationEvidence, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
        Id, IdTag, LaneCreated, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION,
        RecordBody, RecordDraft, RecordTag, RunTag, SessionCreated, SessionTag, Timestamp,
    };
    use finstack_ai_protocol::{envelope_checksum, payload_digest, verify_envelope};

    use super::*;

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => thread::yield_now(),
            }
        }
    }

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn limits() -> MemoryStoreLimits {
        MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 8,
            records_per_session: 16,
            snapshot_bytes: 1024,
        }
    }

    fn draft(record_ordinal: u64, session_ordinal: u64) -> RecordDraft {
        let principal =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
        let authorization =
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
        let rejection = ExternalCommandRejected::try_new(
            ExternalCommandKind::EffectCompletion,
            format!("completion-{record_ordinal}"),
            ExternalCommandTarget::Effect(id(record_ordinal + 1000)),
            principal,
            authorization,
            "conflicting_completion",
            Digest::raw_json(b"{}"),
            None,
        )
        .expect("rejection");
        RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(record_ordinal),
            id::<SessionTag>(session_ordinal),
            id::<LaneTag>(session_ordinal + 100),
            Some(id::<RunTag>(session_ordinal + 200)),
            Timestamp::from_unix_ms(i64::try_from(record_ordinal).expect("timestamp"))
                .expect("timestamp"),
            Vec::new(),
            RecordBody::ExternalCommandRejected(rejection),
        )
        .expect("draft")
    }

    fn request(
        batch_ordinal: u64,
        session_ordinal: u64,
        expected_sequence: u64,
        drafts: Vec<RecordDraft>,
    ) -> AppendRequest {
        AppendRequest::try_new(
            id(batch_ordinal),
            id::<SessionTag>(session_ordinal),
            expected_sequence,
            drafts,
        )
        .expect("append request")
    }

    #[test]
    fn append_load_and_health_preserve_boundaries_and_non_durability() {
        let store = MemoryJournalStore::try_new(limits()).expect("store");
        let first = block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
        let second = block_on(store.append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)])))
            .expect("append");
        assert_eq!((first.first_sequence, first.last_sequence), (1, 1));
        assert_eq!((second.first_sequence, second.last_sequence), (2, 3));
        verify_envelope(&first.records[0]).expect("first envelope");
        verify_envelope(&second.records[0]).expect("second envelope");
        assert_eq!(
            second.records[0].previous_checksum(),
            Some(first.records[0].checksum())
        );
        assert_eq!(
            first.records[0].payload_digest(),
            payload_digest(first.records[0].body()).expect("payload")
        );
        assert_eq!(
            first.records[0].checksum(),
            envelope_checksum(&first.records[0]).expect("checksum")
        );

        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 3);
        assert_eq!(loaded.head_checksum, Some(second.records[1].checksum()));
        assert_eq!(loaded.metadata, Metadata::empty());
        assert_eq!(loaded.committed_batches.as_ref(), &[first, second]);
        assert!(loaded.snapshot.is_none());

        let health = block_on(store.health()).expect("health");
        assert!(health.ready);
        assert!(!health.durable);
        assert_eq!(health.detail.as_ref(), "memory_non_durable");
    }

    #[test]
    fn load_from_sequence_returns_verified_tail_only() {
        let store = MemoryJournalStore::try_new(limits()).expect("store");
        let first = block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
        let second = block_on(store.append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)])))
            .expect("append");
        let tail = block_on(store.load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 2,
                prior_checksum: first.records[0].checksum(),
            },
        }))
        .expect("tail");
        assert!(tail.omits_prefix());
        assert_eq!(tail.head_sequence, 3);
        assert_eq!(tail.committed_batches.as_ref(), &[second]);
        assert!(matches!(
            block_on(store.load_from(LoadFromRequest {
                session_id: id::<SessionTag>(1),
                window: LoadWindow::FromSequence {
                    from_sequence: 2,
                    prior_checksum: first.records[0].payload_digest(),
                },
            })),
            Err(StoreError::Integrity {
                reason_code: "load_from_prior_checksum_mismatch"
            })
        ));
    }

    #[test]
    fn scan_and_metadata_cas_are_session_local() {
        let store = MemoryJournalStore::try_new(limits()).expect("store");
        block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
        let second = block_on(store.append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)])))
            .expect("append");
        let page = block_on(store.scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 0,
            limit: 2,
        }))
        .expect("scan");
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.next_sequence, Some(3));
        let tail = block_on(store.scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 3,
            limit: 2,
        }))
        .expect("mid scan");
        assert_eq!(tail.records.len(), 1);
        assert_eq!(tail.records[0].sequence(), 3);
        assert_eq!(tail.next_sequence, None);
        assert!(matches!(
            block_on(store.scan(ScanRequest {
                session_id: id::<SessionTag>(1),
                from_sequence: 1,
                limit: 0,
            })),
            Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_zero"
            })
        ));
        assert!(matches!(
            block_on(store.scan(ScanRequest {
                session_id: id::<SessionTag>(1),
                from_sequence: 1,
                limit: SCAN_PAGE_MAX_RECORDS + 1,
            })),
            Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_exceeded"
            })
        ));

        let metadata = Metadata::parse(br#"{"label":"demo"}"#).expect("metadata");
        assert!(matches!(
            block_on(store.write_metadata(WriteMetadataRequest {
                session_id: id::<SessionTag>(1),
                expected_head_checksum: None,
                metadata: metadata.clone(),
            })),
            Err(StoreError::InvalidRequest {
                reason_code: "metadata_cas_mismatch"
            })
        ));
        let receipt = block_on(store.write_metadata(WriteMetadataRequest {
            session_id: id::<SessionTag>(1),
            expected_head_checksum: Some(second.records[1].checksum()),
            metadata: metadata.clone(),
        }))
        .expect("cas");
        assert_eq!(receipt.metadata, metadata);
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.metadata, metadata);
    }

    #[test]
    fn structural_session_and_lane_records_commit_without_run_id() {
        let store = MemoryJournalStore::try_new(limits()).expect("store");
        let session = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(90),
            id::<SessionTag>(9),
            id::<LaneTag>(91),
            None,
            Timestamp::from_unix_ms(1).expect("ts"),
            Vec::new(),
            RecordBody::SessionCreated(SessionCreated::new(Metadata::empty())),
        )
        .expect("session");
        let lane = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(91),
            id::<SessionTag>(9),
            id::<LaneTag>(91),
            None,
            Timestamp::from_unix_ms(1).expect("ts"),
            Vec::new(),
            RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
        )
        .expect("lane");
        let committed =
            block_on(store.append(request(90, 9, 1, vec![session, lane]))).expect("append");
        assert_eq!(committed.records.len(), 2);
        verify_envelope(&committed.records[0]).expect("session envelope");
        verify_envelope(&committed.records[1]).expect("lane envelope");
    }

    #[test]
    fn batch_and_record_idempotency_precede_sequence_checks() {
        let store = MemoryJournalStore::try_new(limits()).expect("store");
        let frozen = request(10, 1, 1, vec![draft(10, 1)]);
        let original = block_on(store.append(frozen.clone())).expect("append");
        assert_eq!(
            block_on(store.append(frozen.clone())).expect("same batch"),
            original
        );

        let same_records_new_batch = AppendRequest::try_new(
            id(11),
            frozen.session_id(),
            frozen.expected_sequence(),
            frozen.records().to_vec(),
        )
        .expect("request");
        assert_eq!(
            block_on(store.append(same_records_new_batch)).expect("record idempotency"),
            original
        );

        let unequal_batch = request(10, 1, 2, vec![draft(11, 1)]);
        assert!(matches!(
            block_on(store.append(unequal_batch)),
            Err(StoreError::Corruption {
                reason_code: "append_batch_id_reuse"
            })
        ));
        let mixed = request(12, 1, 2, vec![draft(10, 1), draft(12, 1)]);
        assert!(matches!(
            block_on(store.append(mixed)),
            Err(StoreError::Corruption {
                reason_code: "mixed_record_id_reuse"
            })
        ));
    }

    #[test]
    fn conflicts_are_atomic_and_concurrent_writers_have_one_winner() {
        let store = Arc::new(MemoryJournalStore::try_new(limits()).expect("store"));
        let barrier = Arc::new(Barrier::new(3));
        let mut joins = Vec::new();
        for ordinal in [20_u64, 21] {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            joins.push(thread::spawn(move || {
                barrier.wait();
                block_on(store.append(request(ordinal, 1, 1, vec![draft(ordinal, 1)])))
            }));
        }
        barrier.wait();
        let results = joins
            .into_iter()
            .map(|join| join.join().expect("writer"))
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(StoreError::Conflict { .. })))
                .count(),
            1
        );

        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 1);
        assert_eq!(loaded.committed_batches.len(), 1);
    }

    #[test]
    fn configured_limits_and_snapshot_cache_are_enforced() {
        assert!(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 0,
                ..limits()
            })
            .is_err()
        );
        let store = MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 1,
            records_per_session: 1,
            snapshot_bytes: 3,
        })
        .expect("store");
        block_on(store.append(request(30, 1, 1, vec![draft(30, 1)]))).expect("append");
        assert!(matches!(
            block_on(store.append(request(31, 1, 2, vec![draft(31, 1)]))),
            Err(StoreError::LimitExceeded {
                resource: "batches_per_session",
                limit: 1
            })
        ));
        assert!(matches!(
            block_on(store.append(request(32, 2, 1, vec![draft(32, 2)]))),
            Err(StoreError::LimitExceeded {
                resource: "sessions",
                limit: 1
            })
        ));

        let oversized =
            OpaqueSnapshot::try_new(1, Digest::raw_json(b"four"), b"four".as_slice(), 8)
                .expect("caller ceiling");
        assert!(matches!(
            block_on(store.write_snapshot(SnapshotRequest {
                session_id: id::<SessionTag>(1),
                snapshot: oversized,
            })),
            Err(StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: 3
            })
        ));

        let snapshot = OpaqueSnapshot::try_new(1, Digest::raw_json(b"one"), b"one".as_slice(), 3)
            .expect("snapshot");
        let receipt = block_on(store.write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: snapshot.clone(),
        }))
        .expect("snapshot write");
        assert_eq!(receipt.bytes, 3);
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.snapshot, Some(snapshot));
    }

    #[test]
    fn record_limit_is_independent_from_batch_limit() {
        let store = MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 3,
            records_per_session: 1,
            snapshot_bytes: 8,
        })
        .expect("store");
        block_on(store.append(request(40, 1, 1, vec![draft(40, 1)]))).expect("append");
        assert!(matches!(
            block_on(store.append(request(41, 1, 2, vec![draft(41, 1)]))),
            Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: 1
            })
        ));
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 1);
        assert_eq!(loaded.committed_batches.len(), 1);
    }
}
