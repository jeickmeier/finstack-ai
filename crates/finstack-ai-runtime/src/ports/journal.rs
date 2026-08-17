//! Journal store port and replay/snapshot transfer types.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AppendRequest, CommittedBatch, Digest, KernelState, Metadata, RecordEnvelope, SessionId,
    Timestamp,
};
use thiserror::Error;

use crate::{PortFuture, PortObject};

/// Maximum envelopes returned by one [`JournalStore::scan`] call.
pub const SCAN_PAGE_MAX_RECORDS: u32 = 256;

/// Shared resource ceilings for in-process and sqlite journal stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreLimits {
    /// Maximum distinct sessions.
    pub sessions: usize,
    /// Maximum committed batches per session.
    pub batches_per_session: usize,
    /// Maximum committed records per session.
    pub records_per_session: usize,
    /// Maximum snapshot bytes per session.
    pub snapshot_bytes: usize,
}

impl StoreLimits {
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
                reason_code: "zero_store_limit",
            });
        }
        Ok(self)
    }
}

/// Object-safe append/load/snapshot contract owned by the runtime.
///
/// Records are append-only and authoritative. Snapshots are a disposable
/// replay cache, not a second semantic model. `health().durable` is `true`
/// only when the leaf actually flushed under its documented pragmas.
pub trait JournalStore: PortObject {
    /// Atomically append one frozen request.
    ///
    /// # Arguments
    ///
    /// * `request` - Idempotent append batch. Duplicate identity must not fork history.
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>>;

    /// Load one session without any target-ID lookup side channel.
    ///
    /// # Arguments
    ///
    /// * `request` - Session identity plus optional snapshot acceleration.
    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>>;

    /// Load a verified suffix of one session journal.
    ///
    /// [`LoadWindow::Full`] matches [`Self::load`]. [`LoadWindow::FromSequence`]
    /// materializes batches at or after `from_sequence` and fail-closes unless
    /// that tail chains from `prior_checksum`. [`LoadWindow::SnapshotPlusTail`]
    /// materializes the disposable snapshot plus batches after it, or falls
    /// back to a full load when no snapshot exists.
    ///
    /// Default implementations call [`Self::load`] and trim. Stores that can
    /// omit the prefix must still verify the returned tail against the prior
    /// checksum or snapshot head. Chain verification is not optional.
    fn load_from(&self, request: LoadFromRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let load = self.load(LoadRequest {
            session_id: request.session_id,
        });
        Box::pin(async move {
            let loaded = load.await?;
            trim_loaded_session(loaded, request.window)
        })
    }

    /// Replace the disposable replay snapshot for one session.
    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>>;

    /// Report current store readiness and durability.
    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>>;

    /// Scan committed envelopes in one session, starting at `from_sequence`.
    ///
    /// `from_sequence == 0` means the first committed record. `limit == 0` is
    /// invalid. Default implementations return `scan_unsupported`.
    fn scan(&self, _request: ScanRequest) -> PortFuture<Result<ScanPage, StoreError>> {
        Box::pin(async {
            Err(StoreError::InvalidRequest {
                reason_code: "scan_unsupported",
            })
        })
    }

    /// Compare-and-swap session metadata against the expected head checksum.
    ///
    /// Metadata never grants authority. Default implementations return
    /// `write_metadata_unsupported`.
    fn write_metadata(
        &self,
        _request: WriteMetadataRequest,
    ) -> PortFuture<Result<MetadataReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::InvalidRequest {
                reason_code: "write_metadata_unsupported",
            })
        })
    }

    /// Encode and replace the disposable kernel-state snapshot for one session.
    ///
    /// Default implementations return `write_snapshot_unsupported`. Protocol-aware
    /// stores encode [`KernelState`] through the shared snapshot envelope.
    fn write_state_snapshot(
        &self,
        _request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::InvalidRequest {
                reason_code: "write_snapshot_unsupported",
            })
        })
    }

    /// Delete snapshot-covered prefix records while retaining the session head.
    ///
    /// Default implementations return `prune_unsupported`. Outstanding tail
    /// records are never deleted. Settlement classification after prune is
    /// rebuilt from the retained snapshot plus tail.
    fn prune(&self, _request: PruneRequest) -> PortFuture<Result<PruneReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::InvalidRequest {
                reason_code: "prune_unsupported",
            })
        })
    }
}

/// Application-configured callback-token / settlement retention window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdempotencyHorizon {
    /// Commands submitted at or after this timestamp are `expired_locator`.
    pub expire_at: Timestamp,
}

/// Session-scoped prune request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneRequest {
    /// Session whose snapshot-covered prefix may be deleted.
    pub session_id: SessionId,
    /// Retention window for post-horizon expiry. Prune itself requires a snapshot.
    pub horizon: IdempotencyHorizon,
}

