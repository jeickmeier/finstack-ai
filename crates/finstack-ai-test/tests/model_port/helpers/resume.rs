//! model-port contract provider-neutral Model port and runtime acceptance proofs.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AllocatedIds, CancellationRequestTag, ComponentId, Duration as KernelDuration, ErrorCategory,
    ExternalHandleRef, Metadata, RawJson, ReconciliationPolicy, ReducerStageOutcome,
    RetryClassification, RetryDirective, RetrySafety, RunPhase, SessionTag, Stage, TransitionEnv,
};
use finstack_ai_kernel::{KernelState, RecordBody};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::events::EventHubConfig;
use finstack_ai_runtime::ids::Clock;
use finstack_ai_runtime::ports::journal::{JournalStore, LoadRequest};
use finstack_ai_runtime::ports::model::{
    ApprovalGrantMode, Model, ModelDeferral, ModelError, ModelStreamItem, ModelStreamLimits,
    TextDelta,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunHandleError, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy,
};
use finstack_ai_runtime::testing::ManualDriveAction;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{FixedClock, ScriptedModelAction, ScriptedModelPlan};

use super::*;

pub(crate) fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 32,
            records_per_session: 64,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    )
}

pub(crate) fn owner_run_config() -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: 4,
        event_hub: EventHubConfig {
            source_capacity: 16,
            max_subscribers: 8,
        },
        shutdown_deadline: StdDuration::from_millis(250),
        approval_grant: ApprovalGrantMode::PerCall,
    }
}

pub(crate) fn owner_model_config() -> ModelTaskConfig {
    ModelTaskConfig {
        job_capacity: 2,
        result_capacity: 2,
        stream_limits: ModelStreamLimits::default(),
        same_identity_retry: SameIdentityRetryPolicy::default(),
    }
}

pub(crate) async fn spawn_model_owner(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    clock_ms: i64,
    random: u64,
) -> Result<RunTaskOwner, RunHandleError> {
    Box::pin(spawn_model_owner_with_clock(
        coordinator,
        model,
        FixedClock::new(timestamp(clock_ms)),
        random,
    ))
    .await
}

pub(crate) async fn spawn_model_owner_with_clock<C>(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    clock: C,
    random: u64,
) -> Result<RunTaskOwner, RunHandleError>
where
    C: Clock + Send + Sync + 'static,
{
    let model = Arc::new(
        finstack_ai_runtime::ports::model::ReadyModel::prepare(model)
            .await
            .map_err(|error| RunHandleError::Model {
                code: Arc::from(error.code().as_str()),
            })?,
    );
    Box::pin(RunTaskOwner::spawn_with_model(
        coordinator,
        owner_run_config(),
        owner_model_config(),
        model,
        locked_profile(),
        clock,
        CounterRandom(AtomicU64::new(random)),
    ))
    .await
}

pub(crate) fn completed_plan(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(text)))),
        ],
    }
}

pub(crate) fn deferred_plan(handle: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
            scripted_deferral(handle),
        )))],
    }
}

pub(crate) fn scripted_deferral(handle: &str) -> ModelDeferral {
    ModelDeferral {
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.model.scripted").expect("component"),
            handle,
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
    }
}

pub(crate) async fn recover_session(store: &Arc<MemoryJournalStore>) -> CommitCoordinator {
    CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover")
}

pub(crate) async fn wait_state(
    store: &Arc<MemoryJournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let recovered = recover_session(store).await;
            if predicate(recovered.state()) {
                return recovered;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("state wait")
}

pub(crate) async fn crash_before_dispatch(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    retry_safety: RetrySafety,
) -> CommitCoordinator {
    Box::pin(crash_before_dispatch_inner(store, model, retry_safety)).await
}

pub(crate) async fn crash_before_dispatch_inner(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    retry_safety: RetrySafety,
) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(store.clone());
    let mut drive = coordinator.enable_manual_drive(1).expect("manual drive");
    let owner = spawn_model_owner(coordinator, model, 2_000, 700)
        .await
        .expect("owner");
    let handle = owner.handle();
    let driving = tokio::spawn(async move {
        drive_to_active_model_request_with(&handle, retry_safety).await;
    });
    let permit = tokio::time::timeout(StdDuration::from_secs(1), drive.next_effect())
        .await
        .expect("paused")
        .expect("permit");
    assert_eq!(permit.effect().action, ManualDriveAction::Execute);
    drop(owner);
    driving.abort();
    let _ = driving.await;
    drop(permit);
    recover_session(&store).await
}

pub(crate) async fn journal_has_rejection(store: &Arc<MemoryJournalStore>) -> bool {
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    loaded.committed_batches.iter().any(|batch| {
        batch
            .records
            .iter()
            .any(|record| matches!(record.body(), RecordBody::ExternalCommandRejected(_)))
    })
}

pub(crate) fn retryable_failure() -> ModelError {
    ModelError::try_new(
        "temporary_model_failure",
        ErrorCategory::Model,
        true,
        "temporary model failure",
        Metadata::empty(),
    )
    .expect("retryable error")
}

pub(crate) fn cancel_env(now: i64, record: u64, append_batch: u64, request: u64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id(record)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(append_batch)],
            vec![id::<CancellationRequestTag>(request)],
        )
        .expect("cancel ids"),
    }
}

pub(crate) async fn park_on_sleeping(
    store: &Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    clock_ms: i64,
    random: u64,
    backoff_ms: u64,
) -> RunTaskOwner {
    Box::pin(park_on_sleeping_inner(
        store, model, clock_ms, random, backoff_ms,
    ))
    .await
}

pub(crate) async fn park_on_sleeping_inner(
    store: &Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    clock_ms: i64,
    random: u64,
    backoff_ms: u64,
) -> RunTaskOwner {
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model,
        clock_ms,
        random,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(store, |state| {
        state.phase() == Some(RunPhase::BeforeFinalize)
    })
    .await;
    owner
        .handle()
        .submit(
            env(clock_ms + 300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                ReducerStageOutcome::Retry(
                    RetryDirective::try_new(
                        RetryClassification::Model,
                        KernelDuration::from_millis(backoff_ms),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    wait_state(store, |state| state.phase() == Some(RunPhase::Sleeping)).await;
    owner
}
