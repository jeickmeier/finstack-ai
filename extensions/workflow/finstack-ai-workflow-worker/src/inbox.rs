//! Durable response inbox: buffers interaction resolutions and external
//! effect completions until the worker consumes them.
//!
//! Each row is keyed by `(tenant_scope, session_id, pending_id)`. Equal
//! redelivery is idempotent, while a different command under the same key is
//! rejected before it can replace the first response. The stored `payload`
//! is opaque bytes: the `serde_json`
//! serialization of an `InteractionResolutionCommand` or
//! `ExternalEffectCompletionCommand`
//! (`crates/finstack-ai-kernel/src/records/run/external.rs:109-218`). This
//! module never deserializes it — that happens at consume time in the
//! worker.

use std::sync::Arc;

use finstack_ai_kernel::{Digest, SessionId, Timestamp};

use crate::error::WorkerError;

/// Maximum encoded interaction or completion command accepted by the worker.
pub const MAX_INBOX_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

/// Kind of response buffered in the inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxKind {
    /// Resolution of a human-in-the-loop interaction.
    Interaction,
    /// Completion of an external effect.
    External,
}

impl InboxKind {
    /// Stable lowercase representation used in storage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interaction => "interaction",
            Self::External => "external",
        }
    }

    /// Parse a stored kind, failing closed on anything unrecognized.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreIntegrity`] with code `"inbox_kind"` for
    /// any value other than `"interaction"` or `"external"`.
    pub fn parse(value: &str) -> Result<Self, WorkerError> {
        match value {
            "interaction" => Ok(Self::Interaction),
            "external" => Ok(Self::External),
            _ => Err(WorkerError::StoreIntegrity { code: "inbox_kind" }),
        }
    }
}

/// One buffered response awaiting consumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxRow {
    /// Tenant that owns the session.
    pub tenant_scope: Arc<str>,
    /// Session the response belongs to.
    pub session_id: SessionId,
    /// Identifier of the pending interaction or effect being resolved.
    pub pending_id: Arc<str>,
    /// Which command kind the payload deserializes to.
    pub kind: InboxKind,
    /// Opaque `serde_json` payload; deserialized at consume time.
    pub payload: Arc<[u8]>,
    /// Digest of the exact payload bytes used for conflict detection.
    pub payload_digest: Digest,
    /// When the response was received.
    pub received_at: Timestamp,
}

impl InboxRow {
    /// Build a bounded inbox row and derive its exact payload digest.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::InvalidConfiguration`] with code
    /// `"inbox_payload_too_large"` when `payload` exceeds
    /// [`MAX_INBOX_PAYLOAD_BYTES`].
    pub fn try_new(
        tenant_scope: Arc<str>,
        session_id: SessionId,
        pending_id: Arc<str>,
        kind: InboxKind,
        payload: Arc<[u8]>,
        received_at: Timestamp,
    ) -> Result<Self, WorkerError> {
        if payload.len() > MAX_INBOX_PAYLOAD_BYTES {
            return Err(WorkerError::InvalidConfiguration {
                code: "inbox_payload_too_large",
            });
        }
        Ok(Self {
            tenant_scope,
            session_id,
            pending_id,
            kind,
            payload_digest: Digest::blob_content(payload.as_ref()),
            payload,
            received_at,
        })
    }

    /// Whether this row carries a valid digest for its exact payload bytes.
    #[must_use]
    pub fn digest_is_valid(&self) -> bool {
        self.payload_digest == Digest::blob_content(self.payload.as_ref())
    }
}

/// Result of inserting a response under its durable inbox key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxInsertOutcome {
    /// The key was absent and the row was inserted.
    Inserted,
    /// The same kind and payload digest already occupied the key.
    Idempotent,
}

/// One permanently rejected inbox command retained for operator inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetterRow {
    /// Original buffered response.
    pub response: InboxRow,
    /// Stable, non-secret rejection reason.
    pub reason_code: Arc<str>,
    /// When the response was moved out of the active inbox.
    pub rejected_at: Timestamp,
}

