use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_kernel::{
    AgentId, AllocatedIds, AppendBatchId, BudgetPropagation, CancellationPropagation,
    ChildPlacement, ChildRunLocator, DeadlinePropagation, Digest, EffectId, EffectOutputContract,
    EffectOutputKind, EventId, LaneId, Metadata, OperationLocator, PrincipalPropagation,
    PrincipalRef, RawJson, RecordId, RunAccepted, RunId, RunLimits, RunPropagationPolicy,
    RunRelation, RunSecurityContext, SessionId, Timestamp, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolFailurePolicy, TransitionEnv, ValidatedToolCall,
};
use finstack_ai_runtime::child::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AgentRef, ChildRunContext,
    ChildRunHandle, ChildRunPolicy, ChildRunRequest, ChildRunStarter, ChildRunStatus,
    child_relation_digest,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::{
    AuthorizationContext, CancellationSignal, RunCallContext, SideEffectClass,
};
use finstack_ai_runtime::ports::tool::{ToolStreamItem, Toolset};
use finstack_ai_runtime::session::{SessionCreateIds, SessionRuntime};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use futures_util::StreamExt;

use super::*;

struct RecordingInvoker {
    starts: AtomicUsize,
    cancels: AtomicUsize,
    statuses: AtomicUsize,
    error: Option<AgentInvokeError>,
    status: ChildRunStatus,
}

impl AgentInvoker for RecordingInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.error.clone() {
            return Box::pin(async move { Err(error) });
        }
        let relation_digest = match child_relation_digest(&context, &request) {
            Ok(digest) => digest,
            Err(error) => {
                return Box::pin(async move {
                    Err(AgentInvokeError::InvalidRequest {
                        message: Arc::from(error.to_string()),
                    })
                });
            }
        };
        let locator = request.locator().clone();
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }

    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let _ = locator;
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn status(
        &self,
        locator: &ChildRunLocator,
    ) -> PortFuture<Result<ChildRunStatus, AgentInvokeError>> {
        let _ = locator;
        self.statuses.fetch_add(1, Ordering::SeqCst);
        let status = self.status;
        Box::pin(async move { Ok(status) })
    }
}

fn allow_list() -> Arc<[AgentRef]> {
    Arc::from([AgentRef {
        id: AgentId::parse("finstack.agent.child").expect("agent"),
        bundle: None,
        spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
    }])
}

fn starter(invoker: Arc<dyn AgentInvoker>) -> Arc<ChildRunStarter> {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 256,
            snapshot_bytes: 64 * 1_024,
        })
        .expect("store"),
    );
    Arc::new(ChildRunStarter::new(
        store,
        ChildRunPolicy::Allow { max_depth: 1 },
        invoker,
    ))
}

async fn seeded_starter(invoker: Arc<dyn AgentInvoker>) -> Arc<ChildRunStarter> {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 256,
            snapshot_bytes: 64 * 1_024,
        })
        .expect("store"),
    );
    let locator = context().run.locator;
    let session = SessionRuntime::create(
        Arc::clone(&store),
        Arc::clone(&locator.tenant_scope),
        SessionCreateIds {
            session_id: locator.session_id,
            main_lane_id: locator.lane_id,
            session_created_record_id: RecordId::from_bytes([10; 16]),
            lane_created_record_id: RecordId::from_bytes([11; 16]),
            batch_id: AppendBatchId::from_bytes([12; 16]),
            now: Timestamp::from_unix_ms(1).expect("time"),
        },
    )
    .await
    .expect("session");
    session
        .try_acquire_run(locator.lane_id, locator.run_id)
        .expect("guard");
    let accepted = RunAccepted::try_new(
        locator.run_id,
        RunRelation::root(locator.run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            context().run.authorization.principal,
            "test",
            "test",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(b"{}"),
        None,
    )
    .expect("accepted");
    session
        .accept_run(
            locator.lane_id,
            accepted,
            TransitionEnv {
                now: Timestamp::from_unix_ms(2).expect("time"),
                ids: AllocatedIds::try_new(
                    vec![RecordId::from_bytes([13; 16])],
                    vec![EventId::from_bytes([14; 16])],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![AppendBatchId::from_bytes([15; 16])],
                    Vec::new(),
                )
                .expect("ids"),
            },
        )
        .await
        .expect("accept");
    Arc::new(ChildRunStarter::new(
        store,
        ChildRunPolicy::Allow { max_depth: 1 },
        invoker,
    ))
}

