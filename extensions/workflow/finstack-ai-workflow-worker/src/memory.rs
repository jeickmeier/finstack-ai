//! In-process adapter tables backing the workflow worker in tests and
//! non-durable deployments.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use finstack_ai_kernel::{SessionId, Timestamp};

use crate::error::WorkerError;
use crate::fires::{FireRow, FireStartOutcome, FireStatus, FireStore};
use crate::inbox::{DeadLetterRow, InboxInsertOutcome, InboxRow, InboxStore};
use crate::wake::{WakeIndexStore, WakeRow, lease_deadline, lease_open, wake_due};

type WakeRows = BTreeMap<(Arc<str>, SessionId), WakeRow>;
type FireRows = BTreeMap<(Arc<str>, Arc<str>, u64), FireRow>;
type InboxRows = BTreeMap<(Arc<str>, SessionId, Arc<str>), InboxRow>;

#[derive(Debug, Default)]
struct MemoryInboxState {
    active: InboxRows,
    dead_letters: Vec<DeadLetterRow>,
}

/// In-memory worker store. Implements the wake index, cron-fire, and inbox
/// tables behind a single mutex per table.
#[derive(Debug, Default)]
pub struct MemoryWorkerStore {
    wake: Mutex<WakeRows>,
    wake_cursor: Mutex<Option<(Arc<str>, SessionId)>>,
    fires: Mutex<FireRows>,
    inbox: Mutex<MemoryInboxState>,
}

impl MemoryWorkerStore {
    /// Empty store with no rows.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lock one adapter table, failing closed on a poisoned mutex.
fn locked<T>(table: &Mutex<T>) -> Result<MutexGuard<'_, T>, WorkerError> {
    table.lock().map_err(|_| WorkerError::StoreUnavailable {
        code: "memory_worker_lock_poisoned",
    })
}

impl WakeIndexStore for MemoryWorkerStore {
    fn upsert(&self, row: &WakeRow) -> Result<(), WorkerError> {
        let mut wake = locked(&self.wake)?;
        wake.insert((Arc::clone(&row.tenant_scope), row.session_id), row.clone());
        Ok(())
    }

    fn delete(&self, tenant_scope: &str, session_id: SessionId) -> Result<(), WorkerError> {
        let mut wake = locked(&self.wake)?;
        wake.remove(&(Arc::from(tenant_scope), session_id));
        Ok(())
    }

    fn load_due(&self, now: Timestamp, limit: usize) -> Result<Vec<WakeRow>, WorkerError> {
        let wake = locked(&self.wake)?;
        let mut cursor = locked(&self.wake_cursor)?;
        let (after, before) = match cursor.as_ref() {
            Some(key) => (
                wake.range((std::ops::Bound::Excluded(key), std::ops::Bound::Unbounded)),
                wake.range((std::ops::Bound::Unbounded, std::ops::Bound::Included(key))),
            ),
            None => (wake.range(..), wake.range(..)),
        };
        let rows: Vec<WakeRow> = after
            .chain(before.take(if cursor.is_some() { usize::MAX } else { 0 }))
            .map(|(_, row)| row)
            .filter(|row| lease_open(row, now) && wake_due(row, now))
            .take(limit)
            .cloned()
            .collect();
        if let Some(last) = rows.last() {
            *cursor = Some((Arc::clone(&last.tenant_scope), last.session_id));
        }
        Ok(rows)
    }

    fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<WakeRow>, WorkerError> {
        let wake = locked(&self.wake)?;
        Ok(wake
            .values()
            .filter(|row| row.tenant_scope.as_ref() == tenant_scope)
            .cloned()
            .collect())
    }

    fn contains_interaction(
        &self,
        tenant_scope: &str,
        pending_id: &str,
    ) -> Result<bool, WorkerError> {
        let wake = locked(&self.wake)?;
        Ok(wake.values().any(|row| {
            row.tenant_scope.as_ref() == tenant_scope
                && row.reason == crate::wake::WakeReason::Interaction
                && row.pending_id.as_ref() == pending_id
        }))
    }

    fn try_claim(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let mut wake = locked(&self.wake)?;
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
        let mut wake = locked(&self.wake)?;
        let Some(row) = wake.get_mut(&(Arc::from(tenant_scope), session_id)) else {
            return Ok(false);
        };
        if row.leased_by.as_deref() != Some(worker_id) {
            return Ok(false);
        }
        row.lease_expires_at = Some(lease_deadline(now, lease_ttl_ms)?);
        Ok(true)
    }

    fn release(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
    ) -> Result<bool, WorkerError> {
        let mut wake = locked(&self.wake)?;
        let Some(row) = wake.get_mut(&(Arc::from(tenant_scope), session_id)) else {
            return Ok(false);
        };
        if row.leased_by.as_deref() != Some(worker_id) {
            return Ok(false);
        }
        row.leased_by = None;
        row.lease_expires_at = None;
        Ok(true)
    }

    fn record_failure(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        retry_at: Timestamp,
    ) -> Result<(), WorkerError> {
        let mut wake = locked(&self.wake)?;
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
        let mut fires = locked(&self.fires)?;
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
        started_session: SessionId,
    ) -> Result<FireStartOutcome, WorkerError> {
        let mut fires = locked(&self.fires)?;
        let key = (Arc::from(tenant_scope), Arc::from(schedule_id), fire_count);
        let row = fires.get_mut(&key).ok_or(WorkerError::StoreIntegrity {
            code: "fire_missing",
        })?;
        match (row.status, row.started_session) {
            (FireStatus::Claimed, None) => {
                row.status = FireStatus::Started;
                row.started_session = Some(started_session);
                Ok(FireStartOutcome::Started)
            }
            (FireStatus::Started, Some(existing)) if existing == started_session => {
                Ok(FireStartOutcome::Idempotent)
            }
            _ => Err(WorkerError::Conflict {
                code: "fire_start_conflict",
            }),
        }
    }

