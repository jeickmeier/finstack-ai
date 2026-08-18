use std::sync::Arc;

use finstack_ai_runtime::{
    AssembledToolStream, AuthorizationContext, CancellationSignal, ContentBlock, ContextAuthority,
    ContextBudget, ContextCallContext, ContextItemKind, ContextOverflowPolicy, ContextProvider,
    ContextRequest, Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata,
    OperationLocator, PendingToolEffect, PrincipalRef, ReconcileContext, RetrySafety,
    RunCallContext, RunId, SessionId, SideEffectClass, TextBlock, ToolBatchId, ToolCallBlock,
    ToolCallContext, ToolCallId, ToolFailurePolicy, ToolReconcileResult, ToolStreamItem,
    ToolStreamLimits, Toolset, ValidatedToolCall,
};
use finstack_ai_test::{
    ContextConformanceCase, ToolsetConformanceCase, check_context_conformance,
    check_toolset_conformance,
};
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
    McpToolset::connect(Arc::new(transport), McpConfig::default(), None)
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
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default(), None)
        .await
        .expect("connects");
    let error = call(&toolset, "boom", serde_json::json!({}))
        .await
        .expect_err("isError must surface as a ToolError");
    assert!(format!("{error}").contains("tool_output_invalid"));
}

#[tokio::test]
async fn input_required_maps_to_form_interaction() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"ask","description":"Ask","inputSchema":{"type":"object"}}]}),
        serde_json::json!({
            "resultType":"input_required",
            "content":[{"type":"text","text":"Need a city"}],
            "inputRequests":{"type":"object","properties":{"city":{"type":"string"}}}
        }),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"resolved"}],"isError":false}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default(), None)
        .await
        .expect("connects");
    let error = call(&toolset, "ask", serde_json::json!({}))
        .await
        .expect_err("elicitation parks the tool");
    assert!(format!("{error}").contains(MCP_INPUT_REQUIRED));
    let request = interaction_request_from_tool_error(&error).expect("mapped request");
    assert_eq!(request.kind(), &finstack_ai_runtime::InteractionKind::Form);
    let output = call(&toolset, "ask", serde_json::json!({"city":"oslo"}))
        .await
        .expect("host resolution completes the tool");
    assert!(output.contains("resolved"));
}

#[tokio::test]
async fn oversize_result_truncates_with_a_recorded_marker() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"big","description":"Big","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"abcdefghijklmnopqrstuvwxyz"}],"isError":false}),
    ]);
    let config = McpConfig::default().with_inline_result_bytes(32);
    let toolset = McpToolset::connect(Arc::new(transport), config, None)
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

#[test]
fn mcp_json_snapshot_allowlists_and_binds_one_server() {
    let config = McpConfig::try_from_mcp_json(
        r#"{"mcpServers":{"echo":{"command":"/bin/echo","args":[]}}}"#,
    )
    .expect("snapshot");
    assert!(config.identity().contains("/bin/echo"));
}

#[test]
fn mcp_json_rejects_runtime_scan_keys() {
    let error = McpConfig::try_from_mcp_json(
        r#"{"mcpServers":{"echo":{"command":"/bin/echo","cwd":"."}}}"#,
    )
    .expect_err("cwd is a scan");
    assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
}

#[tokio::test]
async fn factory_rejects_a_server_that_is_not_allowlisted() {
    let error = McpConfig::default()
        .stdio(StdioConfig::new("/usr/bin/unlisted", Vec::<String>::new()))
        .expect_err("deny by default");
    assert!(format!("{error}").contains(MCP_SERVER_NOT_ALLOWLISTED));
}

#[tokio::test]
async fn confined_stdio_fails_closed_when_confinement_is_unavailable() {
    let root = std::env::temp_dir();
    let profile = finstack_ai_runtime::ConfinementProfile::try_new(&root).expect("root");
    let config = McpConfig::default()
        .allow_command("/bin/echo")
        .stdio_confined(
            StdioConfig::new("/bin/echo", Vec::<String>::new()),
            finstack_ai_runtime::ProcessConfinement::unavailable(),
            profile,
        )
        .expect("allowlisted");
    let result = McpToolsetFactory::new(config).construct().await;
    let error = match result {
        Ok(_) => panic!("requested confinement is unavailable"),
        Err(error) => error,
    };
    assert!(format!("{error}").contains(finstack_ai_runtime::CONFINEMENT_UNAVAILABLE));
}

#[tokio::test]
async fn toolset_satisfies_the_published_port_conformance_suite() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"pong"}],"isError":false}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default(), None)
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