/// Receipt for an accepted prefix prune.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneReceipt {
    /// Highest deleted sequence, equal to the snapshot sequence.
    pub pruned_through_sequence: u64,
    /// Outstanding effects or interactions retained in snapshot/tail state.
    pub retained_outstanding: u64,
    /// Settlement or rejection identities retained in the snapshot index.
    pub retained_tombstones: u64,
}

/// Session-scoped journal load request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadRequest {
    /// Session to load.
    pub session_id: SessionId,
}

/// How much of a session journal to materialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadWindow {
    /// Every committed batch, including any disposable snapshot.
    Full,
    /// Batches whose first sequence is at least `from_sequence`.
    ///
    /// `from_sequence == 0` means the first committed record. When
    /// `from_sequence > 1`, `prior_checksum` is the checksum of sequence
    /// `from_sequence - 1` and the store fail-closes if the tail does not
    /// chain from it. `from_sequence` must land on a batch boundary.
    FromSequence {
        /// Inclusive start sequence; `0` means the first committed record.
        from_sequence: u64,
        /// Checksum of the record immediately before `from_sequence`.
        prior_checksum: Digest,
    },
    /// Disposable snapshot plus batches after the snapshot sequence.
    ///
    /// Falls back to [`Self::Full`] when no snapshot exists. The tail must
    /// chain from the snapshot head checksum.
    SnapshotPlusTail,
}

/// Session-scoped journal load that may omit a verified prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadFromRequest {
    /// Session to load.
    pub session_id: SessionId,
    /// Prefix window to materialize.
    pub window: LoadWindow,
}

/// Loaded journal preserving the store's committed batch boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSession {
    /// Loaded session identity.
    pub session_id: SessionId,
    /// Current committed head; zero denotes an empty journal.
    pub head_sequence: u64,
    /// Last committed envelope checksum; `None` for an empty journal.
    pub head_checksum: Option<Digest>,
    /// Session metadata. Never grants authority.
    pub metadata: Metadata,
    /// Ordered atomic commit boundaries.
    pub committed_batches: Arc<[CommittedBatch]>,
    /// Optional bounded opaque replay cache.
    pub snapshot: Option<OpaqueSnapshot>,
    /// Best-effort decoded snapshot used only as a disposable replay cache.
    pub accelerated: Option<AcceleratedRestore>,
}

impl LoadedSession {
    /// Empty session with no records, checksum, or metadata.
    #[must_use]
    pub fn empty(session_id: SessionId) -> Self {
        Self {
            session_id,
            head_sequence: 0,
            head_checksum: None,
            metadata: Metadata::empty(),
            committed_batches: Arc::from([]),
            snapshot: None,
            accelerated: None,
        }
    }

    /// First sequence covered by [`Self::committed_batches`], or `0` when empty.
    #[must_use]
    pub fn first_loaded_sequence(&self) -> u64 {
        self.committed_batches
            .first()
            .map_or(0, |batch| batch.first_sequence)
    }

    /// Whether the authoritative prefix before the first loaded batch was omitted.
    #[must_use]
    pub fn omits_prefix(&self) -> bool {
        self.first_loaded_sequence() > 1
            || (self.committed_batches.is_empty() && self.head_sequence > 0)
    }
}

fn trim_loaded_session(
    loaded: LoadedSession,
    window: LoadWindow,
) -> Result<LoadedSession, StoreError> {
    match window {
        LoadWindow::Full => Ok(loaded),
        LoadWindow::FromSequence {
            from_sequence,
            prior_checksum,
        } => trim_from_sequence(loaded, from_sequence, prior_checksum),
        LoadWindow::SnapshotPlusTail => trim_snapshot_plus_tail(loaded),
    }
}

