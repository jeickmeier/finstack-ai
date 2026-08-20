//! Integration tests for `CodexToolset` (`codex_start` / `codex_status` / `codex_cancel`).

use std::sync::Arc;

use finstack_ai_codex_child::{CodexChildInvoker, CodexExecConfig, CodexSandboxMode, CodexToolset};
use finstack_ai_kernel::{
    EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata, OperationLocator,
    PrincipalRef, RawJson, RunId, SessionId, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolFailurePolicy, ValidatedToolCall,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, RunCallContext, ToolCallContext, ToolStreamItem,
    Toolset,
};
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
    for _ in 0..200 {
        let status = call_tool(
            toolset,
            "codex_status",
            &format!(r#"{{"run_id":"{run_id}"}}"#),
        )
        .await;
        if status["status"] != "running" {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("codex run never settled");
}

#[tokio::test]
async fn start_then_status_reports_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = Arc::new(fake_invoker("success", dir.path()));
    let toolset = CodexToolset::try_new(Arc::clone(&invoker)).expect("toolset");
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
    let toolset =
        CodexToolset::try_new(Arc::new(fake_invoker("success", dir.path()))).expect("toolset");
    let output = call_tool(&toolset, "codex_status", r#"{"run_id":"not-a-run"}"#).await;
    assert_eq!(output["code"], "codex_child_not_found");
}

#[tokio::test]
async fn start_rejects_empty_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset =
        CodexToolset::try_new(Arc::new(fake_invoker("success", dir.path()))).expect("toolset");
    let output = call_tool(&toolset, "codex_start", r#"{"prompt":"   "}"#).await;
    assert_eq!(output["code"], "codex_invalid_arguments");
}

#[tokio::test]
async fn cancel_stops_a_hanging_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset =
        CodexToolset::try_new(Arc::new(fake_invoker("hang", dir.path()))).expect("toolset");
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