fn context_call() -> ContextCallContext {
    ContextCallContext {
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
        provider_index: 0,
        chain_digest: Digest::raw_json(b"mcp-context-chain"),
    }
}

fn context_request() -> ContextRequest {
    ContextRequest {
        session_id: SessionId::from_bytes([1; 16]),
        lane_id: LaneId::from_bytes([2; 16]),
        run_id: RunId::from_bytes([3; 16]),
        user_input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )]),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 8,
            max_tokens: 1_000,
            max_bytes: 64 * 1024,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    }
}

fn listed_resource(name: &str, uri: &str) -> serde_json::Value {
    serde_json::json!({
        "resultType": "complete",
        "resources": [{
            "uri": uri,
            "name": name,
            "title": name,
            "mimeType": "text/plain"
        }]
    })
}

fn read_resource(uri: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "resultType": "complete",
        "contents": [{
            "uri": uri,
            "mimeType": "text/plain",
            "text": text
        }]
    })
}

#[test]
fn mcp_resource_provider_is_never_trusted_application_instructions() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let provider = runtime.block_on(async {
        let transport =
            ScriptedTransport::new(vec![listed_resource("notes", "mcp://fixture/notes")]);
        McpContextProvider::connect(Arc::new(transport), &McpConfig::default(), None)
            .await
            .expect("connects")
    });
    assert!(!provider.descriptor().trusted_application_instructions);
}

#[tokio::test]
async fn resource_pagination_follows_cursors_including_the_empty_string() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({
            "resultType":"complete",
            "resources":[{"uri":"mcp://a","name":"a"}],
            "nextCursor":""
        }),
        serde_json::json!({
            "resultType":"complete",
            "resources":[{"uri":"mcp://b","name":"b"}]
        }),
    ]);
    let listed = crate::resources::enumerate_resources(&transport)
        .await
        .expect("enumerates");
    assert_eq!(
        listed
            .iter()
            .map(|resource| resource.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[tokio::test]
async fn factory_context_provider_rejects_a_server_that_is_not_allowlisted() {
    let result = McpToolsetFactory::new(McpConfig::default())
        .construct_context_provider()
        .await;
    let Err(error) = result else {
        panic!("deny by default");
    };
    assert!(format!("{error}").contains(MCP_SERVER_NOT_ALLOWLISTED));
}

#[tokio::test]
async fn mid_run_resource_list_changes_are_ignored() {
    let transport = Arc::new(ScriptedTransport::new(vec![
        listed_resource("notes", "mcp://fixture/notes"),
        read_resource("mcp://fixture/notes", "remember this note"),
        listed_resource("later", "mcp://fixture/later"),
    ]));
    let provider = McpContextProvider::connect(
        Arc::clone(&transport) as Arc<dyn crate::transport::McpTransport>,
        &McpConfig::default(),
        None,
    )
    .await
    .expect("connects");
    assert_eq!(provider.frozen_names(), ["notes"]);
    let contribution = provider
        .collect(context_call(), context_request())
        .await
        .expect("collect");
    assert_eq!(contribution.items.len(), 1);
    assert_eq!(contribution.items[0].kind, ContextItemKind::QuotedSource);
    assert_eq!(contribution.items[0].authority, ContextAuthority::Untrusted);
    assert_eq!(
        contribution.items[0].provenance.source_ref.as_deref(),
        Some("mcp://fixture/notes")
    );
    assert_eq!(
        transport.called_methods(),
        ["resources/list", "resources/templates", "resources/read"]
    );
}

async fn notes_provider() -> McpContextProvider {
    let transport = ScriptedTransport::new(vec![
        listed_resource("notes", "mcp://fixture/notes"),
        read_resource("mcp://fixture/notes", "remember this note"),
    ]);
    McpContextProvider::connect(Arc::new(transport), &McpConfig::default(), None)
        .await
        .expect("connects")
}

#[tokio::test]
async fn resource_provider_satisfies_context_conformance() {
    let expected = notes_provider()
        .await
        .collect(context_call(), context_request())
        .await
        .expect("expected");
    check_context_conformance(
        &notes_provider().await,
        ContextConformanceCase {
            context: context_call(),
            request: context_request(),
            expected,
        },
    )
    .await
    .expect("published context conformance suite");
}

fn preview_model() -> Arc<finstack_ai_test::ScriptedModel> {
    use finstack_ai::runtime::{
        ModelContextProfile, ModelName, ModelResponse, ModelStreamItem, TokenEstimatorRef,
        TokenEstimatorSource,
    };
    use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};

    Arc::new(ScriptedModel::from_plans(
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("preview-1").expect("model"),
            hard_input_bytes: 1_048_576,
            context_window_tokens: 8_192,
            max_output_tokens: 512,
            reserved_output_tokens: 128,
            provider_overhead_tokens: 32,
            estimator: TokenEstimatorRef {
                id: Arc::from("scripted.utf8"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        },
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                    finstack_ai::runtime::TextDelta {
                        text: Arc::from("ok"),
                    },
                ))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([ContentBlock::Text(
                        TextBlock::try_new("ok").expect("text"),
                    )]),
                    tool_calls: Arc::from([]),
                    usage: finstack_ai::runtime::Usage::empty(),
                    provider_ids: finstack_ai::runtime::ProviderIds::empty(),
                    completion_id: Arc::from("mcp-resource-1"),
                    continuation_state: None,
                }))),
            ],
        }],
    ))
}

