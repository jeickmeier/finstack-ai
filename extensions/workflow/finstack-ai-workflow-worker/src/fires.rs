//! Cron-fire records: idempotency guards for the local workflow driver's
//! scheduled fires.
//!
//! Each row records that a cron fire was claimed (and, later, that a
//! session was started for it). The primary key `(tenant_scope,
//! schedule_id, fire_count)` makes `record_claimed` idempotent, so a worker
//! that crashes after claiming a fire but before starting a session can
//! retry safely on restart.

use std::sync::Arc;

use finstack_ai_kernel::Timestamp;

use crate::error::WorkerError;

/// Lifecycle state of a claimed cron fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireStatus {
    /// The fire was claimed but no session has started yet.
    Claimed,
    /// A session was started for this fire.
    Started,
}

impl FireStatus {
    /// Stable lowercase representation used in storage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Started => "started",
        }
    }

    /// Parse a stored status, failing closed on anything unrecognized.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::StoreIntegrity`] with code `"fire_status"` for
    /// any value other than `"claimed"` or `"started"`.
    pub fn parse(value: &str) -> Result<Self, WorkerError> {
        match value {
            "claimed" => Ok(Self::Claimed),
            "started" => Ok(Self::Started),
            _ => Err(WorkerError::StoreIntegrity {
                code: "fire_status",
            }),
        }
    }
}

/// One claimed cron fire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FireRow {
    /// Tenant that owns the schedule.
    pub tenant_scope: Arc<str>,
    /// Identifier of the cron schedule that fired.
    pub schedule_id: Arc<str>,
    /// Monotonic fire counter from the cron schedule.
    pub fire_count: u64,
    /// When the fire was claimed.
    pub fired_at: Timestamp,
    /// Current lifecycle status.
    pub status: FireStatus,
    /// Session started for this fire, once known.
    pub started_session: Option<Arc<str>>,
}

/// Stable idempotency key for a claimed fire: `tenant:schedule:fire_count`.
#[must_use]
pub fn idempotency_key(row: &FireRow) -> String {
    format!("{}:{}:{}", row.tenant_scope, row.schedule_id, row.fire_count)
}

/// Adapter table recording claimed cron fires.
///
/// Implementations back the local workflow driver's cron loop: claiming a
/// fire is idempotent (retrying an already-claimed fire is a no-op), and
/// `load_unstarted` lets a worker resume sessions for fires that were
/// claimed but never started.
pub trait FireStore: Send + Sync {
    /// Record that `row` was claimed. Idempotent: re-recording the same
    /// `(tenant_scope, schedule_id, fire_count)` key is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn record_claimed(&self, row: &FireRow) -> Result<(), WorkerError>;

    /// Mark a claimed fire as started, recording the session that was
    /// started for it.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn mark_started(
        &self,
        tenant_scope: &str,
        schedule_id: &str,
        fire_count: u64,
        started_session: &str,
    ) -> Result<(), WorkerError>;

    /// Load every claimed-but-not-started fire, across all tenants, ordered
    /// by `(tenant_scope, schedule_id, fire_count)`.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable or a
    /// stored row fails to decode.
    fn load_unstarted(&self) -> Result<Vec<FireRow>, WorkerError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::Timestamp;

    use super::{FireRow, FireStatus, FireStore, idempotency_key};

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn exercise_fire_store(store: &dyn FireStore) {
        let row = FireRow {
            tenant_scope: Arc::from("tenant-a"),
            schedule_id: Arc::from("nightly"),
            fire_count: 7,
            fired_at: ts(2_000),
            status: FireStatus::Claimed,
            started_session: None,
        };
        store.record_claimed(&row).expect("claim");
        store.record_claimed(&row).expect("idempotent re-claim");
        assert_eq!(store.load_unstarted().expect("unstarted"), vec![row.clone()]);
        store
            .mark_started("tenant-a", "nightly", 7, "session-9")
            .expect("start");
        assert!(store.load_unstarted().expect("drained").is_empty());
    }

    #[test]
    fn memory_fire_store_round_trips() {
        exercise_fire_store(&crate::MemoryWorkerStore::new());
    }

    #[test]
    fn sqlite_fire_store_round_trips() {
        let dir = tempfile::tempdir().expect("dir");
        let store = crate::SqliteWorkerStore::open(dir.path().join("w.sqlite")).expect("open");
        exercise_fire_store(&store);
    }

    #[test]
    fn idempotency_key_is_tenant_schedule_count() {
        let row = FireRow {
            tenant_scope: Arc::from("tenant-a"),
            schedule_id: Arc::from("nightly"),
            fire_count: 7,
            fired_at: ts(2_000),
            status: FireStatus::Claimed,
            started_session: None,
        };
        assert_eq!(idempotency_key(&row), "tenant-a:nightly:7");
    }
}
