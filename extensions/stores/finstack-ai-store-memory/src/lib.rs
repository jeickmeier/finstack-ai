//! Bounded, explicitly non-durable in-memory [`JournalStore`] implementation.
//!
//! This leaf uses transitional in-process JSON envelope bytes. It deliberately
//! does not claim the canonical-CBOR or checksum guarantees owned by PR-039.

#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use finstack_ai_kernel::{
    AppendBatchId, AppendRequest, CommittedBatch, DOMAIN_RECORD_PAYLOAD, Digest,
    RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION, RecordDraft, RecordEnvelope, RecordId, SessionId,
};
use finstack_ai_runtime::{
    JournalStore, LoadRequest, LoadedSession, OpaqueSnapshot, PortFuture, SnapshotReceipt,
    SnapshotRequest, StoreError, StoreHealth,
};

const TRANSITIONAL_CHECKSUM_DOMAIN: &str = "record-envelope-transitional";
const TRANSITIONAL_CHECKSUM_VERSION: u32 = 1;

/// Required resource ceilings for [`MemoryJournalStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStoreLimits {
    /// Maximum distinct sessions.
    pub sessions: usize,
    /// Maximum committed batches per session.
    pub batches_per_session: usize,
    /// Maximum committed records per session.
    pub records_per_session: usize,
    /// Maximum snapshot bytes per session.
    pub snapshot_bytes: usize,
}

impl MemoryStoreLimits {
    /// Validate an explicit, usable set of limits.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] when any limit is zero.
    pub fn validate(self) -> Result<Self, StoreError> {
        if self.sessions == 0
            || self.batches_per_session == 0
            || self.records_per_session == 0
            || self.snapshot_bytes == 0
        {
            return Err(StoreError::InvalidRequest {
                reason_code: "zero_memory_store_limit",
            });
        }
        Ok(self)
    }
}

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

        let committed = build_committed_batch(&request, current_head)?;
        let last_sequence = committed.last_sequence;
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
        session.records += request.records().len();
        session.batches.push(committed.clone());
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
            return Ok(LoadedSession {
                session_id: request.session_id,
                head_sequence: 0,
                committed_batches: Arc::from([]),
                snapshot: None,
            });
        };
        Ok(LoadedSession {
            session_id: request.session_id,
            head_sequence: session.head_sequence,
            committed_batches: session.batches.clone().into(),
            snapshot: session.snapshot.clone(),
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
    records: usize,
    batches: Vec<CommittedBatch>,
    snapshot: Option<OpaqueSnapshot>,
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

fn build_committed_batch(
    request: &AppendRequest,
    previous_sequence: u64,
) -> Result<CommittedBatch, StoreError> {
    let mut previous_checksum = None;
    let mut records = Vec::with_capacity(request.records().len());
    for (offset, draft) in request.records().iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| StoreError::Integrity {
            reason_code: "record_offset_overflow",
        })?;
        let sequence =
            request
                .expected_sequence()
                .checked_add(offset)
                .ok_or(StoreError::Integrity {
                    reason_code: "sequence_exhausted",
                })?;
        let payload_bytes =
            serde_json::to_vec(draft.body()).map_err(|_| StoreError::Integrity {
                reason_code: "transitional_payload_encode_failed",
            })?;
        let payload_digest = Digest::domain_separated(
            DOMAIN_RECORD_PAYLOAD,
            RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION,
            &payload_bytes,
        )
        .map_err(|_| StoreError::Integrity {
            reason_code: "record_payload_digest_failed",
        })?;
        let checksum_bytes = serde_json::to_vec(&(
            request.batch_id(),
            sequence,
            draft,
            previous_checksum,
            previous_sequence,
        ))
        .map_err(|_| StoreError::Integrity {
            reason_code: "transitional_envelope_encode_failed",
        })?;
        let checksum = Digest::domain_separated(
            TRANSITIONAL_CHECKSUM_DOMAIN,
            TRANSITIONAL_CHECKSUM_VERSION,
            &checksum_bytes,
        )
        .map_err(|_| StoreError::Integrity {
            reason_code: "transitional_checksum_failed",
        })?;
        let envelope = RecordEnvelope::try_new(
            draft.format_version(),
            draft.kind_version(),
            draft.record_id(),
            draft.session_id(),
            draft.lane_id(),
            draft.run_id(),
            sequence,
            draft.timestamp(),
            None,
            payload_digest,
            previous_checksum,
            checksum,
            draft.derived_event_ids().to_vec(),
            draft.body().clone(),
        )
        .map_err(|_| StoreError::Integrity {
            reason_code: "committed_envelope_invalid",
        })?;
        previous_checksum = Some(checksum);
        records.push(envelope);
    }
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

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Barrier};
    use std::task::{Context, Poll, Waker};
    use std::thread;

    use finstack_ai_kernel::{
        AuthorizationEvidence, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
        Id, IdTag, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
        RecordDraft, RecordTag, RunTag, SessionTag, Timestamp,
    };

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

        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 3);
        assert_eq!(loaded.committed_batches.as_ref(), &[first, second]);
        assert!(loaded.snapshot.is_none());

        let health = block_on(store.health()).expect("health");
        assert!(health.ready);
        assert!(!health.durable);
        assert_eq!(health.detail.as_ref(), "memory_non_durable");
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
