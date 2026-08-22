use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{EffectId, RunPhase};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ids::ExternalClock;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::Model;
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_local::{
    CronFire, CronSchedule, CronScheduleStore, IntervalSchedule, MemoryCronStore,
};
use finstack_ai_workflow_worker::{
    FireStore, InboxKind, InboxRow, InboxStore, MemoryWorkerStore, PortsFactory, RunStarter,
    StartedRun, WakeIndexStore, WakeReason, WorkerBuilder, WorkerError, park_for_wake,
};

use crate::helpers::{
    completed_plan, completion_command, deferring_model, id, memory_store, park_on_deferred_effect,
    park_on_retry_timer, profile, retryable_failure, timestamp,
};

struct BindPorts {
    model: Arc<dyn Model>,
}

impl PortsFactory for BindPorts {
    fn bind(
        &self,
        session: finstack_ai_runtime::workflow::WorkflowSession,
    ) -> Result<finstack_ai_runtime::workflow::WorkflowSession, WorkerError> {
        Ok(session.with_ports(
            Arc::clone(&self.model),
            crate::helpers::locked_profile(),
            None,
        ))
    }
}

struct CountingStarter {
    calls: AtomicUsize,
}

impl RunStarter for CountingStarter {
    fn start<'a>(
        &'a self,
        _fire: &'a CronFire,
        idempotency_key: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<StartedRun, WorkerError>> + Send + 'a>>
    {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let _ = idempotency_key;
        Box::pin(async move {
            Ok(StartedRun {
                session_id: id(900),
            })
        })
    }
}

