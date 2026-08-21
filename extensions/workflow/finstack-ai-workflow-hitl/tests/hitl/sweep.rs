//! Reconciliation behavior for the HITL router.

use std::sync::Arc;

use finstack_ai_kernel::{Id, InteractionTag};
use finstack_ai_runtime::{JournalStore, WorkflowWait};
use finstack_ai_workflow_hitl::{
    HitlInboxStore, HitlRouter, InteractionStatus, MemoryHitlStore, capture,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, MemoryWorkerStore, WakeIndexStore, WakeReason, WakeRow, WorkerBuilder,
};

use crate::capture::{accepted, approval_request, checkpoint, id, memory_store, timestamp};

fn harness() -> (
    Arc<MemoryHitlStore>,
    Arc<MemoryWorkerStore>,
    HitlRouter,
    Id<InteractionTag>,
) {
    let inbox = Arc::new(MemoryHitlStore::new());
    let interaction_id = id(50);
    let wait = WorkflowWait::Interaction {
        interaction_id,
        request: approval_request(interaction_id, id(51), None),
    };
    assert!(
        capture(
            inbox.as_ref(),
            &checkpoint(),
            &wait,
            accepted().security(),
            timestamp(2_000),
        )
        .expect("capture")
    );

    let worker_store = Arc::new(MemoryWorkerStore::new());
    worker_store
        .upsert(&WakeRow {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(2),
            run_id: id(3),
            workflow_kind: Arc::from("research"),
            reason: WakeReason::Interaction,
            wake_at: None,
            expires_at: None,
            pending_id: Arc::from(interaction_id.to_canonical_string()),
            leased_by: None,
            lease_expires_at: None,
            attempts: 0,
        })
        .expect("wake");
    let worker = Arc::new(
        WorkerBuilder::new(
            memory_store() as Arc<dyn JournalStore>,
            Arc::new(MemoryCronStore::new()),
            Arc::clone(&worker_store) as Arc<dyn WakeIndexStore>,
            Arc::clone(&worker_store) as Arc<dyn FireStore>,
            Arc::clone(&worker_store) as Arc<dyn InboxStore>,
        )
        .build()
        .expect("worker"),
    );
    let router = HitlRouter::new(
        Arc::clone(&inbox) as Arc<dyn HitlInboxStore>,
        Arc::clone(&worker),
        Arc::clone(&worker_store) as Arc<dyn WakeIndexStore>,
    );
    (inbox, worker_store, router, interaction_id)
}

#[test]
fn sweep_keeps_an_interaction_that_is_still_awaited() {
    let (inbox, _worker_store, router, interaction_id) = harness();

    assert_eq!(router.sweep(timestamp(3_000)).expect("sweep").reconciled, 0);
    let row = inbox
        .load("tenant-a", &interaction_id.to_canonical_string())
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Open);
}

#[test]
fn sweep_closes_an_active_row_after_its_wake_is_consumed() {
    let (inbox, worker_store, router, interaction_id) = harness();
    worker_store
        .delete("tenant-a", id(1))
        .expect("consume wake");

    assert_eq!(router.sweep(timestamp(3_000)).expect("sweep").reconciled, 1);
    let row = inbox
        .load("tenant-a", &interaction_id.to_canonical_string())
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Closed);
    assert_eq!(row.outcome_code.as_deref(), Some("wake_reconciled"));
}
