//! Bounded wake scanning must not starve ready work.

use finstack_ai_kernel::*;
use finstack_ai_runtime::workflow::WorkflowSession;
use finstack_ai_workflow_worker::*;
use std::sync::Arc;
fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}
/// Registered so the ready row is claimed; the resume then fails before
/// `bind` is ever reached because the journal holds no such session.
struct NeverBinds;
impl PortsFactory for NeverBinds {
    fn bind(&self, _session: WorkflowSession) -> Result<WorkflowSession, WorkerError> {
        Err(WorkerError::InvalidConfiguration {
            code: "never_binds",
        })
    }
}
async fn wake_fairness(store: Arc<dyn WakeIndexStore>, inbox: Arc<dyn InboxStore>) {
    let journal = Arc::new(
        finstack_ai_store_memory::MemoryJournalStore::try_new(
            finstack_ai_store_memory::MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 64,
                records_per_session: 256,
                snapshot_bytes: 4096,
            },
        )
        .unwrap(),
    );
    // Due (a response is buffered) but served by no factory here: the tick
    // skips it unclaimed, and it must not shadow the ready timer behind it.
    let row = WakeRow {
        tenant_scope: Arc::from("t1"),
        session_id: id(1),
        lane_id: id(1),
        run_id: id(1),
        workflow_kind: Arc::from("unregistered"),
        reason: WakeReason::Interaction,
        wake_at: None,
        expires_at: None,
        pending_id: Arc::from("waiting"),
        leased_by: None,
        lease_id: None,
        lease_expires_at: None,
        attempts: 0,
    };
    store.upsert(&row, None).unwrap();
    inbox
        .insert(
            &InboxRow::try_new(
                Arc::clone(&row.tenant_scope),
                row.session_id,
                Arc::clone(&row.pending_id),
                InboxKind::Interaction,
                Arc::from(&b"{}"[..]),
                UNIX_EPOCH,
            )
            .unwrap(),
        )
        .unwrap();
    let mut ready = row.clone();
    ready.session_id = id(2);
    ready.workflow_kind = Arc::from("stuck");
    ready.reason = WakeReason::Timer;
    ready.wake_at = Some(UNIX_EPOCH);
    ready.pending_id = Arc::from("ready");
    store.upsert(&ready, None).unwrap();
    let worker = WorkerBuilder::new(journal, store.clone(), inbox)
        .batch_limit(1)
        .register_ports("stuck", Arc::new(NeverBinds))
        .build()
        .unwrap();
    let attempts = |session: SessionId| {
        store
            .load_tenant("t1")
            .unwrap()
            .into_iter()
            .find(|row| row.session_id == session)
            .unwrap()
            .attempts
    };
    assert_eq!(worker.tick().await.unwrap().failures, 0);
    assert_eq!(attempts(id(2)), 0, "the one-row page held the skipped row");
    let report = worker.tick().await.unwrap();
    // The ready timer is claimed on the next page despite the unchanged
    // interaction row ahead of it.
    assert_eq!(store.load_tenant("t1").unwrap().len(), 2);
    assert_eq!(report.failures, 1);
    assert_eq!(
        attempts(id(2)),
        1,
        "the ready timer was claimed and backed off"
    );
    assert_eq!(attempts(id(1)), 0, "the unserved row is never written");
}
#[tokio::test]
async fn memory_worker_eventually_visits_ready_timer() {
    let store = Arc::new(MemoryWorkerStore::new());
    wake_fairness(store.clone(), store).await;
}
#[tokio::test]
async fn sqlite_worker_eventually_visits_ready_timer() {
    let store = Arc::new(SqliteWorkerStore::try_open(":memory:").unwrap());
    wake_fairness(store.clone(), store).await;
}
