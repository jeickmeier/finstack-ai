//! Recorded wire fixtures against a loopback-only OpenAI-compatible endpoint.

use std::sync::Arc;

use finstack_ai_provider_openai_compatible::{
    Authentication, EndpointKind, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    OpenAiModelConfig, SecretString,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, AuthorizationContext, CancellationSignal, ContentBlock,
    EffectId, JsonSchemaDraft, LaneId, Message, MessageId, MessageRole, Metadata, Model,
    ModelCallContext, ModelRequest, ModelRequestDraft, ModelRequestId, ModelRequestLimits,
    ModelSettings, ModelStreamItem, OperationLocator, OutputSpec, PrincipalRef, ProviderIds,
    RawJson, RetrySafety, RunCallContext, RunId, SUBMIT_FINAL_OUTPUT_TOOL, SchemaRef, SessionId,
    SideEffectClass, TextBlock, Timestamp, ToolExecutionMode, ToolId, ToolSpec,
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TEXT_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/openai-compatible/v1/text.sse");
const TOOL_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/openai-compatible/v1/tool.sse");
const STRUCTURED_SSE: &str = include_str!(
    "../../../../fixtures/compatibility/providers/openai-compatible/v1/structured.sse"
);

#[tokio::test(flavor = "multi_thread")]
async fn keyless_local_streams_text_usage_and_reuses_the_request_contract() {
    let (base_url, server) = serve_sse(TEXT_SSE).await;
    let provider = provider(&base_url);
    let mut stream = provider
        .request(request(draft(OutputSpec::PlainText, Arc::from([]))))
        .await
        .expect("request");
    let mut text = String::new();
    let mut completed = None;
    while let Some(item) = stream.next().await {
        match item.expect("stream item") {
            ModelStreamItem::TextDelta(delta) => text.push_str(&delta.text),
            ModelStreamItem::Completed(response) => completed = Some(response),
            _ => {}
        }
    }
    let completed = completed.expect("terminal response");
    assert_eq!(text, "hello world");
    assert_eq!(completed.usage.total_tokens(), Some(6));
    assert_eq!(completed.completion_id.as_ref(), "chat-text-1");
    let captured = server.await.expect("server task");
    assert!(
        captured.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "captured request: {captured}"
    );
    assert!(captured.contains("\"stream\":true"));
    assert!(captured.contains("\"model\":\"fixture-model\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn recorded_tool_call_fixture_normalizes_fragmented_arguments() {
    let (base_url, server) = serve_sse(TOOL_SSE).await;
    let provider = provider(&base_url);
    let tools = Arc::from([tool("weather", schema())]);
    let mut stream = provider
        .request(request(draft(OutputSpec::PlainText, tools)))
        .await
        .expect("request");
    let mut completed = None;
    while let Some(item) = stream.next().await {
        if let ModelStreamItem::Completed(response) = item.expect("stream item") {
            completed = Some(response);
        }
    }
    let response = completed.expect("terminal response");
    assert_eq!(response.tool_calls[0].name.as_ref(), "weather");
    assert_eq!(
        response.tool_calls[0].arguments.as_str(),
        r#"{"city":"Toronto"}"#
    );
    assert_eq!(response.usage.total_tokens(), Some(13));
    let captured = server.await.expect("server task");
    assert!(captured.contains("\"name\":\"weather\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn native_structured_output_is_schema_bound_and_normalized_as_json() {
    let (base_url, server) = serve_sse(STRUCTURED_SSE).await;
    let provider = provider(&base_url);
    let schema = schema();
    let output = OutputSpec::JsonSchema {
        schema: SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: schema.digest(),
        },
    };
    let tools = Arc::from([tool(SUBMIT_FINAL_OUTPUT_TOOL, schema)]);
    let mut stream = provider
        .request(request(draft(output, tools)))
        .await
        .expect("request");
    let mut completed = None;
    while let Some(item) = stream.next().await {
        match item.expect("stream item") {
            ModelStreamItem::TextDelta(_) => panic!("structured JSON must not emit text deltas"),
            ModelStreamItem::Completed(response) => completed = Some(response),
            _ => {}
        }
    }
    assert!(matches!(
        completed.expect("terminal").assistant_content[0],
        ContentBlock::Json(_)
    ));
    let captured = server.await.expect("server task");
    assert!(captured.contains("\"type\":\"json_schema\""));
    assert!(!captured.contains(SUBMIT_FINAL_OUTPUT_TOOL));
}

#[tokio::test(flavor = "multi_thread")]
async fn retryable_http_error_does_not_expose_response_body() {
    let canary = "provider-body-secret-canary";
    let (base_url, server) = serve_response(
        "429 Too Many Requests",
        "application/json",
        format!(r#"{{"error":"{canary}"}}"#).into_bytes(),
        false,
    )
    .await;
    let result = provider(&base_url)
        .request(request(draft(OutputSpec::PlainText, Arc::from([]))))
        .await;
    let Err(error) = result else {
        panic!("429 must fail");
    };
    assert_eq!(error.code(), "openai_http_error");
    assert!(error.retryable());
    assert!(!format!("{error:?}").contains(canary));
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_terminates_an_open_stream() {
    let (base_url, server) = serve_response(
        "200 OK",
        "text/event-stream",
        b"data: {\"id\":\"chat-open\"}".to_vec(),
        true,
    )
    .await;
    let signal = CancellationSignal::new();
    let mut model_request = request(draft(OutputSpec::PlainText, Arc::from([])));
    model_request.call.run.cancellation = signal.clone();
    let mut stream = provider(&base_url)
        .request(model_request)
        .await
        .expect("headers establish stream");
    signal.cancel();
    let error = stream
        .next()
        .await
        .expect("cancellation item")
        .expect_err("cancelled stream");
    assert_eq!(error.code(), "openai_cancelled");
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires FINSTACK_OPENAI_BASE_URL, FINSTACK_OPENAI_MODEL, and optional FINSTACK_OPENAI_API_KEY"]
async fn optional_live_smoke() {
    let base_url = std::env::var("FINSTACK_OPENAI_BASE_URL").expect("live base URL");
    let model_name = std::env::var("FINSTACK_OPENAI_MODEL").expect("live model");
    let mut config = OpenAiCompatibleConfig::try_new(base_url, EndpointKind::Gateway)
        .expect("live provider config");
    if let Ok(secret) = std::env::var("FINSTACK_OPENAI_API_KEY") {
        config = config.with_authentication(Authentication::Bearer(
            SecretString::try_new(secret).expect("live credential"),
        ));
    }
    let provider = OpenAiCompatibleProvider::try_new(
        config,
        vec![
            OpenAiModelConfig::try_new(&model_name, 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("live model config"),
        ],
    )
    .expect("live provider");
    let mut request = request(draft(OutputSpec::PlainText, Arc::from([])));
    request.draft.model = finstack_ai_runtime::ModelName::try_new(model_name).expect("live model");
    let mut stream = provider.request(request).await.expect("live request");
    let mut completed = false;
    while let Some(item) = stream.next().await {
        if matches!(
            item.expect("live stream item"),
            ModelStreamItem::Completed(_)
        ) {
            completed = true;
        }
    }
    assert!(completed, "live stream must complete");
}

fn provider(base_url: &str) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::try_new(
        OpenAiCompatibleConfig::try_new(base_url, EndpointKind::OpenAi).expect("config"),
        vec![
            OpenAiModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model"),
        ],
    )
    .expect("provider")
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
        model: finstack_ai_runtime::ModelName::try_new("fixture-model").expect("model"),
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
    }
}

fn schema() -> RawJson {
    RawJson::parse(br#"{"additionalProperties":false,"properties":{"answer":{"type":"integer"},"city":{"type":"string"}},"type":"object"}"#)
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
            request_id: ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789a5")
                .expect("request id"),
        },
        draft,
        continuation_state: None,
    }
}

async fn serve_sse(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    serve_response(
        "200 OK",
        "text/event-stream",
        body.as_bytes().to_vec(),
        false,
    )
    .await
}

async fn serve_response(
    status: &str,
    content_type: &str,
    body: Vec<u8>,
    hold_open: bool,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let status = status.to_owned();
    let content_type = content_type.to_owned();
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
        let advertised_length = if hold_open {
            body.len() + 1_000
        } else {
            body.len()
        };
        let response_headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {advertised_length}\r\nConnection: close\r\n\r\n"
        );
        socket
            .write_all(response_headers.as_bytes())
            .await
            .expect("write headers");
        socket.write_all(&body).await.expect("write body");
        if hold_open {
            core::future::pending::<()>().await;
        }
        String::from_utf8(request).expect("request UTF-8")
    });
    (format!("http://{address}"), task)
}
