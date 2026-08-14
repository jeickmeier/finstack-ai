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

/// Object-safe append/load/snapshot contract owned by the runtime.
pub trait JournalStore: PortObject {
    /// Atomically append one frozen request.
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>>;

    /// Load one session without any target-ID lookup side channel.
    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>>;

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
}

/// Session-scoped journal load request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadRequest {
    /// Session to load.
    pub session_id: SessionId,
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
