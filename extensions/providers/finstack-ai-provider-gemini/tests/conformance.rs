//! Model-port conformance for Gemini `generateContent` (ported from
//! `finstack-ai-provider-anthropic/tests/conformance.rs`).

use std::sync::Arc;

use finstack_ai_kernel::{
    ContentBlock, EffectId, JsonSchemaDraft, LaneId, Message, MessageId, MessageRole, Metadata,
    ModelRequestId, OperationLocator, OutputSpec, PrincipalRef, ProviderIds, RawJson, RetrySafety,
    RunId, SUBMIT_FINAL_OUTPUT_TOOL, SchemaRef, SessionId, TextBlock, Timestamp,
    ToolExecutionMode, ToolId,
};
use finstack_ai_provider_gemini::{GeminiConfig, GeminiModelConfig, GeminiProvider};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, AuthorizationContext, CancellationSignal,
    GeminiGenerateContentAssembly, Model, ModelCallContext, ModelName, ModelRequest,
    ModelRequestDraft, ModelRequestLimits, ModelSettings, ModelStreamItem, ModelStreamLimits,
    ModelTerminal, RunCallContext, SideEffectClass, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_test::{ModelConformanceCase, check_model_conformance};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TEXT_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--text.sse");
const TOOL_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--tool.sse");
const THINKING_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--thinking.sse");
const STRUCTURED_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--structured.sse");

const REQUEST_ID: &str = "01234567-89ab-7cde-89ab-0123456789a5";

fn fixture_model() -> GeminiModelConfig {
    GeminiModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096).expect("model")
}

fn provider(base_url: &str) -> GeminiProvider {
    let config = GeminiConfig::try_new(base_url).expect("config");
    GeminiProvider::try_new(config, vec![fixture_model()]).expect("provider")
}

fn draft(output: OutputSpec, tools: Arc<[ToolSpec]>) -> ModelRequestDraft {
    let message = Message::try_new(
        MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        Timestamp::from_unix_ms(0).expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message");
    ModelRequestDraft {
        model: ModelName::try_new("fixture-model").expect("model"),
        messages: Arc::from([message]),
        tools,
        output,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 1_000_000,
            max_input_tokens: 100_000,
            max_output_tokens: 1_024,
        },
    }
}

fn tool(name: &str, input_schema: RawJson) -> ToolSpec {
    ToolSpec {
        id: ToolId::parse("finstack.tools.fixture").expect("tool id"),
        model_name: Arc::from(name),
        title: Arc::from("Fixture tool"),
        description: Arc::from("A deterministic fixture tool."),
        input_schema,
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 1_024,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    }
}

fn schema() -> RawJson {
    RawJson::parse(br#"{"additionalProperties":false,"properties":{"answer":{"type":"integer"}},"type":"object"}"#)
        .expect("schema")
}

fn request(draft: ModelRequestDraft) -> ModelRequest {
    ModelRequest {
        call: ModelCallContext {
            run: RunCallContext {
                locator: OperationLocator::try_new(
                    "tenant-a",
                    SessionId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("session"),
                    LaneId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("lane"),
                    RunId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("run"),
                )
                .expect("locator"),
                authorization: AuthorizationContext {
                    principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                        .expect("principal"),
                    authentication_method: Arc::from("fixture"),
                    assurance_level: Arc::from("test"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("policy-v1"),
                    decision_id: Arc::from("decision-v1"),
                },
                effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("effect"),
                attempt: 1,
                deadline: None,
                budget_scope_id: None,
                cancellation: CancellationSignal::new(),
            },
            request_id: ModelRequestId::parse(REQUEST_ID).expect("request id"),
        },
        draft,
        continuation_state: None,
    }
}

/// Replay one fixture's SSE `data:` payloads through the same assembler the
/// leaf's drive loop uses, and return the terminal it produces. This mirrors
/// `drive_response` in `src/provider.rs` (request id + structured flag) so
/// the expected terminal is derived independently of the loopback stream.
fn expected_terminal(sse: &str, structured: bool) -> ModelTerminal {
    let mut assembly = GeminiGenerateContentAssembly::new(REQUEST_ID.to_owned(), structured);
    let mut terminal = None;
    for line in sse.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let items = assembly.consume(data).expect("gemini fixture event");
        for item in items {
            if let ModelStreamItem::Completed(response) = item {
                terminal = Some(ModelTerminal::Completed(response));
            }
        }
    }
    terminal.expect("gemini fixture omitted a completion")
}

async fn serve_sse(body: &str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let body = body.as_bytes().to_vec();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let header_end = loop {
            let count = socket.read(&mut buffer).await.expect("read request");
            assert!(count > 0, "request closed before headers");
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .expect("content length");
        while request.len() < header_end + content_length {
            let count = socket.read(&mut buffer).await.expect("read body");
            assert!(count > 0, "request closed before body");
            request.extend_from_slice(&buffer[..count]);
        }
        let response_headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket
            .write_all(response_headers.as_bytes())
            .await
            .expect("write headers");
        socket.write_all(&body).await.expect("write body");
        String::from_utf8(request).expect("request UTF-8")
    });
    (format!("http://{address}"), task)
}

async fn assert_protocol(draft: ModelRequestDraft, sse: &str, structured: bool) {
    let expected = expected_terminal(sse, structured);
    let (base, server) = serve_sse(sse).await;
    let model = provider(&base);
    let selected = model.descriptor().models[0].clone();
    check_model_conformance(
        &model,
        ModelConformanceCase {
            model: selected,
            request: request(draft),
            expected_terminal: expected,
            stream_limits: ModelStreamLimits::default(),
        },
    )
    .await
    .expect("public Model contract");
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_text_conformance() {
    assert_protocol(draft(OutputSpec::PlainText, Arc::from([])), TEXT_SSE, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_tool_conformance() {
    let tools = Arc::from([tool("weather", schema())]);
    assert_protocol(draft(OutputSpec::PlainText, tools), TOOL_SSE, false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_thinking_conformance() {
    assert_protocol(
        draft(OutputSpec::PlainText, Arc::from([])),
        THINKING_SSE,
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_structured_conformance() {
    let schema = schema();
    let output = OutputSpec::JsonSchema {
        schema: SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: schema.digest(),
        },
    };
    let tools = Arc::from([tool(SUBMIT_FINAL_OUTPUT_TOOL, schema)]);
    assert_protocol(draft(output, tools), STRUCTURED_SSE, true).await;
}