#[test]
fn builder_rejects_unsafe_runtime_limits() {
    let store = Arc::new(MemoryWorkerStore::new());
    let builder = WorkerBuilder::new(
        memory_store(),
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .batch_limit(0);
    let Err(error) = builder.build() else {
        panic!("zero batch size must fail closed");
    };
    assert_eq!(error.code(), "batch_limit_zero");

    let builder = WorkerBuilder::new(
        memory_store(),
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .lease_ttl_ms(1_000)
    .drive_timeout(Duration::from_secs(1));
    let Err(error) = builder.build() else {
        panic!("drive budget must stay below the lease ttl");
    };
    assert_eq!(error.code(), "drive_timeout_exceeds_lease");
}

#[tokio::test]
async fn tick_fires_due_cron_and_starts_runs_exactly_once() {
    let journal = memory_store();
    let cron = Arc::new(MemoryCronStore::new());
    let store = Arc::new(MemoryWorkerStore::new());
    cron.upsert(&CronSchedule {
        tenant_scope: Arc::from("tenant-a"),
        schedule_id: Arc::from("nightly"),
        expression: IntervalSchedule::parse("every 10ms").expect("expr"),
        origin: timestamp(2_000),
        next_fire_at: timestamp(2_010),
        last_fired_at: None,
        fire_count: 0,
    })
    .expect("schedule");
    let starter = Arc::new(CountingStarter {
        calls: AtomicUsize::new(0),
    });
    let clock = ExternalClock::new(timestamp(2_000));
    let worker = WorkerBuilder::new(
        journal,
        cron,
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .register_starter("nightly", Arc::clone(&starter) as Arc<dyn RunStarter>)
    .build()
    .expect("worker");

    let early = Box::pin(worker.tick()).await.expect("early tick");
    assert_eq!(early.cron_fires, 0);
    clock.set(timestamp(2_015));
    let due = Box::pin(worker.tick()).await.expect("due tick");
    assert_eq!(due.cron_fires, 1);
    assert_eq!(due.runs_started, 1);
    let repeat = Box::pin(worker.tick()).await.expect("repeat tick");
    assert_eq!(repeat.runs_started, 0, "fire is not re-started");
    assert_eq!(starter.calls.load(Ordering::Acquire), 1);
}

/// A due timer row is claimed, its committed timer is fired, and — because
/// the run then re-enters the facade-driven stage loop, which no worker can
/// advance — the row is preserved for a later tick instead of being dropped.
#[tokio::test]
async fn tick_fires_a_due_timer_and_keeps_the_row_when_no_new_wait() {
    let journal = memory_store();
    // Model: first request fails retryably (parks a retry timer), the
    // retried request completes.
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![
            ScriptedModelPlan {
                actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
            },
            completed_plan("done"),
        ],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let mut session = Box::pin(park_on_retry_timer(&journal, &model, &clock, 800)).await;
    let store = Arc::new(MemoryWorkerStore::new());
    park_for_wake(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    let recover_from = Arc::clone(&journal) as Arc<dyn JournalStore>;
    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_millis(500))
    .register_ports("research", Arc::new(BindPorts { model }))
    .build()
    .expect("worker");

    let before_due = Box::pin(worker.tick()).await.expect("before due");
    assert_eq!(before_due.sessions_resumed, 0);
    assert_eq!(before_due.failures, 0);
    let asleep = CommitCoordinator::recover(Arc::clone(&recover_from), id(1))
        .await
        .expect("recover");
    assert_eq!(
        asleep.state().phase(),
        Some(RunPhase::Sleeping),
        "an undue timer is left alone"
    );

    clock.jump(60_000).expect("past due");
    let resumed = Box::pin(worker.tick()).await.expect("resume");

    // The worker respawned the owner, which fired the committed timer: the
    // run is awake and its retry is settled.
    let awake = CommitCoordinator::recover(recover_from, id(1))
        .await
        .expect("recover");
    assert!(
        awake.state().retry().pending.is_none(),
        "the due timer fired and cleared the pending retry"
    );
    assert_ne!(awake.state().phase(), Some(RunPhase::Sleeping));

    // The woken run is mid-flight in the stage loop with no classifiable
    // wait, so this worker cannot park it. The row must survive, unleased
    // and backed off, rather than leaving an orphaned run behind.
    assert_eq!(resumed.sessions_resumed, 0);
    assert_eq!(resumed.failures, 1);
    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1, "a run the worker cannot park keeps its row");
    assert_eq!(rows[0].attempts, 1);
    assert_eq!(rows[0].leased_by, None);
    assert_eq!(rows[0].wake_at, Some(timestamp(63_000)), "1s backoff");
}

/// A wake row that is due before the committed timer is takes the fast path:
/// the worker re-parks on the journal's own due time instead of spending the
/// drive budget on a timer that cannot fire yet.
#[tokio::test]
async fn tick_reparks_a_row_that_is_due_before_its_committed_timer() {
    let journal = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let mut session = Box::pin(park_on_retry_timer(&journal, &model, &clock, 800)).await;
    let store = Arc::new(MemoryWorkerStore::new());
    park_for_wake(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    // The committed retry timer is due at 2_310. Rewrite the hint row as if
    // it had been indexed early, then tick between the two instants.
    let mut early = store.load_tenant("tenant-a").expect("rows").remove(0);
    assert_eq!(early.wake_at, Some(timestamp(2_310)));
    early.wake_at = Some(timestamp(2_100));
    store.upsert(&early).expect("early row");
    clock.set(timestamp(2_200));

    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    // Far more than the 5s outer timeout below, so spending any real part of
    // the budget fails the test — but still under the 30s default lease TTL,
    // which `build()` asserts.
    .drive_timeout(Duration::from_secs(20))
    .register_ports("research", Arc::new(BindPorts { model }))
    .build()
    .expect("worker");

    let reparked = tokio::time::timeout(Duration::from_secs(5), Box::pin(worker.tick()))
        .await
        .expect("the fast path does not spend the drive budget")
        .expect("tick");
    assert_eq!(reparked.sessions_resumed, 1);
    assert_eq!(reparked.sessions_reparked, 1);
    assert_eq!(reparked.failures, 0);
    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].wake_at,
        Some(timestamp(2_310)),
        "the row is corrected to the journal's due time"
    );
    assert_eq!(rows[0].attempts, 0);
    assert_eq!(rows[0].leased_by, None);
}

/// A response the runtime rejects durably is retained as a dead letter and
/// removed from the retry path, while the original wake remains available
/// for a corrected command.
#[tokio::test]
async fn tick_dead_letters_a_durable_external_ingress_rejection() {
    let journal = memory_store();
    let model = deferring_model();
    let clock = ExternalClock::new(timestamp(2_000));
    let mut session = Box::pin(park_on_deferred_effect(&journal, &model, &clock, 740)).await;
    let store = Arc::new(MemoryWorkerStore::new());
    park_for_wake(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    let row = store.load_tenant("tenant-a").expect("rows").remove(0);
    assert_eq!(row.reason, WakeReason::Deferred);
    let effect_id = EffectId::parse(row.pending_id.as_ref()).expect("effect id");
    let payload = serde_json::to_vec(&completion_command(effect_id)).expect("payload");
    store
        .insert(
            &InboxRow::try_new(
                Arc::clone(&row.tenant_scope),
                row.session_id,
                Arc::clone(&row.pending_id),
                InboxKind::External,
                Arc::from(payload.into_boxed_slice()),
                timestamp(2_000),
            )
            .expect("row"),
        )
        .expect("inbox");

    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_millis(500))
    .register_ports("research", Arc::new(BindPorts { model }))
    .build()
    .expect("worker");

    // The runtime refuses this completion durably. It must leave the active
    // key, retain operator evidence, and not burn retry attempts forever.
    let first = Box::pin(worker.tick()).await.expect("first tick");
    assert_eq!(first.failures, 0, "report: {first:?}");
    assert_eq!(first.responses_rejected, 1);
    assert_eq!(first.sessions_resumed, 0);
    assert_eq!(
        store.load_batch(10).expect("inbox").len(),
        0,
        "a durable rejection leaves the active inbox"
    );
    assert_eq!(store.load_dead_letters(10).expect("dead letters").len(), 1);

    // With no active response, later ticks leave the journal wait available
    // for a corrected command and do not retry the rejected bytes.
    clock.jump(500).expect("inside backoff");
    let inside = Box::pin(worker.tick()).await.expect("next tick");
    assert_eq!(inside.failures, 0);
    assert_eq!(inside.sessions_resumed, 0);
    let sleeping = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(sleeping.len(), 1);
    assert_eq!(sleeping[0].attempts, 0);

    clock.jump(1_500).expect("past backoff");
    let second = Box::pin(worker.tick()).await.expect("second tick");
    assert_eq!(second.failures, 0);
    assert_eq!(second.responses_rejected, 0);
    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attempts, 0);
}

/// `spawn` runs the tick loop on a real interval and `shutdown` returns once
/// the current tick has finished, without hanging.
#[tokio::test]
async fn spawn_ticks_and_shuts_down_cleanly() {
    let journal = memory_store();
    let cron = Arc::new(MemoryCronStore::new());
    let store = Arc::new(MemoryWorkerStore::new());
    let worker = Arc::new(
        WorkerBuilder::new(
            journal,
            cron,
            Arc::clone(&store) as Arc<dyn WakeIndexStore>,
            Arc::clone(&store) as Arc<dyn FireStore>,
            Arc::clone(&store) as Arc<dyn InboxStore>,
        )
        .build()
        .expect("worker"),
    );
    let handle = Arc::clone(&worker).spawn(Duration::from_millis(5));
    tokio::time::sleep(Duration::from_millis(30)).await;
    handle.shutdown().await;
}
