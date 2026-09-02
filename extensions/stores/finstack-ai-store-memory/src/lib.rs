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
    AppendBatchId, AppendRequest, CommittedBatch, Digest, Metadata, RecordEnvelope, RecordId,
    SessionId,
};
use finstack_ai_protocol::ChainAnchor;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::{
    JournalStore, JournalStoreDescriptor, LoadFromRequest, LoadRequest, LoadWindow, LoadedSession,
    MetadataReceipt, OpaqueSnapshot, PruneReceipt, PruneRequest, ScanPage, ScanRequest,
    SnapshotReceipt, SnapshotRequest, StateSnapshotRequest, StoreError, StoreHealth, StoreLimits,
    WriteMetadataRequest,
};
use finstack_ai_store_common::{
    FROM_SEQUENCE_WINDOW, SNAPSHOT_WINDOW, SessionUsage, VerifiedHead, VerifiedHeadCache,
    VerifiedRead, WindowCodes, accelerated_from, admit_append_limits, admit_prune_snapshot,
    admit_snapshot_sequence, build_committed_batch, check_append_sequence, check_snapshot_size,
    classify_record_reuse, encode_state_request, outstanding_count, scan_next_sequence, scan_start,
    select_tail_batches, tombstone_count, validate_scan_limit, verify_full_head,
    verify_head_against_cache, verify_tail_records,
};

/// Required resource ceilings for [`MemoryJournalStore`].
pub type MemoryStoreLimits = StoreLimits;

