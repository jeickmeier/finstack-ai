//! Integration tests for `CodexToolset` (`codex_start` / `codex_status` / `codex_cancel`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use finstack_ai_codex_child::{CodexChildInvoker, CodexExecConfig, CodexSandboxMode, CodexToolset};
use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, BudgetPropagation, CancellationPropagation, DeadlinePropagation,
    Digest, EffectId, EffectOutputContract, EffectOutputKind, EventId, LaneId, Metadata,
    OperationLocator, PrincipalPropagation, PrincipalRef, RawJson, RecordId, RunAccepted, RunId,
    RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, SessionId, Timestamp,
    ToolBatchId, ToolCallBlock, ToolCallId, ToolFailurePolicy, TransitionEnv, ValidatedToolCall,
};
use finstack_ai_runtime::{
    AgentInvoker, AuthorizationContext, CancellationSignal, ChildRunPolicy, ChildRunStarter,
    JournalStore, RunCallContext, SessionCreateIds, SessionRuntime, ToolCallContext,
    ToolStreamItem, Toolset,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use futures_util::StreamExt;

fn fake_invoker(mode: &str, workspace: &std::path::Path) -> CodexChildInvoker {
    CodexChildInvoker::try_new(CodexExecConfig {
        binary: std::path::PathBuf::from(env!("CARGO_BIN_EXE_codex_fake")),
        workspace_root: workspace.to_path_buf(),
        sandbox: CodexSandboxMode::WorkspaceWrite,
        skip_git_repo_check: true,
        extra_args: vec!["--fake-mode".to_string(), mode.to_string()],
    })
    .expect("invoker")
}

async fn test_toolset(mode: &str, workspace: &std::path::Path) -> CodexToolset {
    let invoker = Arc::new(fake_invoker(mode, workspace));
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
    let starter = Arc::new(ChildRunStarter::new(
        store,
        ChildRunPolicy::Allow { max_depth: 1 },
        Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
    ));
    CodexToolset::try_new(starter, invoker).expect("toolset")
}

fn context() -> ToolCallContext {
    ToolCallContext {
        run: RunCallContext {
            relation_depth: 0,
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
        },
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn call(toolset: &CodexToolset, name: &str, arguments_json: &str) -> ValidatedToolCall {
    let tools = toolset.tools();
    let spec = tools
        .iter()
        .find(|tool| tool.model_name.as_ref() == name)
        .expect("tool");
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            context().tool_call_id,
            name,
            RawJson::parse(arguments_json.as_bytes()).expect("raw"),
        )
        .expect("tool call"),
        tool_id: spec.id.clone(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: finstack_ai_kernel::Digest::raw_json(b"{}"),
        },
        retry_safety: spec.retry_safety,
        deadline: None,
        execution: spec.execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

async fn call_tool(toolset: &CodexToolset, name: &str, arguments_json: &str) -> serde_json::Value {
    let mut stream = toolset
        .call(context(), call(toolset, name, arguments_json))
        .await
        .expect("call accepted");
    match stream.next().await.expect("item").expect("stream item") {
        ToolStreamItem::Completed(result) => {
            serde_json::from_slice(result.output.as_bytes()).expect("json output")
        }
        other => panic!("expected completion, got {other:?}"),
    }
}

async fn poll_status(toolset: &CodexToolset, run_id: &str) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let status = call_tool(
            toolset,
            "codex_status",
            &format!(r#"{{"run_id":"{run_id}"}}"#),
        )
        .await;
        if status["status"] != "running" {
            return status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "codex run never settled"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn start_then_status_reports_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset = test_toolset("success", dir.path()).await;
    let output = call_tool(
        &toolset,
        "codex_start",
        r#"{"prompt":"fix the failing test"}"#,
    )
    .await;
    assert_eq!(output["status"], "accepted");
    let run_id = output["run_id"].as_str().expect("run_id").to_string();

    let settled = poll_status(&toolset, &run_id).await;
    assert_eq!(settled["status"], "completed");
    assert_eq!(settled["thread_id"], "thread-fake-1");
    assert_eq!(settled["last_message"], "fake done");
    assert_eq!(settled["usage"]["output_tokens"], 5);
}

#[tokio::test]
async fn status_of_unknown_run_is_child_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset = test_toolset("success", dir.path()).await;
    let output = call_tool(&toolset, "codex_status", r#"{"run_id":"not-a-run"}"#).await;
    assert_eq!(output["code"], "codex_child_not_found");
}

#[tokio::test]
async fn start_rejects_empty_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset = test_toolset("success", dir.path()).await;
    let output = call_tool(&toolset, "codex_start", r#"{"prompt":"   "}"#).await;
    assert_eq!(output["code"], "codex_invalid_arguments");
}

#[tokio::test]
async fn cancel_stops_a_hanging_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset = test_toolset("hang", dir.path()).await;
    let output = call_tool(&toolset, "codex_start", r#"{"prompt":"never finish"}"#).await;
    let run_id = output["run_id"].as_str().expect("run_id").to_string();
    let cancel = call_tool(
        &toolset,
        "codex_cancel",
        &format!(r#"{{"run_id":"{run_id}"}}"#),
    )
    .await;
    assert_eq!(cancel["cancelled"], true);
    let settled = poll_status(&toolset, &run_id).await;
    assert_eq!(settled["status"], "cancelled");
}
