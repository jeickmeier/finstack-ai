//! tool-port contract Toolset port, validation, scheduler, ordering, and panic proofs.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    ActiveToolCallStatus, ComponentId, ContentBlock, EffectId, ExternalHandleRef, KernelState,
    RawJson, ReconciliationPolicy, RecordBody, RetrySafety, SessionTag, ToolCallId, ToolCallPlan,
    ValidatedToolCall,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::events::EventHubConfig;
use finstack_ai_runtime::ids::Clock;
use finstack_ai_runtime::ports::journal::{JournalStore, LoadRequest};
use finstack_ai_runtime::ports::model::{
    ApprovalGrantMode, Model, ModelStreamLimits, SideEffectClass,
};
use finstack_ai_runtime::ports::tool::{
    ResolvedToolCatalog, ToolDeferral, ToolReconcileResult, ToolResult, ToolStreamLimits,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunHandleError, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy,
    ToolTaskConfig,
};
use finstack_ai_runtime::testing::ManualDriveAction;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{FixedClock, ScriptedModel, ScriptedToolPlan, ScriptedToolset};

use super::*;

pub(crate) fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 128,
            records_per_session: 512,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    )
}

pub(crate) fn owner_run_config() -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: 8,
        event_hub: EventHubConfig {
            source_capacity: 16,
            max_subscribers: 8,
        },
        shutdown_deadline: StdDuration::from_millis(500),
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

pub(crate) fn owner_tool_config() -> ToolTaskConfig {
    ToolTaskConfig {
        job_capacity: 8,
        result_capacity: 8,
        global_max_concurrency: 2,
        stream_limits: ToolStreamLimits::default(),
    }
}

pub(crate) fn tool_result(value: i64) -> ToolResult {
    ToolResult {
        output: RawJson::parse(format!(r#"{{"ok":true,"value":{value}}}"#)).expect("result"),
        is_error: false,
    }
}

pub(crate) fn scripted_tool_deferral(handle: &str) -> ToolDeferral {
    ToolDeferral {
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tool.scripted").expect("component"),
            handle,
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
    }
}

pub(crate) fn at_most_once_spec() -> finstack_ai_runtime::ports::model::ToolSpec {
    let mut spec = tool_spec("echo");
    spec.retry_safety = RetrySafety::AtMostOnce;
    spec
}

pub(crate) fn non_idempotent_spec() -> finstack_ai_runtime::ports::model::ToolSpec {
    let mut spec = tool_spec("echo");
    spec.side_effect = SideEffectClass::NonIdempotentWrite;
    spec
}

pub(crate) type ResumePorts = (
    Arc<MemoryJournalStore>,
    Arc<ScriptedToolset>,
    Arc<dyn Model>,
    Arc<ResolvedToolCatalog>,
    Arc<[finstack_ai_runtime::ports::model::ToolSpec]>,
);

pub(crate) fn resume_ports(
    call_count: usize,
    plans: Vec<ScriptedToolPlan>,
    reconcile: Vec<ToolReconcileResult>,
    spec: finstack_ai_runtime::ports::model::ToolSpec,
) -> ResumePorts {
    let tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]> = Arc::from([spec]);
    let toolset =
        Arc::new(ScriptedToolset::new(Arc::clone(&tools), plans).with_reconcile_results(reconcile));
    let catalog = catalog(Arc::clone(&toolset), 2);
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![model_plan(call_count, "echo")],
    ));
    (memory_store(), toolset, model, catalog, tools)
}

pub(crate) fn spawn_tool_owner(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    clock_ms: i64,
    random: u64,
) -> impl Future<Output = Result<RunTaskOwner, RunHandleError>> {
    Box::pin(spawn_tool_owner_with_clock(
        coordinator,
        model,
        catalog,
        FixedClock::new(timestamp(clock_ms)),
        random,
    ))
}