/// Adapter table buffering durable responses for the workflow worker.
pub trait InboxStore: Send + Sync {
    /// Insert a response, accept an equal replay, and reject a conflicting
    /// command under the same `(tenant_scope, session_id, pending_id)` key.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn insert(&self, row: &InboxRow) -> Result<InboxInsertOutcome, WorkerError>;

    /// Load one buffered response by its exact key.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable or a
    /// stored row fails to decode.
    fn load(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
    ) -> Result<Option<InboxRow>, WorkerError>;

    /// Load at most `limit` active responses in stable key order.
    ///
    /// This is an operator and test surface. The worker hot path uses
    /// [`InboxStore::load`] so one due wake never scans unrelated tenants.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable or a
    /// stored row fails to decode.
    fn load_batch(&self, limit: usize) -> Result<Vec<InboxRow>, WorkerError>;

    /// Remove a buffered response only if its digest still matches the row
    /// the worker consumed.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn delete_if_digest(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
        expected_digest: Digest,
    ) -> Result<bool, WorkerError>;

    /// Atomically move a matching active response to the dead-letter store.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn dead_letter(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
        expected_digest: Digest,
        reason_code: &str,
        rejected_at: Timestamp,
    ) -> Result<bool, WorkerError>;

    /// Load at most `limit` dead letters, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable or a
    /// stored row fails to decode.
    fn load_dead_letters(&self, limit: usize) -> Result<Vec<DeadLetterRow>, WorkerError>;

    /// Purge at most `limit` dead letters older than `before`.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn purge_dead_letters(&self, before: Timestamp, limit: usize) -> Result<usize, WorkerError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::Timestamp;

    use super::{InboxInsertOutcome, InboxKind, InboxRow, InboxStore};

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        finstack_ai_kernel::Id::from_bytes(bytes)
    }

    fn exercise_inbox(store: &dyn InboxStore) {
        let row = InboxRow::try_new(
            Arc::from("tenant-a"),
            id(1),
            Arc::from("interaction-1"),
            InboxKind::Interaction,
            Arc::from(&br#"{"approved":true}"#[..]),
            ts(3_000),
        )
        .expect("row");
        assert_eq!(
            store.insert(&row).expect("insert"),
            InboxInsertOutcome::Inserted
        );
        assert_eq!(
            store.insert(&row).expect("equal replay"),
            InboxInsertOutcome::Idempotent
        );
        let mut redelivery = row.clone();
        redelivery.payload = Arc::from(&br#"{"approved":false}"#[..]);
        redelivery.payload_digest = finstack_ai_kernel::Digest::blob_content(&redelivery.payload);
        assert!(matches!(
            store.insert(&redelivery),
            Err(crate::WorkerError::Conflict {
                code: "inbox_conflict"
            })
        ));
        assert_eq!(
            store.load_batch(10).expect("batch"),
            vec![row.clone()],
            "a conflicting replay must not replace the accepted response"
        );
        assert!(
            store
                .delete_if_digest("tenant-a", id(1), "interaction-1", row.payload_digest,)
                .expect("consume")
        );
        assert!(store.load_batch(10).expect("drained").is_empty());

        assert_eq!(
            store.insert(&row).expect("reinsert"),
            InboxInsertOutcome::Inserted
        );
        assert!(
            store
                .dead_letter(
                    "tenant-a",
                    id(1),
                    "interaction-1",
                    row.payload_digest,
                    "invalid_command",
                    ts(4_000),
                )
                .expect("dead letter")
        );
        assert!(store.load_batch(10).expect("active").is_empty());
        let rejected = store.load_dead_letters(10).expect("dead letters");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].reason_code.as_ref(), "invalid_command");
        assert_eq!(store.purge_dead_letters(ts(5_000), 10).expect("purge"), 1);
    }

    #[test]
    fn memory_inbox_round_trips() {
        exercise_inbox(&crate::MemoryWorkerStore::new());
    }

    #[test]
    fn sqlite_inbox_round_trips() {
        let dir = tempfile::tempdir().expect("dir");
        let store = crate::SqliteWorkerStore::try_open(dir.path().join("w.sqlite")).expect("open");
        exercise_inbox(&store);
    }
}
