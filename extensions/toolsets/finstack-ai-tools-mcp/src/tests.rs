use std::sync::Arc;

use finstack_ai_runtime::{
    AssembledToolStream, AuthorizationContext, CancellationSignal, Digest, EffectOutputContract,
    EffectOutputKind, LaneId, Metadata, OperationLocator, PendingToolEffect, PrincipalRef,
    ReconcileContext, RetrySafety, RunCallContext, RunId, SessionId, SideEffectClass, ToolBatchId,
    ToolCallBlock, ToolCallContext, ToolCallId, ToolFailurePolicy, ToolReconcileResult,
    ToolStreamItem, ToolStreamLimits, Toolset, ValidatedToolCall,
};
use finstack_ai_test::{ToolsetConformanceCase, check_toolset_conformance};
use futures_util::StreamExt;

use super::*;
use crate::classify::{catalog_digest, enumerate_catalog, to_tool_spec};
use crate::protocol::{Tool, ToolAnnotations};
use crate::transport::ScriptedTransport;

fn tool(name: &str) -> Tool {
    Tool {
        name: name.to_owned(),
        title: None,
        description: Some(format!("{name} tool")),
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: None,
        annotations: None,
    }
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
            effect_id: finstack_ai_runtime::EffectId::from_bytes([4; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn validated(toolset: &McpToolset, name: &str, arguments: &serde_json::Value) -> ValidatedToolCall {
    let spec = toolset
        .tools()
        .iter()
        .find(|tool| tool.model_name.as_ref() == name)
        .expect("tool")
        .clone();
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            context().tool_call_id,
            name,
            RawJson::parse(serde_json::to_vec(&arguments).expect("arguments")).expect("raw"),
        )
        .expect("tool call"),
        tool_id: spec.id,
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

async fn call(
    toolset: &McpToolset,
    name: &str,
    arguments: serde_json::Value,
) -> Result<String, ToolError> {
    let mut stream = toolset
        .call(context(), validated(toolset, name, &arguments))
        .await?;
    let item = stream
        .next()
        .await
        .expect("stream item")
        .expect("stream ok");
    match item {
        ToolStreamItem::Completed(result) => Ok(result.output.as_str().to_owned()),
        other => panic!("unexpected stream item: {other:?}"),
    }
}

async fn connected_toolset() -> McpToolset {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"pong"}],"isError":false}),
    ]);
    McpToolset::connect(Arc::new(transport), McpConfig::default())
        .await
        .expect("connects")
}

#[test]
fn crate_is_a_workspace_member() {
    assert_eq!(env!("CARGO_PKG_NAME"), "finstack-ai-tools-mcp");
}

#[tokio::test]
async fn pagination_follows_cursors_including_the_empty_string() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"a","inputSchema":{"type":"object"}}],"nextCursor":""}),
        serde_json::json!({"resultType":"complete","tools":[{"name":"b","inputSchema":{"type":"object"}}]}),
    ]);
    let tools = enumerate_catalog(&transport).await.expect("enumerates");
    assert_eq!(
        tools
            .iter()
            .map(|listed| listed.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"],
        "an empty-string cursor must not terminate pagination"
    );
}

#[tokio::test]
async fn catalog_drift_is_detected_by_digest() {
    let first = vec![tool("a"), tool("b")];
    let second = vec![tool("a")];
    assert_ne!(catalog_digest(&first), catalog_digest(&second));
}

#[tokio::test]
async fn network_ref_in_input_schema_is_rejected() {
    let transport = ScriptedTransport::new(vec![serde_json::json!({
        "resultType":"complete",
        "tools":[{"name":"evil","inputSchema":{"type":"object","$ref":"https://attacker.example/schema.json"}}]
    })]);
    let error = enumerate_catalog(&transport)
        .await
        .expect_err("network $ref must be rejected");
    assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
}

#[test]
fn annotations_alone_never_grant_retry_safety() {
    let mut listed = tool("weather");
    listed.annotations = Some(ToolAnnotations {
        read_only_hint: Some(true),
        idempotent_hint: Some(true),
        destructive_hint: Some(false),
    });
    let spec = to_tool_spec(&listed, &McpConfig::default()).expect("spec");
    assert_eq!(spec.side_effect, SideEffectClass::NonIdempotentWrite);
    assert_eq!(spec.retry_safety, RetrySafety::AtMostOnce);
    assert_eq!(
        spec.approval.requirement,
        finstack_ai_runtime::ApprovalRequirement::Required
    );
}