fn context() -> ToolCallContext {
    ToolCallContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: EffectId::from_bytes([4; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
            relation_depth: 0,
        },
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn call(toolset: &SubagentToolset, name: &str, arguments: &serde_json::Value) -> ValidatedToolCall {
    let tools = toolset.tools();
    let spec = tools
        .iter()
        .find(|tool| tool.model_name.as_ref() == name)
        .expect("tool");
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            context().tool_call_id,
            name,
            RawJson::parse(serde_json::to_vec(arguments).expect("arguments")).expect("raw"),
        )
        .expect("tool call"),
        tool_id: spec.id.clone(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: spec.retry_safety,
        deadline: None,
        execution: spec.execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

async fn invoke(
    toolset: &SubagentToolset,
    name: &str,
    arguments: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let mut stream = toolset
        .call(context(), call(toolset, name, &arguments))
        .await?;
    match stream.next().await.expect("item").expect("stream") {
        ToolStreamItem::Completed(result) => Ok(result),
        other => panic!("expected completion, got {other:?}"),
    }
}

fn start_args(agent_id: &str) -> serde_json::Value {
    serde_json::json!({
        "agent_id": agent_id,
        "input": "child work",
        "placement": "isolated_child_session",
    })
}

#[test]
fn subagent_toolset_exposes_exactly_three_tools() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let toolset = SubagentToolset::try_new(starter(invoker as Arc<dyn AgentInvoker>), allow_list())
        .expect("toolset");
    let tools = toolset.tools();
    let names: Vec<_> = tools.iter().map(|tool| tool.model_name.as_ref()).collect();
    assert_eq!(
        names,
        ["subagent_start", "subagent_status", "subagent_cancel"]
    );
    for spec in tools.iter() {
        assert_eq!(spec.side_effect, SideEffectClass::NonIdempotentWrite);
        assert_eq!(spec.retry_safety, RetrySafety::AtMostOnce);
        assert_eq!(spec.approval.requirement, ApprovalRequirement::Required);
    }
}

#[test]
fn try_new_rejects_empty_allow_list() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let error = SubagentToolset::try_new(starter(invoker as Arc<dyn AgentInvoker>), Arc::from([]))
        .err()
        .expect("empty");
    assert!(matches!(
        error,
        SubagentError::Configuration {
            reason: "empty_allow_list"
        }
    ));
}

#[tokio::test]
async fn unknown_agent_id_is_a_tool_result_not_a_run_abort() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let toolset = SubagentToolset::try_new(
        seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
        allow_list(),
    )
    .expect("toolset");
    let result = invoke(&toolset, START_NAME, start_args("finstack.agent.other"))
        .await
        .expect("tool result");
    assert!(result.is_error);
    let payload: serde_json::Value =
        serde_json::from_slice(result.output.as_bytes()).expect("json");
    assert_eq!(payload["code"], SUBAGENT_AGENT_NOT_ALLOWED);
    assert_eq!(invoker.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn policy_and_budget_failures_surface_as_tool_results() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: Some(AgentInvokeError::InvalidRequest {
            message: Arc::from("child run policy denies child invocation"),
        }),
        status: ChildRunStatus::Running,
    });
    let toolset = SubagentToolset::try_new(
        seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
        allow_list(),
    )
    .expect("toolset");
    let result = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("tool result");
    assert!(result.is_error);
    let payload: serde_json::Value =
        serde_json::from_slice(result.output.as_bytes()).expect("json");
    assert_eq!(payload["code"], AGENT_INVOKE_INVALID_ACCEPTANCE);
    assert_eq!(invoker.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn start_await_and_cancel_track_compatible_and_isolated_only() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let toolset = SubagentToolset::try_new(
        seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
        allow_list(),
    )
    .expect("toolset");
    let started = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("start");
    assert!(!started.is_error);
    let payload: serde_json::Value =
        serde_json::from_slice(started.output.as_bytes()).expect("json");
    let run_id = payload["run_id"].as_str().expect("run_id").to_string();
    let awaited = invoke(
        &toolset,
        STATUS_NAME,
        serde_json::json!({ "run_id": run_id }),
    )
    .await
    .expect("status");
    assert!(!awaited.is_error);
    let cancelled = invoke(
        &toolset,
        CANCEL_NAME,
        serde_json::json!({ "run_id": run_id }),
    )
    .await
    .expect("cancel");
    assert!(!cancelled.is_error);
    assert_eq!(invoker.cancels.load(Ordering::SeqCst), 1);
    let missing = invoke(
        &toolset,
        CANCEL_NAME,
        serde_json::json!({ "run_id": "00000000-0000-7000-8000-000000000099" }),
    )
    .await
    .expect("missing");
    assert!(missing.is_error);
}