fn trim_from_sequence(
    loaded: LoadedSession,
    from_sequence: u64,
    prior_checksum: Digest,
) -> Result<LoadedSession, StoreError> {
    let start = if from_sequence == 0 { 1 } else { from_sequence };
    if start <= 1 {
        return Ok(loaded);
    }
    if start > loaded.head_sequence.saturating_add(1) {
        return Err(StoreError::Integrity {
            reason_code: "load_from_sequence_gap",
        });
    }
    if let Some(prior) = record_at(&loaded, start.saturating_sub(1))
        && prior.checksum() != prior_checksum
    {
        return Err(StoreError::Integrity {
            reason_code: "load_from_prior_checksum_mismatch",
        });
    }
    if start == loaded.head_sequence.saturating_add(1) {
        if loaded.head_checksum != Some(prior_checksum) {
            return Err(StoreError::Integrity {
                reason_code: "load_from_prior_checksum_mismatch",
            });
        }
        return Ok(LoadedSession {
            committed_batches: Arc::from([]),
            ..loaded
        });
    }
    let start_index = loaded
        .committed_batches
        .iter()
        .position(|batch| batch.first_sequence >= start)
        .ok_or(StoreError::Integrity {
            reason_code: "load_from_sequence_gap",
        })?;
    if loaded.committed_batches[start_index].first_sequence != start {
        return Err(StoreError::Integrity {
            reason_code: "load_from_splits_batch",
        });
    }
    let first = loaded.committed_batches[start_index]
        .records
        .first()
        .ok_or(StoreError::Integrity {
            reason_code: "load_from_empty_batch",
        })?;
    if first.sequence() != start || first.previous_checksum() != Some(prior_checksum) {
        return Err(StoreError::Integrity {
            reason_code: "load_from_prior_checksum_mismatch",
        });
    }
    Ok(LoadedSession {
        committed_batches: Arc::from(loaded.committed_batches[start_index..].to_vec()),
        ..loaded
    })
}

fn trim_snapshot_plus_tail(loaded: LoadedSession) -> Result<LoadedSession, StoreError> {
    let Some(accelerated) = loaded.accelerated.as_ref() else {
        return Ok(loaded);
    };
    let start = accelerated.sequence.saturating_add(1);
    if start > loaded.head_sequence.saturating_add(1) {
        return Err(StoreError::Integrity {
            reason_code: "snapshot_sequence_invalid",
        });
    }
    if loaded.head_sequence == accelerated.sequence {
        if loaded.head_checksum != Some(accelerated.head_checksum) {
            return Err(StoreError::Integrity {
                reason_code: "snapshot_checksum_mismatch",
            });
        }
        return Ok(LoadedSession {
            committed_batches: Arc::from([]),
            ..loaded
        });
    }
    let start_index = loaded
        .committed_batches
        .iter()
        .position(|batch| batch.first_sequence >= start)
        .ok_or(StoreError::Integrity {
            reason_code: "snapshot_missing_record",
        })?;
    if loaded.committed_batches[start_index].first_sequence != start {
        return Err(StoreError::Integrity {
            reason_code: "snapshot_splits_batch",
        });
    }
    let first = loaded.committed_batches[start_index]
        .records
        .first()
        .ok_or(StoreError::Integrity {
            reason_code: "snapshot_missing_record",
        })?;
    if first.previous_checksum() != Some(accelerated.head_checksum) {
        return Err(StoreError::Integrity {
            reason_code: "snapshot_checksum_mismatch",
        });
    }
    Ok(LoadedSession {
        committed_batches: Arc::from(loaded.committed_batches[start_index..].to_vec()),
        ..loaded
    })
}

fn record_at(loaded: &LoadedSession, sequence: u64) -> Option<&RecordEnvelope> {
    loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .find(|record| record.sequence() == sequence)
}

/// Session-local scan request. There is no global target-ID scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanRequest {
    /// Session to scan.
    pub session_id: SessionId,
    /// Inclusive start sequence; `0` means the first committed record.
    pub from_sequence: u64,
    /// Maximum envelopes to return. `0` is invalid.
    pub limit: u32,
}

/// Page of committed envelopes from [`JournalStore::scan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPage {
    /// Scanned session.
    pub session_id: SessionId,
    /// Envelopes in sequence order.
    pub records: Arc<[RecordEnvelope]>,
    /// Next sequence to request, or `None` at the end of the session.
    pub next_sequence: Option<u64>,
}

/// Compare-and-swap metadata write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteMetadataRequest {
    /// Session whose metadata is replaced.
    pub session_id: SessionId,
    /// Expected current head checksum.
    pub expected_head_checksum: Option<Digest>,
    /// Replacement metadata. Never grants authority.
    pub metadata: Metadata,
}

/// Receipt for an accepted metadata compare-and-swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataReceipt {
    /// Session whose metadata was written.
    pub session_id: SessionId,
    /// Head checksum the write was conditioned on.
    pub head_checksum: Option<Digest>,
    /// Stored metadata.
    pub metadata: Metadata,
}

/// Bounded opaque snapshot returned by a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueSnapshot {
    sequence: u64,
    digest: Digest,
    bytes: Arc<[u8]>,
}

