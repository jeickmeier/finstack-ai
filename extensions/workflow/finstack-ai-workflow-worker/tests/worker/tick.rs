use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_runtime::{ExternalClock, Model};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_local::{
    CronFire, CronSchedule, CronScheduleStore, IntervalSchedule, MemoryCronStore,
};
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, MemoryWorkerStore, PortsFactory, RunStarter, StartedRun, WakeIndexStore,
    WorkerBuilder, WorkerError, park,
};

use crate::helpers::{
    completed_plan, memory_store, park_on_retry_timer, profile, retryable_failure, timestamp,
};

struct BindPorts {
    model: Arc<dyn Model>,
}

impl PortsFactory for BindPorts {
    fn bind(
        &self,
        session: finstack_ai_runtime::WorkflowSession,
    ) -> Result<finstack_ai_runtime::WorkflowSession, WorkerError> {
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
        let key: Arc<str> = Arc::from(idempotency_key);
        Box::pin(async move { Ok(StartedRun { session_id: key }) })
    }
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
    .build();

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

#[tokio::test]
async fn tick_resumes_a_due_timer_and_clears_its_wake_row() {
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
    park(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .register_ports("research", Arc::new(BindPorts { model }))
    .build();

    let before_due = Box::pin(worker.tick()).await.expect("before due");
    assert_eq!(before_due.sessions_resumed, 0);
    clock.jump(60_000).expect("past due");
    let resumed = Box::pin(worker.tick()).await.expect("resume");
    assert_eq!(resumed.sessions_resumed, 1);
    assert_eq!(resumed.failures, 0);
    assert!(
        store.load_tenant("tenant-a").expect("rows").is_empty(),
        "a resumed run leaves no stale wake row"
    );
}
