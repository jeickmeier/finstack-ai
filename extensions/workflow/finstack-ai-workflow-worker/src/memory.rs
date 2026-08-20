//! In-process adapter tables backing the workflow worker in tests and
//! non-durable deployments.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{SessionId, Timestamp};

use crate::error::WorkerError;
use crate::fires::{FireRow, FireStatus, FireStore};
use crate::inbox::{InboxRow, InboxStore};
use crate::wake::{WakeIndexStore, WakeRow, lease_deadline, lease_open, wake_due};

type WakeRows = BTreeMap<(Arc<str>, SessionId), WakeRow>;
type FireRows = BTreeMap<(Arc<str>, Arc<str>, u64), FireRow>;
type InboxRows = BTreeMap<(Arc<str>, SessionId, Arc<str>), InboxRow>;

/// In-memory worker store. Implements the wake index, cron-fire, and inbox
/// tables behind a single mutex per table.
#[derive(Debug, Default)]
pub struct MemoryWorkerStore {
    wake: Mutex<WakeRows>,
    fires: Mutex<FireRows>,
    inbox: Mutex<InboxRows>,
}

impl MemoryWorkerStore {
    /// Empty store with no rows.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl WakeIndexStore for MemoryWorkerStore {
    fn upsert(&self, row: &WakeRow) -> Result<(), WorkerError> {
        let mut wake = self
            .wake
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        wake.insert((Arc::clone(&row.tenant_scope), row.session_id), row.clone());
        Ok(())
    }

    fn delete(&self, tenant_scope: &str, session_id: SessionId) -> Result<(), WorkerError> {
        let mut wake = self
            .wake
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        wake.remove(&(Arc::from(tenant_scope), session_id));
        Ok(())
    }

    fn load_due(&self, now: Timestamp) -> Result<Vec<WakeRow>, WorkerError> {
        let wake = self
            .wake
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        Ok(wake
            .values()
            .filter(|row| lease_open(row, now) && wake_due(row, now))
            .cloned()
            .collect())
    }

    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<WakeRow>, WorkerError> {
        let wake = self
            .wake
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        Ok(wake
            .values()
            .filter(|row| row.tenant_scope.as_ref() == tenant_scope)
            .cloned()
            .collect())
    }

    fn try_claim(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let mut wake = self
            .wake
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        let Some(row) = wake.get_mut(&(Arc::from(tenant_scope), session_id)) else {
            return Ok(false);
        };
        if !lease_open(row, now) {
            return Ok(false);
        }
        row.leased_by = Some(Arc::from(worker_id));
        row.lease_expires_at = Some(lease_deadline(now, lease_ttl_ms)?);
        Ok(true)
    }

    fn renew(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let mut wake = self
            .wake
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        let Some(row) = wake.get_mut(&(Arc::from(tenant_scope), session_id)) else {
            return Ok(false);
        };
        if row.leased_by.as_deref() != Some(worker_id) {
            return Ok(false);
        }
        row.lease_expires_at = Some(lease_deadline(now, lease_ttl_ms)?);
        Ok(true)
    }

    fn record_failure(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        retry_at: Timestamp,
    ) -> Result<(), WorkerError> {
        let mut wake = self
            .wake
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        let Some(row) = wake.get_mut(&(Arc::from(tenant_scope), session_id)) else {
            return Ok(());
        };
        row.attempts += 1;
        row.leased_by = None;
        row.lease_expires_at = None;
        row.wake_at = Some(retry_at);
        Ok(())
    }
}

impl FireStore for MemoryWorkerStore {
    fn record_claimed(&self, row: &FireRow) -> Result<(), WorkerError> {
        let mut fires = self
            .fires
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        let key = (
            Arc::clone(&row.tenant_scope),
            Arc::clone(&row.schedule_id),
            row.fire_count,
        );
        fires.entry(key).or_insert_with(|| row.clone());
        Ok(())
    }

    fn mark_started(
        &self,
        tenant_scope: &str,
        schedule_id: &str,
        fire_count: u64,
        started_session: &str,
    ) -> Result<(), WorkerError> {
        let mut fires = self
            .fires
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        let key = (Arc::from(tenant_scope), Arc::from(schedule_id), fire_count);
        if let Some(row) = fires.get_mut(&key) {
            row.status = FireStatus::Started;
            row.started_session = Some(Arc::from(started_session));
        }
        Ok(())
    }