async fn run_with_mcp_provider(
    provider: McpContextProvider,
    model: Arc<finstack_ai_test::ScriptedModel>,
) {
    use finstack_ai::runtime::{
        AgentId, BundleId, ComponentId, ComponentRef, JournalStore, Model, ModelName, Version,
    };
    use finstack_ai::{Agent, AgentRunRequest, PrincipalRef, RunSecurityContext};
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("store"),
    );
    let version = Version {
        major: 1,
        minor: 0,
        patch: 0,
    };
    let agent = Agent::builder(
        AgentId::parse("test.agent.mcp-resources").expect("agent"),
        BundleId::parse("test.bundle.mcp-resources").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.mcp-resources").expect("model"),
                Some(version),
            ),
            Arc::clone(&model) as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.mcp-resources").expect("store"),
                Some(version),
            ),
            store,
        ),
    )
    .context_provider(
        ComponentRef::new(
            ComponentId::parse("finstack.context.mcp").expect("provider"),
            Some(version),
        ),
        Arc::new(provider),
    )
    .build()
    .await
    .expect("agent");
    let security = RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
        "local",
        "test",
        "mcp-resource-policy-v1",
        "mcp-resource-decision-v1",
        None,
    )
    .expect("security");
    agent
        .run(
            AgentRunRequest::try_new(
                ModelName::try_new("preview-1").expect("model name"),
                "hello",
                security,
            )
            .expect("request"),
        )
        .await
        .expect("production run");
}

fn message_texts(request: &finstack_ai::runtime::ModelRequest) -> Vec<String> {
    request
        .draft
        .messages
        .iter()
        .flat_map(|message| message.content().iter())
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn production_driver_collects_frozen_mcp_resources() {
    let expected = notes_provider()
        .await
        .collect(context_call(), context_request())
        .await
        .expect("expected collect");
    check_context_conformance(
        &notes_provider().await,
        ContextConformanceCase {
            context: context_call(),
            request: context_request(),
            expected: expected.clone(),
        },
    )
    .await
    .expect("conformance before driver");

    let transport = Arc::new(ScriptedTransport::new(vec![
        listed_resource("notes", "mcp://fixture/notes"),
        read_resource("mcp://fixture/notes", "remember this note"),
    ]));
    let provider = McpContextProvider::connect(
        Arc::clone(&transport) as Arc<dyn crate::transport::McpTransport>,
        &McpConfig::default(),
        None,
    )
    .await
    .expect("driver provider");
    let model = preview_model();
    run_with_mcp_provider(provider, Arc::clone(&model)).await;
    let texts = message_texts(&model.last_request().expect("model request"));
    assert!(
        texts.iter().any(|text| text.contains("remember this note")),
        "production install_context_providers / collect_context_stage must project the frozen resource, got {texts:?}"
    );
    assert_eq!(texts.last().map(String::as_str), Some("hello"));
    assert_eq!(
        transport.called_methods(),
        ["resources/list", "resources/templates", "resources/read"]
    );
    assert_eq!(expected.items[0].kind, ContextItemKind::QuotedSource);
    assert_eq!(expected.items[0].authority, ContextAuthority::Untrusted);
}

#[tokio::test]
async fn sampling_create_message_is_rejected() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"echo","inputSchema":{"type":"object"}}]}),
        serde_json::json!({"method":"sampling/createMessage","params":{}}),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default(), None)
        .await
        .expect("connects");
    let error = call(&toolset, "echo", serde_json::json!({}))
        .await
        .expect_err("sampling stays rejected");
    assert!(format!("{error}").contains(MCP_RESULT_UNSUPPORTED));
    assert!(format!("{error}").contains("sampling"));
}