/// Mutex-protected ordered-map journal intended for tests and embedded ephemeral use.
pub struct MemoryJournalStore {
    limits: MemoryStoreLimits,
    inner: Mutex<Inner>,
    /// Process-local chain-verification cache (spec D9), keyed by session.
    ///
    /// A separate lock from `inner`. Nothing ever holds the cache lock while
    /// acquiring `inner`: reads that precede a load release it first, and
    /// every nested use takes `inner` first and releases the cache lock
    /// inside that critical section. Cache updates that must not be raced by
    /// a concurrent writer (`append_sync`, `prune_sync`) are made while
    /// `inner` is still held.
    verified: VerifiedHeadCache,
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
            verified: VerifiedHeadCache::new(),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Unavailable {
            reason_code: "memory_store_lock_poisoned",
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "append validation and the atomic state transition are kept together"
    )]
    fn append_sync(&self, request: AppendRequest) -> Result<CommittedBatch, StoreError> {
        let mut inner = self.lock()?;

        if let Some(existing) = inner.batches_by_id.get(&request.batch_id()) {
            if existing.history_pruned {
                return if existing.request == request {
                    Err(StoreError::InvalidRequest {
                        reason_code: "append_history_pruned",
                    })
                } else {
                    Err(StoreError::Corruption {
                        reason_code: "append_batch_id_reuse",
                    })
                };
            }
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
            .copied()
            .collect::<Vec<_>>();
        if hits.iter().any(|batch_id| {
            inner
                .batches_by_id
                .get(batch_id)
                .is_some_and(|entry| entry.history_pruned)
        }) {
            return Err(StoreError::InvalidRequest {
                reason_code: "append_history_pruned",
            });
        }
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

        let session = inner.sessions.entry(session_id).or_default();
        session.head_sequence = last_sequence;
        session.head_checksum = head_checksum;
        session.records += request.records().len();
        session.batches.push(committed.clone());
        let head = VerifiedHead {
            sequence: session.head_sequence,
            checksum: session.head_checksum,
        };
        for record in request.records() {
            inner.records_by_id.insert(record.record_id(), batch_id);
        }
        inner.batches_by_id.insert(
            batch_id,
            BatchIndexEntry {
                request,
                committed: committed.clone(),
                history_pruned: false,
            },
        );
        // The chain this append extended was built by this process from the
        // session's own head, so its head is verified by construction. This
        // stays inside the `inner` critical section: releasing the lock first
        // would let a concurrent append's newer head be overwritten by this
        // older-but-valid one (the generation is unbumped, so the guard does
        // not reject it) and cost the next load a full re-verification.
        let read = self.verified.read(session_id);
        self.verified.remember(session_id, read, head);
        Ok(committed)
    }

    /// Verify one session's chain, using (and maintaining) the shared
    /// verified-head cache according to `cache_use`.
    ///
    /// The cache never changes the outcome: [`verify_head_against_cache`]
    /// falls back to a full verification whenever the cached anchor is not
    /// usable, so the reason codes are those of an uncached verification.
    fn verify_session_head(
        &self,
        session_id: SessionId,
        session: &SessionData,
        read: VerifiedRead,
        cache_use: CacheUse,
    ) -> Result<Option<Digest>, StoreError> {
        let records = flatten_records(session);
        let cached = match cache_use {
            CacheUse::Proving => read.head,
            CacheUse::Windowed => None,
        };
        match verify_head_against_cache(
            ChainAnchor {
                session_id,
                next_sequence: session.anchor_next_sequence,
                previous_checksum: session.anchor_previous_checksum,
            },
            &records,
            session.head_sequence,
            session.head_checksum,
            cached,
        ) {
            Ok(head) => {
                if cache_use == CacheUse::Proving {
                    self.verified.remember(
                        session_id,
                        read,
                        VerifiedHead {
                            sequence: session.head_sequence,
                            checksum: head,
                        },
                    );
                }
                Ok(head)
            }
            Err(error) => {
                // Spec D9(b): an integrity failure invalidates whatever this
                // process believed it had verified for this session.
                if matches!(error, StoreError::Integrity { .. }) {
                    self.verified.invalidate(session_id);
                }
                Err(error)
            }
        }
    }

    /// Load and verify a whole session journal.
    ///
    /// `cache_use` is [`CacheUse::Windowed`] when a windowed request
    /// degenerated to this path, which is why it takes the session id rather
    /// than a [`LoadRequest`].
    fn load_full(
        &self,
        session_id: SessionId,
        cache_use: CacheUse,
    ) -> Result<LoadedSession, StoreError> {
        let read = self.verified.read(session_id);
        let inner = self.lock()?;
        let Some(session) = inner.sessions.get(&session_id) else {
            return Ok(LoadedSession::empty(session_id));
        };
        self.verify_session_head(session_id, session, read, cache_use)?;
        Ok(LoadedSession {
            session_id,
            head_sequence: session.head_sequence,
            head_checksum: session.head_checksum,
            metadata: session.metadata.clone(),
            committed_batches: session.batches.clone().into(),
            accelerated: session.snapshot.as_ref().and_then(accelerated_from),
            snapshot: session.snapshot.clone(),
        })
    }

    fn load_from_sync(&self, request: LoadFromRequest) -> Result<LoadedSession, StoreError> {
        let result = match request.window {
            LoadWindow::Full => self.load_full(request.session_id, CacheUse::Proving),
            LoadWindow::FromSequence {
                from_sequence,
                prior_checksum,
            } => self.load_from_sequence(request.session_id, from_sequence, prior_checksum),
            LoadWindow::SnapshotPlusTail => self.load_snapshot_plus_tail(request.session_id),
        };
        // Spec D9(b): every integrity failure invalidates this process's
        // proof, whichever window produced it. `load_full` already does this
        // for the failures it owns; the windowed paths below report gaps,
        // splits and undecodable snapshots without going through it, so the
        // rule is applied once here for all three windows — matching sqlite
        // and postgres, which invalidate on any `Integrity` from either
        // entry point.
        if matches!(result, Err(StoreError::Integrity { .. })) {
            self.verified.invalidate(request.session_id);
        }
        result
    }

    fn load_from_sequence(
        &self,
        session_id: SessionId,
        from_sequence: u64,
        prior_checksum: Digest,
    ) -> Result<LoadedSession, StoreError> {
        if from_sequence <= 1 {
            // The window covers the whole journal, so this is a plain full
            // load — but it arrived as a windowed request, and windowed
            // requests never touch the cache (sqlite and postgres pass no
            // cached head down this same path and never cache its outcome).
            return self.load_full(session_id, CacheUse::Windowed);
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
            from_sequence,
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
            // No snapshot to accelerate from: same windowed-request rule as
            // `load_from_sequence` above.
            return self.load_full(session_id, CacheUse::Windowed);
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
        let read = self.verified.read(request.session_id);
        let inner = self.lock()?;
        let Some(session) = inner.sessions.get(&request.session_id) else {
            return Ok(ScanPage {
                session_id: request.session_id,
                records: Arc::from([]),
                next_sequence: None,
            });
        };
        self.verify_session_head(request.session_id, session, read, CacheUse::Proving)?;
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

    #[allow(
        clippy::too_many_lines,
        reason = "prune validation and the atomic state transition are kept together"
    )]
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
        let boundary_batch = session
            .batches
            .iter()
            .find(|batch| batch.last_sequence == pruned_through_sequence)
            .cloned()
            .ok_or(StoreError::Integrity {
                reason_code: "prune_boundary_batch_missing",
            })?;
        let boundary_record =
            boundary_batch
                .records
                .last()
                .cloned()
                .ok_or(StoreError::Integrity {
                    reason_code: "prune_boundary_record_missing",
                })?;
        let boundary_record_id = boundary_record.record_id();
        let boundary_batch_id = boundary_batch.batch_id;
        let boundary_record_ids = boundary_batch
            .records
            .iter()
            .map(RecordEnvelope::record_id)
            .collect::<Vec<_>>();
        session
            .batches
            .retain(|batch| batch.last_sequence > pruned_through_sequence);
        session.boundary_record = Some(boundary_record.clone());
        session.records = 1 + session
            .batches
            .iter()
            .map(|batch| batch.records.len())
            .sum::<usize>();
        session.anchor_next_sequence = boundary_record.sequence();
        session.anchor_previous_checksum = boundary_record.previous_checksum();
        let head = VerifiedHead {
            sequence: session.head_sequence,
            checksum: session.head_checksum,
        };
        let records = flatten_records(session);
        let stored_head = session.head_checksum;
        let anchor = ChainAnchor {
            session_id: request.session_id,
            next_sequence: session.anchor_next_sequence,
            previous_checksum: session.anchor_previous_checksum,
        };

        inner.batches_by_id.retain(|_, entry| {
            entry.request.session_id() != request.session_id
                || entry.committed.last_sequence >= pruned_through_sequence
        });
        if let Some(entry) = inner.batches_by_id.get_mut(&boundary_batch_id) {
            entry.history_pruned = boundary_batch.first_sequence != boundary_batch.last_sequence;
        }
        for record_id in boundary_record_ids {
            if record_id != boundary_record_id {
                inner.records_by_id.remove(&record_id);
            }
        }
        let retained_batch_ids = inner.batches_by_id.keys().copied().collect::<Vec<_>>();
        inner
            .records_by_id
            .retain(|_, batch_id| retained_batch_ids.contains(batch_id));
        // The retained journal is a different chain prefix than the one the
        // cached proof described, so the proof is dropped and the pruned
        // journal re-verified in full before a new one is recorded.
        self.verified.invalidate(request.session_id);
        // `inner` is one mutex over *every* session, so nothing expensive may
        // run under it. `records` is already an owned clone and `stored_head`
        // a `Copy` digest, so the chain walk needs no lock at all: release it
        // first, and every unrelated append/load/scan keeps running while this
        // prune re-verifies.
        drop(inner);
        verify_full_head(anchor, &records, stored_head)?;
        // The cache write does go back under `inner`, briefly, for
        // `append_sync`'s reason: a concurrent writer's newer head must not be
        // overwritten by this older-but-valid one. The generation guard alone
        // cannot prevent that — an intervening append bumps nothing, so its
        // proof would be silently replaced by the pre-append head verified
        // above. Re-checking the head under the lock is what rules it out: if
        // the session moved on, that writer has already recorded its own
        // (newer, equally valid) proof and this one is simply dropped.
        {
            let inner = self.lock()?;
            let head_unchanged = inner
                .sessions
                .get(&request.session_id)
                .is_some_and(|session| {
                    session.head_sequence == head.sequence && session.head_checksum == head.checksum
                });
            if head_unchanged {
                let read = self.verified.read(request.session_id);
                self.verified.remember(request.session_id, read, head);
            }
        }
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
    fn descriptor(&self) -> JournalStoreDescriptor {
        JournalStoreDescriptor {
            store_id: Arc::from("finstack.store.memory"),
            metadata: Metadata::empty(),
        }
    }

    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        Box::pin(core::future::ready(self.append_sync(request)))
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(core::future::ready(
            self.load_full(request.session_id, CacheUse::Proving),
        ))
    }

    fn load_from(&self, request: LoadFromRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(core::future::ready(self.load_from_sync(request)))
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(core::future::ready(self.write_snapshot_sync(request)))
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
        Box::pin(core::future::ready(self.scan_sync(request)))
    }

    fn write_metadata(
        &self,
        request: WriteMetadataRequest,
    ) -> PortFuture<Result<MetadataReceipt, StoreError>> {
        Box::pin(core::future::ready(self.write_metadata_sync(request)))
    }

    fn write_state_snapshot(
        &self,
        request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(core::future::ready(
            self.write_state_snapshot_sync(&request),
        ))
    }

    fn prune(&self, request: PruneRequest) -> PortFuture<Result<PruneReceipt, StoreError>> {
        Box::pin(core::future::ready(self.prune_sync(request)))
    }
}