    fn load_unstarted(&self) -> Result<Vec<FireRow>, WorkerError> {
        let fires = self
            .fires
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        Ok(fires
            .values()
            .filter(|row| row.status == FireStatus::Claimed)
            .cloned()
            .collect())
    }
}

impl InboxStore for MemoryWorkerStore {
    fn insert(&self, row: &InboxRow) -> Result<(), WorkerError> {
        let mut inbox = self
            .inbox
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        let key = (
            Arc::clone(&row.tenant_scope),
            row.session_id,
            Arc::clone(&row.pending_id),
        );
        inbox.insert(key, row.clone());
        Ok(())
    }

    fn load_all(&self) -> Result<Vec<InboxRow>, WorkerError> {
        let inbox = self
            .inbox
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        Ok(inbox.values().cloned().collect())
    }

    fn delete(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
    ) -> Result<(), WorkerError> {
        let mut inbox = self
            .inbox
            .lock()
            .map_err(|_| WorkerError::StoreUnavailable {
                code: "memory_worker_lock_poisoned",
            })?;
        inbox.remove(&(Arc::from(tenant_scope), session_id, Arc::from(pending_id)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::Timestamp;

    use super::MemoryWorkerStore;
    use crate::wake::{WakeIndexStore, WakeReason, WakeRow};

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

    fn timer_row(tenant: &str, session: u64, due_ms: i64) -> WakeRow {
        WakeRow {
            tenant_scope: Arc::from(tenant),
            session_id: id(session),
            lane_id: id(2),
            run_id: id(3),
            workflow_kind: Arc::from("research"),
            reason: WakeReason::Timer,
            wake_at: Some(ts(due_ms)),
            pending_id: Arc::from("effect-1"),
            leased_by: None,
            lease_expires_at: None,
            attempts: 0,
        }
    }

    #[test]
    fn timer_rows_are_due_only_at_or_after_wake_at() {
        let store = MemoryWorkerStore::new();
        store
            .upsert(&timer_row("tenant-a", 1, 2_000))
            .expect("upsert");
        assert!(store.load_due(ts(1_999)).expect("early").is_empty());
        assert_eq!(store.load_due(ts(2_000)).expect("due").len(), 1);
    }

    #[test]
    fn non_timer_rows_are_always_due() {
        let store = MemoryWorkerStore::new();
        let mut row = timer_row("tenant-a", 1, 9_000);
        row.reason = WakeReason::Interaction;
        row.wake_at = None;
        store.upsert(&row).expect("upsert");
        assert_eq!(store.load_due(ts(0)).expect("due").len(), 1);
    }

    #[test]
    fn claim_excludes_row_until_lease_expires() {
        let store = MemoryWorkerStore::new();
        store
            .upsert(&timer_row("tenant-a", 1, 1_000))
            .expect("upsert");
        assert!(
            store
                .try_claim("tenant-a", id(1), "worker-a", ts(1_500), 1_000)
                .expect("first claim")
        );
        assert!(
            !store
                .try_claim("tenant-a", id(1), "worker-b", ts(1_600), 1_000)
                .expect("held")
        );
        assert!(store.load_due(ts(1_600)).expect("hidden").is_empty());
        assert!(
            store
                .try_claim("tenant-a", id(1), "worker-b", ts(2_600), 1_000)
                .expect("expired lease is claimable")
        );
    }

    #[test]
    fn record_failure_backs_off_and_unleases() {
        let store = MemoryWorkerStore::new();
        store
            .upsert(&timer_row("tenant-a", 1, 1_000))
            .expect("upsert");
        assert!(
            store
                .try_claim("tenant-a", id(1), "worker-a", ts(1_000), 1_000)
                .expect("claim")
        );
        store
            .record_failure("tenant-a", id(1), ts(5_000))
            .expect("failure");
        let rows = store.load_tenant("tenant-a").expect("load");
        assert_eq!(rows[0].attempts, 1);
        assert_eq!(rows[0].leased_by, None);
        assert!(store.load_due(ts(4_999)).expect("backoff").is_empty());
        assert_eq!(store.load_due(ts(5_000)).expect("retry").len(), 1);
    }

    #[test]
    fn tenants_are_isolated() {
        let store = MemoryWorkerStore::new();
        store.upsert(&timer_row("tenant-a", 1, 1_000)).expect("a");
        store.upsert(&timer_row("tenant-b", 2, 1_000)).expect("b");
        assert_eq!(store.load_tenant("tenant-a").expect("a").len(), 1);
        assert_eq!(store.load_tenant("tenant-b").expect("b").len(), 1);
    }
}