#[tokio::test]
async fn prompts_list_freezes_untrusted_instruction_text() {
    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"echo","inputSchema":{"type":"object"}}]}),
        serde_json::json!({
            "resultType":"complete",
            "prompts":[{"name":"reviewer","description":"Cite primary sources."}]
        }),
    ]);
    let toolset = McpToolset::connect(Arc::new(transport), McpConfig::default(), None)
        .await
        .expect("connects");
    assert_eq!(toolset.frozen_prompts().len(), 1);
    assert_eq!(toolset.frozen_prompts()[0].name(), "reviewer");
    let instruction =
        finstack_ai::InstructionSpec::try_new(toolset.frozen_prompts()[0].text()).expect("spec");
    assert_eq!(instruction.text(), "Cite primary sources.");
}

#[tokio::test]
async fn resource_templates_are_frozen_and_collect_reads_only_those_names() {
    let transport = ScriptedTransport::new(vec![
        listed_resource("notes", "mcp://fixture/notes"),
        serde_json::json!({
            "resultType":"complete",
            "resourceTemplates":[{"name":"ticket","uriTemplate":"mcp://fixture/ticket"}]
        }),
        read_resource("mcp://fixture/notes", "remember this note"),
        read_resource("mcp://fixture/ticket", "ticket body"),
    ]);
    let provider = McpContextProvider::connect(Arc::new(transport), &McpConfig::default(), None)
        .await
        .expect("connects");
    assert_eq!(provider.frozen_names(), ["notes", "ticket"]);
    assert_eq!(provider.frozen_template_names(), ["ticket"]);
    let contribution = provider
        .collect(context_call(), context_request())
        .await
        .expect("collect");
    assert_eq!(contribution.items.len(), 2);
}

struct CaptureListChanged(std::sync::Mutex<Vec<String>>);

impl McpListChangedObserver for CaptureListChanged {
    fn on_list_changed(&self, method: &str) {
        self.0.lock().expect("lock").push(method.to_owned());
    }
}

#[tokio::test]
async fn list_changed_is_observed_and_does_not_add_a_tool() {
    let observer = Arc::new(CaptureListChanged(std::sync::Mutex::new(Vec::new())));
    let transport = ScriptedTransport::new(vec![serde_json::json!({
        "resultType":"complete",
        "tools":[{"name":"echo","inputSchema":{"type":"object"}}]
    })])
    .with_pending_notifications(vec!["notifications/tools/list_changed".to_owned()]);
    let toolset = McpToolset::connect(
        Arc::new(transport),
        McpConfig::default(),
        Some(Arc::clone(&observer) as Arc<dyn McpListChangedObserver>),
    )
    .await
    .expect("connects");
    assert_eq!(toolset.tools().len(), 1);
    assert_eq!(toolset.tools()[0].model_name.as_ref(), "echo");
    assert_eq!(
        observer.0.lock().expect("lock").as_slice(),
        ["notifications/tools/list_changed"]
    );
}

fn elicitation_model() -> Arc<finstack_ai_test::ScriptedModel> {
    use finstack_ai::runtime::{
        ModelContextProfile, ModelName, ModelResponse, ModelStreamItem, ModelToolCall,
        TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta,
    };
    use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};

    let arguments = RawJson::parse(br#"{}"#).expect("arguments");
    Arc::new(ScriptedModel::from_plans(
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("preview-1").expect("model"),
            hard_input_bytes: 1_048_576,
            context_window_tokens: 8_192,
            max_output_tokens: 512,
            reserved_output_tokens: 128,
            provider_overhead_tokens: 32,
            estimator: TokenEstimatorRef {
                id: Arc::from("scripted.utf8"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        },
        vec![
            ScriptedModelPlan {
                actions: vec![
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                        index: 0,
                        name: Some(Arc::from("ask")),
                        arguments_delta: Arc::from(arguments.as_str()),
                        provider_call_id: None,
                    }))),
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                        assistant_content: Arc::from([]),
                        tool_calls: Arc::from([ModelToolCall {
                            name: Arc::from("ask"),
                            arguments,
                            provider_call_id: None,
                        }]),
                        usage: finstack_ai::runtime::Usage::empty(),
                        provider_ids: finstack_ai::runtime::ProviderIds::empty(),
                        completion_id: Arc::from("mcp-elicitation-1"),
                        continuation_state: None,
                    }))),
                ],
            },
            ScriptedModelPlan {
                actions: vec![
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                        finstack_ai::runtime::TextDelta {
                            text: Arc::from("oslo is ready"),
                        },
                    ))),
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                        assistant_content: Arc::from([ContentBlock::Text(
                            TextBlock::try_new("oslo is ready").expect("text"),
                        )]),
                        tool_calls: Arc::from([]),
                        usage: finstack_ai::runtime::Usage::empty(),
                        provider_ids: finstack_ai::runtime::ProviderIds::empty(),
                        completion_id: Arc::from("mcp-elicitation-2"),
                        continuation_state: None,
                    }))),
                ],
            },
        ],
    ))
}