/// Spawn a native model-and-tool owner using the supplied semantic clock.
///
/// # Errors
///
/// Returns [`RunHandleError`] when the runtime configuration, port bindings, or
/// startup reconciliation cannot initialize the owner.
pub(crate) fn spawn_tool_owner_with_clock<C>(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    clock: C,
    random: u64,
) -> impl Future<Output = Result<RunTaskOwner, RunHandleError>>
where
    C: Clock + Send + Sync + 'static,
{
    Box::pin(spawn_tool_owner_with_run_config(
        coordinator,
        model,
        catalog,
        clock,
        random,
        owner_run_config(),
    ))
}

/// Spawn a native model-and-tool owner using an explicit run configuration.
///
/// # Errors
///
/// Returns [`RunHandleError`] when the runtime configuration, port bindings, or
/// startup reconciliation cannot initialize the owner.
pub(crate) async fn spawn_tool_owner_with_run_config<C>(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    clock: C,
    random: u64,
    run_config: RunTaskConfig,
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
    Box::pin(RunTaskOwner::spawn_with_model_and_tools(
        coordinator,
        run_config,
        owner_model_config(),
        owner_tool_config(),
        model,
        locked_profile(),
        catalog,
        clock,
        CounterRandom(AtomicU64::new(random)),
    ))
    .await
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

pub(crate) async fn wait_gate(control: &finstack_ai_test::ScriptedToolsetControl, name: &str) {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        while control.entries(name) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("gate");
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

pub(crate) fn requested_execute(state: &KernelState) -> (EffectId, ToolCallId, ValidatedToolCall) {
    let batch = state.active_tool_batch().expect("batch");
    let call = batch
        .calls
        .iter()
        .find(|call| {
            matches!(
                call.status,
                ActiveToolCallStatus::Requested { deferred: None, .. }
            )
        })
        .expect("requested");
    let ToolCallPlan::Execute(validated) = &call.assigned.plan else {
        panic!("execute");
    };
    (
        call.assigned.effect_id,
        *validated.call.tool_call_id(),
        validated.clone(),
    )
}

pub(crate) fn tool_result_call_ids(state: &KernelState) -> Vec<ToolCallId> {
    state
        .messages()
        .iter()
        .filter_map(|message| match message.content() {
            [ContentBlock::ToolResult(result)] => Some(*result.tool_call_id()),
            _ => None,
        })
        .collect()
}

pub(crate) fn source_tool_call_ids(state: &KernelState) -> Vec<ToolCallId> {
    state
        .messages()
        .iter()
        .flat_map(|message| {
            message.content().iter().filter_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(*call.tool_call_id()),
                _ => None,
            })
        })
        .collect()
}

pub(crate) async fn crash_before_tool_dispatch(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]>,
) -> CommitCoordinator {
    Box::pin(crash_before_tool_dispatch_inner(
        store, model, catalog, tools,
    ))
    .await
}

pub(crate) async fn crash_before_tool_dispatch_inner(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]>,
) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(store.clone());
    let mut drive = coordinator.enable_manual_drive(1).expect("manual drive");
    let owner = spawn_tool_owner(coordinator, model, catalog, 2_000, 700)
        .await
        .expect("owner");
    let handle = owner.handle();
    let drive_store = store.clone();
    let driving = tokio::spawn(async move {
        drive_to_tools(&handle, &drive_store, tools).await;
    });
    let model_permit = tokio::time::timeout(StdDuration::from_secs(2), drive.next_effect())
        .await
        .expect("model paused")
        .expect("model permit");
    assert_eq!(model_permit.effect().action, ManualDriveAction::Execute);
    model_permit.continue_dispatch();
    let tool_permit = tokio::time::timeout(StdDuration::from_secs(2), drive.next_effect())
        .await
        .expect("tool paused")
        .expect("tool permit");
    assert_eq!(tool_permit.effect().action, ManualDriveAction::Execute);
    drop(owner);
    driving.abort();
    let _ = driving.await;
    drop(tool_permit);
    recover_session(&store).await
}
