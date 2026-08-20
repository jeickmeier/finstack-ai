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
use finstack_ai_runtime::{
    JournalStore, LoadFromRequest, LoadRequest, LoadWindow, LoadedSession, MetadataReceipt,
    OpaqueSnapshot, PortFuture, PruneReceipt, PruneRequest, ScanPage, ScanRequest, SnapshotReceipt,
    SnapshotRequest, StateSnapshotRequest, StoreError, StoreHealth, StoreLimits,
    WriteMetadataRequest,
};
use finstack_ai_store_common::{
    FROM_SEQUENCE_WINDOW, SNAPSHOT_WINDOW, SessionUsage, WindowCodes, accelerated_from,
    admit_append_limits, admit_prune_snapshot, admit_snapshot_sequence, build_committed_batch,
    check_append_sequence, check_snapshot_size, classify_record_reuse, encode_state_request,
    outstanding_count, scan_next_sequence, scan_start, select_tail_batches, tombstone_count,
    validate_scan_limit, verify_full_head, verify_tail_records,
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

        // record-id reuse classification
        let hits = request
            .records()
            .iter()
            .filter_map(|record| inner.records_by_id.get(&record.record_id()))
            .map(|entry| entry.batch_id)
            .collect::<Vec<_>>();
        if let Some(original_batch_id) = classify_record_reuse(&hits, request.records().len())? {
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

        // sequence precondition
        let current_head = inner
            .sessions
            .get(&request.session_id())
            .map_or(0, |session| session.head_sequence);
        check_append_sequence(current_head, request.expected_sequence())?;

        // limits
        let usage = inner
            .sessions
            .get(&request.session_id())
            .map(|session| SessionUsage {
                batches: session.batches.len(),
                records: session.records,
            });
        admit_append_limits(
            self.limits,
            inner.sessions.len(),
            usage,
            request.records().len(),
        )?;

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
            FROM_SEQUENCE_WINDOW,
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
            SNAPSHOT_WINDOW,
        )
    }

    fn scan_sync(&self, request: ScanRequest) -> Result<ScanPage, StoreError> {
        validate_scan_limit(request.limit)?;
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
        let start = scan_start(request.from_sequence);
        let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
        let (records, saw_more) = slice_records(session, start, limit);
        let next_sequence = scan_next_sequence(&records, saw_more);
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
        check_snapshot_size(request.snapshot.bytes().len(), self.limits.snapshot_bytes)?;
        let mut inner = self.lock()?;
        let session =
            inner
                .sessions
                .get_mut(&request.session_id)
                .ok_or(StoreError::InvalidRequest {
                    reason_code: "snapshot_session_not_found",
                })?;
        admit_snapshot_sequence(
            request.snapshot.sequence(),
            session.head_sequence,
            session.snapshot.as_ref().map(OpaqueSnapshot::sequence),
        )?;
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
        admit_prune_snapshot(snapshot.sequence(), session.head_sequence)?;
        // Alignment rule twin: the sqlite store enforces the same
        // "snapshot ends exactly at a batch's last_sequence" predicate via SQL
        // in its prune (store.rs); change both together.
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

fn verify_session(session: &SessionData) -> Result<(), StoreError> {
    let records = flatten_records(session);
    verify_full_head(&records, session.head_checksum).map(|_| ())
}

fn loaded_from_batches(
    session_id: SessionId,
    session: &SessionData,
    start: u64,
    prior_checksum: Digest,
    codes: WindowCodes,
) -> Result<LoadedSession, StoreError> {
    let tail = select_tail_batches(&session.batches, start, codes)?;
    let records = tail
        .iter()
        .flat_map(|batch| batch.records.iter().cloned())
        .collect::<Vec<_>>();
    verify_tail_records(
        &records,
        start,
        prior_checksum,
        session.head_sequence,
        session.head_checksum,
        codes,
    )?;
    let snapshot = session.snapshot.clone();
    Ok(LoadedSession {
        session_id,
        head_sequence: session.head_sequence,
        head_checksum: session.head_checksum,
        metadata: session.metadata.clone(),
        committed_batches: tail.to_vec().into(),
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

fn slice_records(session: &SessionData, start: u64, limit: usize) -> (Vec<RecordEnvelope>, bool) {
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
    (records, saw_more)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Barrier};
    use std::task::{Context, Poll, Waker};
    use std::thread;

    use finstack_ai_kernel::{
        LaneCreated, LaneTag, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft,
        RecordTag, SessionCreated, SessionTag, Timestamp,
    };
    use finstack_ai_protocol::{envelope_checksum, payload_digest, verify_envelope};
    use finstack_ai_runtime::SCAN_PAGE_MAX_RECORDS;
    use finstack_ai_test::store_fixtures::{draft, id, request};

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

    fn limits() -> MemoryStoreLimits {
        MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 8,
            records_per_session: 16,
            snapshot_bytes: 1024,
        }
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
    fn tail_window_reports_gap_for_holes_and_split_for_mid_batch_starts() {
        let store = MemoryJournalStore::try_new(limits()).expect("store");
        let first = block_on(store.append(request(1, 1, 1, vec![draft(1, 1), draft(2, 1)])))
            .expect("append");
        block_on(store.append(request(2, 1, 3, vec![draft(3, 1)]))).expect("append");
        // Mid-batch start (sequence 2 is inside batch 1) is a split.
        assert!(matches!(
            block_on(store.load_from(LoadFromRequest {
                session_id: id::<SessionTag>(1),
                window: LoadWindow::FromSequence {
                    from_sequence: 2,
                    prior_checksum: first.records[0].checksum(),
                },
            })),
            Err(StoreError::Integrity {
                reason_code: "load_from_splits_batch"
            })
        ));
        // Start past the head is a gap (unified from the old split code).
        assert!(matches!(
            block_on(store.load_from(LoadFromRequest {
                session_id: id::<SessionTag>(1),
                window: LoadWindow::FromSequence {
                    from_sequence: 5,
                    prior_checksum: first.records[1].checksum(),
                },
            })),
            Err(StoreError::Integrity {
                reason_code: "load_from_sequence_gap"
            })
        ));
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
