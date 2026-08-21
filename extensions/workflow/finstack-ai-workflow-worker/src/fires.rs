//! Cron-fire records: idempotency guards for the local workflow driver's
//! scheduled fires.
//!
//! Each row records that a cron fire was claimed (and, later, that a
//! session was started for it). The primary key `(tenant_scope,
//! schedule_id, fire_count)` makes `record_claimed` idempotent, so a worker
//! that crashes after claiming a fire but before starting a session can
//! retry safely on restart.

use std::sync::Arc;

use finstack_ai_kernel::{SessionId, Timestamp};

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
    pub started_session: Option<SessionId>,
}

/// Collision-resistant idempotency identity for one claimed fire.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FireIdempotencyKey(Arc<str>);

impl FireIdempotencyKey {
    /// Borrow the stable `wf-fire-v1:<digest>` representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Result of marking a claimed fire as started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireStartOutcome {
    /// The claimed fire transitioned to started.
    Started,
    /// The same session had already been recorded for this fire.
    Idempotent,
}

/// Stable idempotency key for a claimed fire.
///
/// The digest input uses length-prefixed tenant and schedule bytes followed
/// by the big-endian fire counter, so delimiter characters cannot create
/// ambiguous identities.
#[must_use]
pub fn idempotency_key(row: &FireRow) -> FireIdempotencyKey {
    let tenant = row.tenant_scope.as_bytes();
    let schedule = row.schedule_id.as_bytes();
    let mut canonical = Vec::with_capacity(16 + tenant.len() + schedule.len() + 8);
    canonical.extend_from_slice(
        &u64::try_from(tenant.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    canonical.extend_from_slice(tenant);
    canonical.extend_from_slice(
        &u64::try_from(schedule.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    canonical.extend_from_slice(schedule);
    canonical.extend_from_slice(&row.fire_count.to_be_bytes());
    let digest = finstack_ai_kernel::fixed_domain_digest!(
        "workflow-fire-idempotency",
        1,
        canonical.as_slice(),
    );
    FireIdempotencyKey(Arc::from(format!("wf-fire-v1:{}", digest.to_hex())))
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
        started_session: SessionId,
    ) -> Result<FireStartOutcome, WorkerError>;

    /// Load every claimed-but-not-started fire, across all tenants, ordered
    /// by `(tenant_scope, schedule_id, fire_count)`.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable or a
    /// stored row fails to decode.
    fn load_unstarted(&self, limit: usize) -> Result<Vec<FireRow>, WorkerError>;

    /// Purge at most `limit` started fires older than `before`.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError`] when the adapter table is unavailable.
    fn purge_started(&self, before: Timestamp, limit: usize) -> Result<usize, WorkerError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::Timestamp;

    use super::{FireRow, FireStartOutcome, FireStatus, FireStore, idempotency_key};

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
        let mut replay = row.clone();
        replay.fired_at = ts(2_500);
        store
            .record_claimed(&replay)
            .expect("idempotent re-claim (different fired_at, same key)");
        assert_eq!(
            store.load_unstarted(10).expect("unstarted"),
            vec![row.clone()],
            "record_claimed must keep the first row (INSERT OR IGNORE), not the replay"
        );
        assert_eq!(
            store
                .mark_started("tenant-a", "nightly", 7, id(9))
                .expect("start"),
            FireStartOutcome::Started
        );
        assert_eq!(
            store
                .mark_started("tenant-a", "nightly", 7, id(9))
                .expect("idempotent start"),
            FireStartOutcome::Idempotent
        );
        assert!(store.load_unstarted(10).expect("drained").is_empty());
        assert_eq!(store.purge_started(ts(3_000), 10).expect("purge"), 1);
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
        let key = idempotency_key(&row);
        assert!(key.as_str().starts_with("wf-fire-v1:"));
        assert_eq!(key.as_str().len(), "wf-fire-v1:".len() + 64);
    }

    #[test]
    fn idempotency_key_has_no_delimiter_collisions() {
        let left = FireRow {
            tenant_scope: Arc::from("a:b"),
            schedule_id: Arc::from("c"),
            fire_count: 7,
            fired_at: ts(2_000),
            status: FireStatus::Claimed,
            started_session: None,
        };
        let right = FireRow {
            tenant_scope: Arc::from("a"),
            schedule_id: Arc::from("b:c"),
            ..left.clone()
        };
        assert_ne!(idempotency_key(&left), idempotency_key(&right));
    }

    fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        finstack_ai_kernel::Id::from_bytes(bytes)
    }
}
