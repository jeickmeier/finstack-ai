//! Durable response inbox: buffers interaction resolutions and external
//! effect completions until the worker consumes them.
//!
//! Each row is keyed by `(tenant_scope, session_id, pending_id)`. A
//! redelivered response overwrites the prior payload rather than erroring —
//! duplicate settlement is guarded on the journal side, not here (see
//! §10.4). The stored `payload` is opaque bytes: the `serde_json`
//! serialization of an `InteractionResolutionCommand` or
//! `ExternalEffectCompletionCommand`
//! (`crates/finstack-ai-kernel/src/records/run/external.rs:109-218`). This
//! module never deserializes it — that happens at consume time in the
//! worker.

use std::sync::Arc;

use finstack_ai_kernel::{SessionId, Timestamp};

use crate::error::WorkerError;

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
            _ => Err(WorkerError::StoreIntegrity {
                code: "inbox_kind",
            }),
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
    /// When the response was received.
    pub received_at: Timestamp,
}

/// Adapter table buffering durable responses for the workflow worker.
pub trait InboxStore: Send + Sync {
    /// Insert or overwrite a buffered response. A redelivered response
    /// replaces the prior payload for the same
    /// `(tenant_scope, session_id, pending_id)` key.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn insert(&self, row: &InboxRow) -> Result<(), WorkerError>;

    /// Load every buffered response, across all tenants.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable or a
    /// stored row fails to decode.
    fn load_all(&self) -> Result<Vec<InboxRow>, WorkerError>;

    /// Remove a buffered response once it has been consumed.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn delete(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
    ) -> Result<(), WorkerError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::Timestamp;

    use super::{InboxKind, InboxRow, InboxStore};

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
        let row = InboxRow {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            pending_id: Arc::from("interaction-1"),
            kind: InboxKind::Interaction,
            payload: Arc::from(&br#"{"approved":true}"#[..]),
            received_at: ts(3_000),
        };
        store.insert(&row).expect("insert");
        store.insert(&row).expect("redelivery replaces");
        assert_eq!(store.load_all().expect("all"), vec![row.clone()]);
        store
            .delete("tenant-a", id(1), "interaction-1")
            .expect("consume");
        assert!(store.load_all().expect("drained").is_empty());
    }

    #[test]
    fn memory_inbox_round_trips() {
        exercise_inbox(&crate::MemoryWorkerStore::new());
    }

    #[test]
    fn sqlite_inbox_round_trips() {
        let dir = tempfile::tempdir().expect("dir");
        let store = crate::SqliteWorkerStore::open(dir.path().join("w.sqlite")).expect("open");
        exercise_inbox(&store);
    }
}