#[tokio::test]
async fn remote_cancel_is_forwarded_to_the_invoker() {
    let child = StartedChild {
        handle: ChildRunHandle {
            locator: ChildRunLocator {
                operation: OperationLocator::try_new(
                    "tenant-a",
                    SessionId::from_bytes([8; 16]),
                    LaneId::from_bytes([9; 16]),
                    RunId::from_bytes([10; 16]),
                )
                .expect("locator"),
                remote: None,
            },
            relation_digest: Digest::raw_json(b"{}"),
        },
        placement: ChildPlacement::RemoteChildSession,
    };
    let mut children = BTreeMap::new();
    children.insert(
        ChildKey {
            tenant_scope: Arc::from("tenant-a"),
            owner_session_id: Arc::from(context().run.locator.session_id.to_string()),
            run_id: Arc::from("remote-1"),
        },
        child,
    );
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let call = {
        let toolset = SubagentToolset::try_new(
            seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
            allow_list(),
        )
        .expect("toolset");
        call(
            &toolset,
            CANCEL_NAME,
            &serde_json::json!({ "run_id": "remote-1" }),
        )
    };
    let cancel_starter = starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>);
    let outcome = cancel_child(&cancel_starter, &children, &context(), &call)
        .await
        .expect("cancel");
    assert!(!outcome.is_error);
    assert_eq!(invoker.cancels.load(Ordering::SeqCst), 1);
}

fn context_for(tenant: &str) -> ToolCallContext {
    let mut ctx = context();
    ctx.run.locator = OperationLocator::try_new(
        tenant,
        SessionId::from_bytes([11; 16]),
        LaneId::from_bytes([12; 16]),
        RunId::from_bytes([13; 16]),
    )
    .expect("locator");
    ctx.run.authorization.principal =
        PrincipalRef::try_new("issuer", "subject", Some(tenant)).expect("principal");
    ctx
}