/// Whether a full-journal verification may use the verified-head cache.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CacheUse {
    /// A `load`/`scan`/`LoadWindow::Full` request: consult the cached proof,
    /// and record the freshly verified head on success.
    Proving,
    /// A windowed request that degenerated to a full journal load: neither
    /// read nor write the cache. Only a request for the whole journal proves
    /// the whole chain, so a windowed request must not seed a proof later
    /// loads would anchor on.
    Windowed,
}

#[derive(Default)]
struct Inner {
    sessions: BTreeMap<SessionId, SessionData>,
    batches_by_id: BTreeMap<AppendBatchId, BatchIndexEntry>,
    /// Committing batch of every stored record id.
    records_by_id: BTreeMap<RecordId, AppendBatchId>,
}

struct SessionData {
    head_sequence: u64,
    head_checksum: Option<Digest>,
    metadata: Metadata,
    records: usize,
    batches: Vec<CommittedBatch>,
    boundary_record: Option<RecordEnvelope>,
    snapshot: Option<OpaqueSnapshot>,
    anchor_next_sequence: u64,
    anchor_previous_checksum: Option<Digest>,
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            head_sequence: 0,
            head_checksum: None,
            metadata: Metadata::empty(),
            records: 0,
            batches: Vec::new(),
            boundary_record: None,
            snapshot: None,
            anchor_next_sequence: 1,
            anchor_previous_checksum: None,
        }
    }
}