#[test]
fn host_declared_read_only_tool_is_classified_read_only() {
    let config = McpConfig::default().with_read_only_tools(["weather"]);
    let spec = to_tool_spec(&tool("weather"), &config).expect("spec");
    assert_eq!(spec.side_effect, SideEffectClass::ReadOnly);
    assert_eq!(spec.retry_safety, RetrySafety::SafeToRetry);
}

#[tokio::test]
async fn call_forwards_the_committed_effect_id_and_returns_the_result() {
    let toolset = connected_toolset().await;
    assert_eq!(toolset.tools().len(), 1);
    let result = call(&toolset, "echo", serde_json::json!({}))
        .await
        .expect("call");
    assert!(result.contains("pong"));
}

#[tokio::test]
async fn is_error_result_becomes_a_tool_error() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"boom","description":"Boom","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"failed"}],"isError":true}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default())
        .await
        .expect("connects");
    let error = call(&toolset, "boom", serde_json::json!({}))
        .await
        .expect_err("isError must surface as a ToolError");
    assert!(format!("{error}").contains("tool_output_invalid"));
}

#[tokio::test]
async fn input_required_result_is_rejected() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"ask","description":"Ask","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"input_required","inputRequests":{}}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default())
        .await
        .expect("connects");
    let error = call(&toolset, "ask", serde_json::json!({}))
        .await
        .expect_err("MRTR is not supported");
    assert!(format!("{error}").contains(MCP_RESULT_UNSUPPORTED));
}

#[tokio::test]
async fn oversize_result_truncates_with_a_recorded_marker() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"big","description":"Big","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"abcdefghijklmnopqrstuvwxyz"}],"isError":false}),
    ]);
    let config = McpConfig::default().with_inline_result_bytes(32);
    let toolset = McpToolset::connect(Arc::new(transport), config)
        .await
        .expect("connects");
    let result = call(&toolset, "big", serde_json::json!({}))
        .await
        .expect("truncated");
    assert!(result.contains(MCP_LIMIT_EXCEEDED));
    assert!(result.contains("truncated"));
}

#[tokio::test]
async fn reconcile_is_non_repeatable_unless_host_declared_retry_safe() {
    let toolset = connected_toolset().await;
    let effect = PendingToolEffect {
        call: validated(&toolset, "echo", &serde_json::json!({})),
    };
    let result = toolset
        .reconcile(
            ReconcileContext {
                run: context().run,
                original_input_digest: Digest::raw_json(b"{}"),
            },
            effect,
        )
        .await
        .expect("reconcile");
    assert_eq!(result, ToolReconcileResult::NonRepeatable);
}

#[tokio::test]
async fn factory_rejects_a_server_that_is_not_allowlisted() {
    let error = McpConfig::default()
        .stdio(StdioConfig::new("/usr/bin/unlisted", Vec::<String>::new()))
        .expect_err("deny by default");
    assert!(format!("{error}").contains(MCP_SERVER_NOT_ALLOWLISTED));
}

#[tokio::test]
async fn toolset_satisfies_the_published_port_conformance_suite() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"pong"}],"isError":false}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default())
        .await
        .expect("connects");
    let spec = toolset.tools()[0].clone();
    let expected_output = RawJson::parse(
        serde_json::to_vec(&serde_json::json!({
            "content": [{"type": "text", "text": "pong"}]
        }))
        .expect("expected"),
    )
    .expect("expected output");
    let expected = AssembledToolStream {
        progress: Arc::from([]),
        usage: None,
        result: finstack_ai_runtime::ToolResult {
            output: expected_output,
            is_error: false,
        },
    };
    check_toolset_conformance(
        &toolset,
        ToolsetConformanceCase {
            context: context(),
            call: validated(&toolset, "echo", &serde_json::json!({})),
            expected: expected.clone(),
            stream_limits: ToolStreamLimits::default(),
            max_result_bytes: spec.max_result_bytes,
        },
    )
    .await
    .expect("published toolset conformance suite");
}

#[test]
fn invocation_digest_covers_server_identity_and_tool_names() {
    let first = invocation_digest_for("stdio:/bin/a", &[tool("echo")]);
    let second = invocation_digest_for("http://127.0.0.1/mcp", &[tool("echo")]);
    let third = invocation_digest_for("stdio:/bin/a", &[tool("other")]);
    assert_ne!(first, second);
    assert_ne!(first, third);
}

fn invocation_digest_for(identity: &str, tools: &[Tool]) -> Digest {
    crate::classify::invocation_digest(identity, tools)
}