async fn invoke_with(
    toolset: &SubagentToolset,
    ctx: ToolCallContext,
    name: &str,
    arguments: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let mut stream = toolset.call(ctx, call(toolset, name, &arguments)).await?;
    match stream.next().await.expect("item").expect("stream") {
        ToolStreamItem::Completed(result) => Ok(result),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[tokio::test]
async fn cross_tenant_status_and_cancel_are_not_found() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let toolset = SubagentToolset::try_new(
        seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
        allow_list(),
    )
    .expect("toolset");
    let started = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("start");
    let payload: serde_json::Value =
        serde_json::from_slice(started.output.as_bytes()).expect("json");
    let run_id = payload["run_id"].as_str().expect("run_id").to_string();
    let other = context_for("tenant-b");
    let status = invoke_with(
        &toolset,
        other.clone(),
        STATUS_NAME,
        serde_json::json!({ "run_id": run_id }),
    )
    .await
    .expect("status");
    assert!(status.is_error);
    let status_json: serde_json::Value =
        serde_json::from_slice(status.output.as_bytes()).expect("json");
    assert_eq!(status_json["code"], SUBAGENT_CHILD_NOT_FOUND);
    let cancelled = invoke_with(
        &toolset,
        other,
        CANCEL_NAME,
        serde_json::json!({ "run_id": run_id }),
    )
    .await
    .expect("cancel");
    assert!(cancelled.is_error);
    let cancel_json: serde_json::Value =
        serde_json::from_slice(cancelled.output.as_bytes()).expect("json");
    assert_eq!(cancel_json["code"], SUBAGENT_CHILD_NOT_FOUND);
    assert_eq!(invoker.cancels.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn status_without_run_id_is_invalid_arguments() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let toolset = SubagentToolset::try_new(
        seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
        allow_list(),
    )
    .expect("toolset");
    let _ = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("start");
    let missing = invoke(&toolset, STATUS_NAME, serde_json::json!({}))
        .await
        .expect("missing run_id");
    assert!(missing.is_error);
    let payload: serde_json::Value =
        serde_json::from_slice(missing.output.as_bytes()).expect("json");
    assert_eq!(payload["code"], SUBAGENT_INVALID_ARGUMENTS);
    assert!(
        !payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains(SessionId::from_bytes([1; 16]).to_string().as_str())
    );
}

fn result_payload(result: &ToolResult) -> serde_json::Value {
    serde_json::from_slice(result.output.as_bytes()).expect("result json")
}

#[tokio::test]
async fn tracking_limit_releases_its_slot_after_cancel() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    let toolset = SubagentToolset::try_with_max_tracked_children(
        seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
        allow_list(),
        1,
    )
    .expect("toolset");
    let first = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("first start");
    let run_id = result_payload(&first)["run_id"]
        .as_str()
        .expect("run id")
        .to_owned();

    let limited = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("limited start");
    assert!(limited.is_error);
    assert_eq!(result_payload(&limited)["code"], SUBAGENT_LIMIT_EXCEEDED);

    let cancelled = invoke(&toolset, CANCEL_NAME, serde_json::json!({"run_id": run_id}))
        .await
        .expect("cancel");
    assert!(!cancelled.is_error);
    let restarted = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("restart");
    assert!(!restarted.is_error);
}

#[tokio::test]
async fn terminal_status_reports_and_evicts_the_child() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Completed,
    });
    let toolset = SubagentToolset::try_with_max_tracked_children(
        seeded_starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>).await,
        allow_list(),
        1,
    )
    .expect("toolset");
    let started = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("start");
    let run_id = result_payload(&started)["run_id"]
        .as_str()
        .expect("run id")
        .to_owned();
    let status = invoke(&toolset, STATUS_NAME, serde_json::json!({"run_id": run_id}))
        .await
        .expect("status");
    assert_eq!(result_payload(&status)["status"], "completed");

    let restarted = invoke(&toolset, START_NAME, start_args("finstack.agent.child"))
        .await
        .expect("restart");
    assert!(!restarted.is_error);
}

#[test]
fn explicit_tracking_limit_is_bounded() {
    let invoker = Arc::new(RecordingInvoker {
        starts: AtomicUsize::new(0),
        cancels: AtomicUsize::new(0),
        statuses: AtomicUsize::new(0),
        error: None,
        status: ChildRunStatus::Running,
    });
    for invalid in [0, MAX_TRACKED_CHILDREN + 1] {
        let error = SubagentToolset::try_with_max_tracked_children(
            starter(Arc::clone(&invoker) as Arc<dyn AgentInvoker>),
            allow_list(),
            invalid,
        )
        .err()
        .expect("invalid bound");
        assert!(matches!(
            error,
            SubagentError::Configuration {
                reason: "invalid_max_tracked_children"
            }
        ));
    }
}
