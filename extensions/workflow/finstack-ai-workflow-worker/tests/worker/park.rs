use std::sync::Arc;

use finstack_ai_runtime::{ExternalClock, Model, WorkflowWait, classify_wait};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_worker::{MemoryWorkerStore, WakeIndexStore, WakeReason, park};

use crate::helpers::{memory_store, park_on_retry_timer, profile, retryable_failure, timestamp};

#[tokio::test]
async fn park_indexes_a_timer_wait_and_drops_the_owner() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    // Build a run parked on a retry timer — identical setup to
    // timer_survives_worker_restart in finstack-ai-workflow-local
    // (tests/local_workflow/restart.rs:1-41): spawn, drive to the model
    // request, submit the Retry directive, wait for Sleeping, drop owner.
    let mut session = Box::pin(park_on_retry_timer(&store, &model, &clock, 800)).await;
    let Some(WorkflowWait::Timer { due_at, .. }) = classify_wait(session.last_state()) else {
        panic!("expected timer wait");
    };
    let wake = MemoryWorkerStore::new();
    let checkpoint = park(&mut session, &wake, "research").expect("park");
    assert!(!session.owner_is_live());
    let rows = wake.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason, WakeReason::Timer);
    assert_eq!(rows[0].wake_at, Some(due_at));
    assert_eq!(rows[0].session_id, checkpoint.session_id);
    assert_eq!(rows[0].workflow_kind.as_ref(), "research");
}