    fn load_unstarted(&self, limit: usize) -> Result<Vec<FireRow>, WorkerError> {
        let fires = locked(&self.fires)?;
        Ok(fires
            .values()
            .filter(|row| row.status == FireStatus::Claimed)
            .take(limit)
            .cloned()
            .collect())
    }

    fn purge_started(&self, before: Timestamp, limit: usize) -> Result<usize, WorkerError> {
        let mut fires = locked(&self.fires)?;
        let keys: Vec<_> = fires
            .iter()
            .filter(|(_, row)| row.status == FireStatus::Started && row.fired_at < before)
            .take(limit)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            fires.remove(key);
        }
        Ok(keys.len())
    }
}

impl InboxStore for MemoryWorkerStore {
    fn insert(&self, row: &InboxRow) -> Result<InboxInsertOutcome, WorkerError> {
        if !row.digest_is_valid() {
            return Err(WorkerError::StoreIntegrity {
                code: "inbox_digest",
            });
        }
        let mut inbox = locked(&self.inbox)?;
        let key = (
            Arc::clone(&row.tenant_scope),
            row.session_id,
            Arc::clone(&row.pending_id),
        );
        match inbox.active.get(&key) {
            Some(existing)
                if existing.kind == row.kind && existing.payload_digest == row.payload_digest =>
            {
                Ok(InboxInsertOutcome::Idempotent)
            }
            Some(_) => Err(WorkerError::Conflict {
                code: "inbox_conflict",
            }),
            None => {
                inbox.active.insert(key, row.clone());
                Ok(InboxInsertOutcome::Inserted)
            }
        }
    }

    fn load(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
    ) -> Result<Option<InboxRow>, WorkerError> {
        let inbox = locked(&self.inbox)?;
        Ok(inbox
            .active
            .get(&(Arc::from(tenant_scope), session_id, Arc::from(pending_id)))
            .cloned())
    }

    fn load_batch(&self, limit: usize) -> Result<Vec<InboxRow>, WorkerError> {
        let inbox = locked(&self.inbox)?;
        Ok(inbox.active.values().take(limit).cloned().collect())
    }

    fn delete_if_digest(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
        expected_digest: finstack_ai_kernel::Digest,
    ) -> Result<bool, WorkerError> {
        let mut inbox = locked(&self.inbox)?;
        let key = (Arc::from(tenant_scope), session_id, Arc::from(pending_id));
        if inbox
            .active
            .get(&key)
            .is_none_or(|row| row.payload_digest != expected_digest)
        {
            return Ok(false);
        }
        inbox.active.remove(&key);
        Ok(true)
    }

    fn dead_letter(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        pending_id: &str,
        expected_digest: finstack_ai_kernel::Digest,
        reason_code: &str,
        rejected_at: Timestamp,
    ) -> Result<bool, WorkerError> {
        let key = (Arc::from(tenant_scope), session_id, Arc::from(pending_id));
        let mut inbox = locked(&self.inbox)?;
        if inbox
            .active
            .get(&key)
            .is_none_or(|row| row.payload_digest != expected_digest)
        {
            return Ok(false);
        }
        let response = inbox.active.remove(&key);
        let Some(response) = response else {
            return Ok(false);
        };
        inbox.dead_letters.push(DeadLetterRow {
            response,
            reason_code: Arc::from(reason_code),
            rejected_at,
        });
        inbox.dead_letters.sort_by(|left, right| {
            left.rejected_at
                .cmp(&right.rejected_at)
                .then_with(|| left.response.tenant_scope.cmp(&right.response.tenant_scope))
                .then_with(|| left.response.session_id.cmp(&right.response.session_id))
                .then_with(|| left.response.pending_id.cmp(&right.response.pending_id))
        });
        Ok(true)
    }

    fn load_dead_letters(&self, limit: usize) -> Result<Vec<DeadLetterRow>, WorkerError> {
        let inbox = locked(&self.inbox)?;
        Ok(inbox.dead_letters.iter().take(limit).cloned().collect())
    }

    fn purge_dead_letters(&self, before: Timestamp, limit: usize) -> Result<usize, WorkerError> {
        let mut inbox = locked(&self.inbox)?;
        let mut removed = 0_usize;
        inbox.dead_letters.retain(|row| {
            if removed < limit && row.rejected_at < before {
                removed += 1;
                false
            } else {
                true
            }
        });
        Ok(removed)
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
            expires_at: None,
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
        assert!(store.load_due(ts(1_999), 10).expect("early").is_empty());
        assert_eq!(store.load_due(ts(2_000), 10).expect("due").len(), 1);
    }

    #[test]
    fn non_timer_rows_are_always_due() {
        let store = MemoryWorkerStore::new();
        let mut row = timer_row("tenant-a", 1, 9_000);
        row.reason = WakeReason::Interaction;
        row.wake_at = None;
        store.upsert(&row).expect("upsert");
        assert_eq!(store.load_due(ts(0), 10).expect("due").len(), 1);
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
        assert!(store.load_due(ts(1_600), 10).expect("hidden").is_empty());
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
        assert!(store.load_due(ts(4_999), 10).expect("backoff").is_empty());
        assert_eq!(store.load_due(ts(5_000), 10).expect("retry").len(), 1);
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
