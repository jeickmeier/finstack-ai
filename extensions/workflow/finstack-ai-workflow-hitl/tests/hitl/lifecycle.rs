//! Worker lifecycle callback behavior.

use std::sync::Arc;

use finstack_ai_kernel::{Id, InteractionTag};
use finstack_ai_runtime::workflow::WorkflowWait;
use finstack_ai_workflow_hitl::{
    HitlInboxStore, HitlLifecycle, InteractionStatus, InteractionTransition, MemoryHitlStore,
    capture,
};
use finstack_ai_workflow_worker::{InteractionDeliveryOutcome, InteractionLifecycle, WorkerError};

use crate::capture::{accepted, approval_request, checkpoint, id, timestamp};

fn buffered() -> (Arc<MemoryHitlStore>, HitlLifecycle, Id<InteractionTag>) {
    let store = Arc::new(MemoryHitlStore::new());
    let interaction_id = id(50);
    let wait = WorkflowWait::Interaction {
        interaction_id,
        request: approval_request(interaction_id, id(51), None),
    };
    capture(
        store.as_ref(),
        &checkpoint(),
        &wait,
        accepted().security(),
        timestamp(2_000),
    )
    .expect("capture");
    assert!(
        store
            .transition(
                "tenant-a",
                &interaction_id.to_canonical_string(),
                InteractionTransition {
                    expected: InteractionStatus::Open,
                    next: InteractionStatus::Buffered,
                    resolved_by: Some("subject"),
                    outcome_code: Some("buffered"),
                    updated_at: timestamp(3_000),
                },
            )
            .expect("buffer")
    );
    let lifecycle = HitlLifecycle::new(Arc::clone(&store) as Arc<dyn HitlInboxStore>);
    (store, lifecycle, interaction_id)
}

#[test]
fn accepted_delivery_is_recorded_idempotently() {
    let (store, lifecycle, interaction_id) = buffered();
    for _ in 0..2 {
        lifecycle
            .settled(
                "tenant-a",
                &interaction_id.to_canonical_string(),
                InteractionDeliveryOutcome::Accepted,
                timestamp(4_000),
            )
            .expect("accepted");
    }
    let row = store
        .load("tenant-a", &interaction_id.to_canonical_string())
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Accepted);
    assert_eq!(row.outcome_code.as_deref(), Some("accepted"));
}

#[test]
fn rejected_delivery_returns_to_the_actionable_view() {
    let (store, lifecycle, interaction_id) = buffered();
    lifecycle
        .settled(
            "tenant-a",
            &interaction_id.to_canonical_string(),
            InteractionDeliveryOutcome::Rejected {
                reason_code: "scope_mismatch",
            },
            timestamp(4_000),
        )
        .expect("rejected");
    let pending = store.load_open("tenant-a", 10).expect("actionable");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, InteractionStatus::Rejected);
    assert_eq!(pending[0].outcome_code.as_deref(), Some("scope_mismatch"));
}

#[test]
fn conflicting_second_outcome_fails_closed() {
    let (_store, lifecycle, interaction_id) = buffered();
    lifecycle
        .settled(
            "tenant-a",
            &interaction_id.to_canonical_string(),
            InteractionDeliveryOutcome::Accepted,
            timestamp(4_000),
        )
        .expect("accepted");
    let error = lifecycle
        .settled(
            "tenant-a",
            &interaction_id.to_canonical_string(),
            InteractionDeliveryOutcome::Rejected {
                reason_code: "scope_mismatch",
            },
            timestamp(5_000),
        )
        .expect_err("conflict");
    assert!(matches!(
        error,
        WorkerError::Conflict {
            code: "hitl_lifecycle_conflict"
        }
    ));
}