#[tokio::test]
async fn elicitation_journals_interaction_and_host_resolution_completes_the_tool() {
    use std::time::Duration;

    use finstack_ai::runtime::{
        AgentId, BundleId, ComponentId, ComponentRef, JournalStore, LoadRequest, Model, Version,
    };
    use finstack_ai::{
        Agent, AgentRunRequest, InteractionResolution, PrincipalRef, RunSecurityContext,
    };
    use finstack_ai_kernel::{AuthorizationEvidence, InteractionKind};
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

    let transport = ScriptedTransport::new(vec![
        serde_json::json!({"resultType":"complete","tools":[{"name":"ask","description":"Ask","inputSchema":{"type":"object"}}]}),
        serde_json::json!({
            "resultType":"input_required",
            "content":[{"type":"text","text":"Need a city"}],
            "inputRequests":{"type":"object","properties":{"city":{"type":"string"}}}
        }),
        serde_json::json!({"resultType":"complete","content":[{"type":"text","text":"resolved"}],"isError":false}),
    ]);
    let toolset = McpToolset::connect(
        Arc::new(transport),
        McpConfig::default().with_read_only_tools(["ask"]),
        None,
    )
    .await
    .expect("connects");
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 1_024,
            snapshot_bytes: 16_384,
        })
        .expect("store"),
    );
    let version = Version {
        major: 1,
        minor: 0,
        patch: 0,
    };
    let model = elicitation_model();
    let agent = Agent::builder(
        AgentId::parse("test.agent.mcp-elicitation").expect("agent"),
        BundleId::parse("test.bundle.mcp-elicitation").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.mcp-elicitation").expect("model"),
                Some(version),
            ),
            Arc::clone(&model) as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.mcp-elicitation").expect("store"),
                Some(version),
            ),
            Arc::clone(&store),
        ),
    )
    .toolset(
        ComponentRef::new(
            ComponentId::parse("finstack.tools.mcp").expect("toolset"),
            Some(version),
        ),
        Arc::new(toolset) as Arc<dyn Toolset>,
    )
    .build()
    .await
    .expect("agent");
    let security = RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
        "local",
        "test",
        "mcp-elicitation-policy-v1",
        "mcp-elicitation-decision-v1",
        None,
    )
    .expect("security");
    let run = agent
        .start(
            AgentRunRequest::try_new(
                finstack_ai::runtime::ModelName::try_new("preview-1").expect("model name"),
                "ask the tool",
                security.clone(),
            )
            .expect("request"),
        )
        .expect("start");
    let listed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let listed = run.list_interactions().await.expect("list");
            if !listed.is_empty() {
                return listed;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("elicitation timeout");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind(), &InteractionKind::Form);
    let resolution = InteractionResolution::try_new(
        listed[0].interaction_id(),
        "mcp-elicitation-1",
        security.principal().clone(),
        AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .expect("auth"),
        RawJson::parse(br#"{"city":"oslo"}"#).expect("city"),
        None::<&str>,
    )
    .expect("resolution");
    tokio::time::timeout(Duration::from_secs(3), run.resolve_interaction(resolution))
        .await
        .expect("resolve timeout")
        .expect("resolve");
    let output = tokio::time::timeout(Duration::from_secs(8), run.result())
        .await
        .expect("run timeout")
        .expect("run result");
    assert!(output.text().contains("oslo is ready"), "{}", output.text());
    let kinds = store
        .load(LoadRequest {
            session_id: run.locator().session_id,
        })
        .await
        .expect("load journal")
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(|record| record.body().kind_name().to_string())
        .collect::<Vec<_>>();
    assert!(
        kinds.iter().any(|kind| kind == "interaction_requested"),
        "durable HITL must journal InteractionRequested, got {kinds:?}"
    );
}
