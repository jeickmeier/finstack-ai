//! Bounded wake scanning must not starve ready work.

use finstack_ai_kernel::*;
use finstack_ai_workflow_worker::*;
use std::sync::Arc;
fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}
async fn wake_fairness(
    store: Arc<dyn WakeIndexStore>,
    inbox: Arc<dyn InboxStore>,
    fires: Arc<dyn FireStore>,
) {
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
        lease_expires_at: None,
        attempts: 0,
    };
    store.upsert(&row).unwrap();
    let mut ready = row.clone();
    ready.session_id = id(2);
    ready.reason = WakeReason::Timer;
    ready.wake_at = Some(UNIX_EPOCH);
    ready.pending_id = Arc::from("ready");
    store.upsert(&ready).unwrap();
    let worker = WorkerBuilder::new(
        journal,
        Arc::new(finstack_ai_workflow_local::MemoryCronStore::new()),
        store.clone(),
        fires,
        inbox,
    )
    .batch_limit(1)
    .build()
    .unwrap();
    assert_eq!(worker.tick().await.unwrap().failures, 0);
    let report = worker.tick().await.unwrap();
    // The ready timer reaches factory lookup despite the unchanged interaction.
    assert_eq!(store.load_tenant("t1").unwrap().len(), 2);
    assert_eq!(report.failures, 1);
}
#[tokio::test]
async fn memory_worker_eventually_visits_ready_timer() {
    let store = Arc::new(MemoryWorkerStore::new());
    wake_fairness(store.clone(), store.clone(), store).await;
}
#[tokio::test]
async fn sqlite_worker_eventually_visits_ready_timer() {
    let store = Arc::new(SqliteWorkerStore::try_open(":memory:").unwrap());
    wake_fairness(store.clone(), store.clone(), store).await;
}