impl OpaqueSnapshot {
    /// Construct an opaque snapshot under a caller-selected byte ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::LimitExceeded`] when `bytes` exceeds `max_bytes`.
    pub fn try_new(
        sequence: u64,
        digest: Digest,
        bytes: impl Into<Arc<[u8]>>,
        max_bytes: usize,
    ) -> Result<Self, StoreError> {
        let bytes = bytes.into();
        if bytes.len() > max_bytes {
            return Err(StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: max_bytes,
            });
        }
        Ok(Self {
            sequence,
            digest,
            bytes,
        })
    }

    /// Journal sequence covered by this snapshot.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Snapshot-state digest supplied by the snapshot producer.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Opaque snapshot bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Validated kernel-state restore handle decoded from a snapshot cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceleratedRestore {
    /// Journal sequence covered by the snapshot.
    pub sequence: u64,
    /// Envelope checksum of the record at [`Self::sequence`].
    pub head_checksum: Digest,
    /// Semantic timestamp of a pending `RetryScheduled` record, when present.
    pub pending_timer_scheduled_at: Option<Timestamp>,
    /// Hydrated kernel state. This is not a second snapshot DTO.
    pub state: KernelState,
}

/// Best-effort snapshot write policy used by [`crate::CommitCoordinator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotSchedule {
    /// Minimum records after the last snapshot before another write is attempted.
    pub every_n_records: u64,
    /// Maximum time a snapshot write may block an active submit.
    pub write_timeout: Duration,
}

impl Default for SnapshotSchedule {
    fn default() -> Self {
        Self {
            every_n_records: 32,
            write_timeout: Duration::from_millis(50),
        }
    }
}

/// Request to encode and persist one session's disposable kernel-state snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSnapshotRequest {
    /// Session owning the snapshot.
    pub session_id: SessionId,
    /// Current replay-derived kernel state.
    pub state: KernelState,
    /// Envelope checksum of the record at `state.last_applied_sequence`.
    pub head_checksum: Digest,
    /// Semantic timestamp of a pending `RetryScheduled` record, when present.
    pub pending_timer_scheduled_at: Option<Timestamp>,
}

/// Request to replace one session's disposable snapshot cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRequest {
    /// Session owning the snapshot.
    pub session_id: SessionId,
    /// Bounded opaque snapshot.
    pub snapshot: OpaqueSnapshot,
}

/// Receipt for an accepted snapshot cache write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReceipt {
    /// Session owning the snapshot.
    pub session_id: SessionId,
    /// Sequence covered by the snapshot.
    pub sequence: u64,
    /// Snapshot-state digest supplied by the producer.
    pub digest: Digest,
    /// Stored byte count.
    pub bytes: usize,
}

/// Store readiness report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreHealth {
    /// Whether the store is ready to accept requests.
    pub ready: bool,
    /// Whether acknowledged appends satisfy durable-storage guarantees.
    pub durable: bool,
    /// Stable non-secret operating-mode description.
    pub detail: Arc<str>,
}

/// Stable journal-store failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StoreError {
    /// Optimistic sequence precondition failed without a write.
    #[error(
        "store conflict: expected sequence {expected_sequence}, actual next sequence {actual_next_sequence}"
    )]
    Conflict {
        /// Request precondition.
        expected_sequence: u64,
        /// Store's current next sequence.
        actual_next_sequence: u64,
    },
    /// A durable identity was reused with unequal content.
    #[error("store corruption: {reason_code}")]
    Corruption {
        /// Stable reason code.
        reason_code: &'static str,
    },
    /// A configured store resource ceiling was exceeded.
    #[error("store limit exceeded for {resource}: {limit}")]
    LimitExceeded {
        /// Stable resource name.
        resource: &'static str,
        /// Configured ceiling.
        limit: usize,
    },
    /// Request is structurally invalid for the store contract.
    #[error("invalid store request: {reason_code}")]
    InvalidRequest {
        /// Stable reason code.
        reason_code: &'static str,
    },
    /// Store could not determine whether an append committed.
    #[error("ambiguous append acknowledgement")]
    AmbiguousAcknowledgement,
    /// Store is temporarily unavailable.
    #[error("store unavailable: {reason_code}")]
    Unavailable {
        /// Stable non-secret reason code.
        reason_code: &'static str,
    },
    /// Stored data cannot be decoded or replayed safely.
    #[error("store integrity failure: {reason_code}")]
    Integrity {
        /// Stable reason code.
        reason_code: &'static str,
    },
}

impl StoreError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Conflict { .. } => "store_conflict",
            Self::Corruption { .. } => "store_corruption",
            Self::LimitExceeded { .. } => "store_limit_exceeded",
            Self::InvalidRequest { .. } => "invalid_store_request",
            Self::AmbiguousAcknowledgement => "ambiguous_acknowledgement",
            Self::Unavailable { .. } => "store_unavailable",
            Self::Integrity { .. } => "store_integrity_failure",
        }
    }
}

/// Diagnostic store commit timestamp used only by implementations that have one.
pub type StoreCommitTimestamp = Timestamp;
