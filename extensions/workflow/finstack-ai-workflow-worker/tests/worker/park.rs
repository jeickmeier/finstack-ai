use std::sync::Arc;

use finstack_ai_runtime::{ExternalClock, Model, WorkflowWait};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_worker::{MemoryWorkerStore, WakeIndexStore, WakeReason, park};

use crate::helpers::{
    attach_session, drive_to_active_model_request, env, memory_store, profile, retryable_failure,
    spawn_model_owner, stage, timestamp, wait_state,
};
use finstack_ai_kernel::{Duration as KernelDuration, RunPhase, Stage};
use finstack_ai_runtime::CommitCoordinator;

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
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        800,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| state.phase == Some(RunPhase::BeforeFinalize)).await;
    owner
        .handle()
        .submit(
            env(2_300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                finstack_ai_kernel::ReducerStageOutcome::Retry(
                    finstack_ai_kernel::RetryDirective::try_new(
                        finstack_ai_kernel::RetryClassification::Model,
                        KernelDuration::from_millis(10),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    wait_state(&store, |state| state.phase == Some(RunPhase::Sleeping)).await;
    drop(owner);

    let mut session = attach_session(store, Arc::clone(&model), clock, 800).await;
    let WorkflowWait::Timer { due_at, .. } = session.drive_until_wait().await.expect("timer")
    else {
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
