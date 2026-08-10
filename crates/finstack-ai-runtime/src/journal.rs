//! Journal store port and replay/snapshot transfer types.

use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch, Digest, SessionId, Timestamp};
use thiserror::Error;

use crate::{PortFuture, PortObject};

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
    /// Ordered atomic commit boundaries.
    pub committed_batches: Arc<[CommittedBatch]>,
    /// Optional bounded opaque replay cache.
    pub snapshot: Option<OpaqueSnapshot>,
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
