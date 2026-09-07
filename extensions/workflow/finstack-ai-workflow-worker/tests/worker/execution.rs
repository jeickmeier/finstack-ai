//! The host callback advances stages while the worker owns lease and shutdown.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_runtime::ids::ExternalClock;
use finstack_ai_runtime::ports::model::Model;
use finstack_ai_runtime::run::RunHandle;
use finstack_ai_runtime::workflow::{WorkflowSession, WorkflowWait, classify_wait};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, MemoryWorkerStore, PortsFactory, WakeIndexStore, WorkerBuilder,
    WorkerError, WorkflowExecution, park_for_wake,
};

use crate::helpers::{
    locked_profile, memory_store, park_on_retry_timer, profile, retryable_failure, timestamp,
};

struct Host {
    model: Arc<dyn Model>,
    store: Arc<MemoryWorkerStore>,
    lose_lease: bool,
    handle: Mutex<Option<RunHandle>>,
}

impl PortsFactory for Host {
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError> {
        Ok(session.with_ports(Arc::clone(&self.model), locked_profile(), None))
    }
}

impl WorkflowExecution for Host {
    fn advance<'a>(
        &'a self,
        session: &'a mut WorkflowSession,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowWait, WorkerError>> + Send + 'a>> {
        Box::pin(async move {
            *self.handle.lock().expect("handle") = session.run_handle();
            if self.lose_lease {
                let locator = session.locator();
                assert!(self.store.release(
                    &locator.tenant_scope,
                    locator.session_id,
                    "worker-1"
                )?);
                std::future::pending().await
            } else {
                Ok(classify_wait(session.last_state()).expect("future timer"))
            }
        })
    }
}

async fn exercise(lose_lease: bool) {
    let journal = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let mut session = Box::pin(park_on_retry_timer(&journal, &model, &clock, 901)).await;
    let store = Arc::new(MemoryWorkerStore::new());
    park_for_wake(&mut session, store.as_ref(), "host").expect("park");
    let mut row = store.load_tenant("tenant-a").expect("rows").remove(0);
    // Claim the hint before its authoritative future timer, so the callback
    // controls the test without dispatching another external model request.
    row.wake_at = Some(timestamp(2_001));
    store.upsert(&row).expect("hint");
    clock.set(timestamp(2_002));
    let host = Arc::new(Host {
        model,
        store: Arc::clone(&store),
        lose_lease,
        handle: Mutex::new(None),
    });
    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock)
    .drive_timeout(Duration::from_secs(1))
    .register_ports("host", Arc::clone(&host) as Arc<dyn PortsFactory>)
    .register_execution("host", Arc::clone(&host) as Arc<dyn WorkflowExecution>)
    .build()
    .expect("worker");
    let report = Box::pin(worker.tick()).await.expect("tick");
    assert_eq!(report.failures, usize::from(lose_lease));
    assert_eq!(report.sessions_reparked, usize::from(!lose_lease));
    let handle = host
        .handle
        .lock()
        .expect("handle")
        .clone()
        .expect("callback ran");
    assert!(
        handle.shutdown_report().is_some(),
        "tick must join the owner on every exit"
    );
    assert!(
        handle.live_state().terminal.is_none(),
        "stopping local work does not cancel the run"
    );
    assert_eq!(store.load_tenant("tenant-a").expect("rows").len(), 1);
}

#[tokio::test]
async fn callback_parks_and_joins_owner() {
    exercise(false).await;
}

#[tokio::test]
async fn lost_lease_drops_callback_and_joins_owner_before_returning() {
    exercise(true).await;
}