struct BatchIndexEntry {
    request: AppendRequest,
    committed: CommittedBatch,
    history_pruned: bool,
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
        session_id,
        &records,
        start,
        prior_checksum,
        session.head_sequence,
        session.head_checksum,
        codes,
    )?;
    Ok(LoadedSession {
        session_id,
        head_sequence: session.head_sequence,
        head_checksum: session.head_checksum,
        metadata: session.metadata.clone(),
        committed_batches: tail.to_vec().into(),
        accelerated: session.snapshot.as_ref().and_then(accelerated_from),
        snapshot: session.snapshot.clone(),
    })
}

fn flatten_records(session: &SessionData) -> Vec<RecordEnvelope> {
    session
        .boundary_record
        .iter()
        .cloned()
        .chain(
            session
                .batches
                .iter()
                .flat_map(|batch| batch.records.iter().cloned()),
        )
        .collect()
}

fn slice_records(session: &SessionData, start: u64, limit: usize) -> (Vec<RecordEnvelope>, bool) {
    let mut records = Vec::new();
    let mut saw_more = false;
    if let Some(boundary) = &session.boundary_record
        && boundary.sequence() >= start
    {
        if limit == 0 {
            return (records, true);
        }
        records.push(boundary.clone());
    }
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
    use finstack_ai_runtime::ports::journal::SCAN_PAGE_MAX_RECORDS;
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
