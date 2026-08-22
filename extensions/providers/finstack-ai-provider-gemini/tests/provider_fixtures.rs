//! Recorded wire fixtures against a loopback-only Gemini `generateContent` endpoint.

use std::sync::Arc;

use finstack_ai_kernel::{
    ContentBlock, EffectId, JsonSchemaDraft, LaneId, LimitKey, Message, MessageId, MessageRole,
    Metadata, ModelRequestId, OpaquePayload, OperationLocator, OutputSpec, PrincipalRef,
    ProviderIds, RawJson, RetrySafety, RunId, SUBMIT_FINAL_OUTPUT_TOOL, SchemaRef, SessionId,
    TextBlock, Timestamp, ToolExecutionMode, ToolId,
};
use finstack_ai_provider_gemini::{GeminiConfig, GeminiModelConfig, GeminiProvider};
use finstack_ai_provider_wire::{
    GEMINI_CACHED_TOKENS_KEY, GEMINI_GROUNDING_MEDIA_TYPE, GEMINI_THOUGHTS_TOKENS_KEY,
};
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, Authentication, AuthorizationContext,
    CancellationSignal, Model, ModelCallContext, ModelRequest, ModelRequestDraft,
    ModelRequestLimits, ModelSettings, ModelStreamItem, RunCallContext, SecretString,
    SideEffectClass, ToolSpec,
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TEXT_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--text.sse");
const TOOL_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--tool.sse");
const STRUCTURED_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--structured.sse");
const THINKING_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--thinking.sse");
const GROUNDING_SSE: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/valid--grounding.sse");
const HTTP_429: &str =
    include_str!("../../../../fixtures/compatibility/providers/v1/gemini/error--http-429.json");

const SIGNATURE: &str = "c2ln-fixture-001";

#[tokio::test(flavor = "multi_thread")]
async fn text_fixture_end_to_end() {
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
    assert_eq!(text, "Hello");
    assert_eq!(completed.usage.input_tokens(), Some(12));
    assert_eq!(completed.usage.output_tokens(), Some(4));
    assert_eq!(completed.usage.total_tokens(), Some(16));
    assert_eq!(completed.completion_id.as_ref(), "resp-fixture-1");
    let captured = server.await.expect("server task");
    assert!(
        captured.starts_with("POST "),
        "captured request: {captured}"
    );
    assert!(captured.contains("\"candidateCount\":1"));
    assert!(!captured.contains("x-goog-api-key"));
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_fixture_yields_tool_calls() {
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
    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].name.as_ref(), "weather");
    assert_eq!(
        response.tool_calls[0].arguments.as_str(),
        r#"{"city":"Toronto"}"#
    );
    assert_eq!(
        response.tool_calls[0].provider_call_id.as_deref(),
        Some("fc-1")
    );
    assert_eq!(response.tool_calls[1].name.as_ref(), "lookup");
    assert!(response.tool_calls[1].provider_call_id.is_none());
    assert_eq!(response.usage.total_tokens(), Some(15));
    let captured = server.await.expect("server task");
    assert!(captured.contains("\"name\":\"weather\""));
    assert!(captured.contains("\"parameters\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn thinking_fixture_round_trips_signature_into_continuation() {
    let (base_url, server) = serve_sse(THINKING_SSE).await;
    let first_provider = provider(&base_url);
    let mut stream = first_provider
        .request(request(draft(OutputSpec::PlainText, Arc::from([]))))
        .await
        .expect("request");
    let mut reasoning = String::new();
    let mut text = String::new();
    let mut completed = None;
    while let Some(item) = stream.next().await {
        match item.expect("stream item") {
            ModelStreamItem::ReasoningDelta(delta) => reasoning.push_str(&delta.text),
            ModelStreamItem::TextDelta(delta) => text.push_str(&delta.text),
            ModelStreamItem::Completed(response) => completed = Some(response),
            _ => {}
        }
    }
    assert_eq!(reasoning, "consider this");
    assert_eq!(text, "hello");
    let completed = completed.expect("terminal");
    assert_eq!(completed.usage.input_tokens(), Some(10));
    assert_eq!(completed.usage.output_tokens(), Some(12));
    assert_eq!(completed.usage.total_tokens(), Some(22));
    assert_eq!(
        *completed
            .usage
            .extension_counters()
            .get(&LimitKey::parse(GEMINI_THOUGHTS_TOKENS_KEY).expect("key"))
            .expect("thoughts counter"),
        7
    );
    let first_captured = server.await.expect("server task");
    assert!(!first_captured.contains(SIGNATURE));

    let continuation_state = completed.continuation_state.clone();
    let (second_base_url, second_server) = serve_sse(TEXT_SSE).await;
    let second_provider = provider(&second_base_url);
    let mut second_request = request(draft(OutputSpec::PlainText, Arc::from([])));
    second_request.continuation_state = continuation_state;
    let mut second_stream = second_provider
        .request(second_request)
        .await
        .expect("second request");
    while let Some(item) = second_stream.next().await {
        item.expect("second stream item");
    }
    let second_captured = second_server.await.expect("second server task");
    assert!(
        second_captured.contains(SIGNATURE),
        "the fabricated thought signature must replay verbatim: {second_captured}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn grounding_fixture_yields_opaque_block() {
    let (base_url, server) = serve_sse(GROUNDING_SSE).await;
    let provider = provider(&base_url);
    let mut stream = provider
        .request(request(draft(OutputSpec::PlainText, Arc::from([]))))
        .await
        .expect("request");
    let mut completed = None;
    while let Some(item) = stream.next().await {
        if let ModelStreamItem::Completed(response) = item.expect("stream item") {
            completed = Some(response);
        }
    }
    let response = completed.expect("terminal response");
    let opaque = response
        .assistant_content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Opaque(block) => Some(block),
            _ => None,
        })
        .expect("grounding opaque block");
    assert_eq!(opaque.media_type(), GEMINI_GROUNDING_MEDIA_TYPE);
    let OpaquePayload::Json(payload) = opaque.payload() else {
        panic!("expected a JSON opaque payload");
    };
    assert!(payload.as_str().contains("webSearchQueries"));
    assert!(payload.as_str().contains("groundingChunks"));
    assert_eq!(
        *response
            .usage
            .extension_counters()
            .get(&LimitKey::parse(GEMINI_CACHED_TOKENS_KEY).expect("key"))
            .expect("cached content counter"),
        3
    );
    assert_eq!(response.usage.total_tokens(), Some(20));
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn structured_fixture_completes() {
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
    let response = completed.expect("terminal");
    let ContentBlock::Json(json) = &response.assistant_content[0] else {
        panic!("expected assistant JSON");
    };
    assert_eq!(json.value().as_str(), r#"{"answer":42}"#);
    let captured = server.await.expect("server task");
    assert!(captured.contains("\"responseMimeType\":\"application/json\""));
    assert!(captured.contains("\"responseJsonSchema\""));
    assert!(!captured.contains("\"tools\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_429_maps_to_retryable_gemini_http_error() {
    let (base_url, server) = serve_response(
        "429 Too Many Requests",
        "application/json",
        HTTP_429.as_bytes().to_vec(),
        false,
    )
    .await;
    let result = provider(&base_url)
        .request(request(draft(OutputSpec::PlainText, Arc::from([]))))
        .await;
    let Err(error) = result else {
        panic!("429 must fail");
    };
    assert_eq!(error.code(), "gemini_http_error");
    assert!(error.retryable());
    assert!(!format!("{error:?}").contains("provider-body-secret-canary"));
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn api_key_header_sent_and_never_logged() {
    // The provider hard-fails config construction when a credential is
    // configured against a non-HTTPS base URL (SEC-INV: credentials require
    // HTTPS). The loopback test server in this file is plaintext HTTP only,
    // so an end-to-end capture of the `x-goog-api-key` header over the wire
    // is not reachable without violating that invariant. This test instead
    // verifies the two enforceable halves of the contract: (1) an API key
    // configured against HTTPS is accepted, and its raw value never appears
    // in `Debug` output of the provider or a resulting config error; (2) the
    // same credential rejected against plaintext HTTP surfaces
    // `gemini_config_invalid` rather than silently sending the header in the
    // clear. See task-8 report for the deviation from the brief's literal
    // loopback-capture wording.
    let canary = "AIza-secret-canary-042";
    let secret = SecretString::try_new(canary).expect("secret");
    let https_config = GeminiConfig::try_new("https://generativelanguage.googleapis.com")
        .expect("config")
        .with_authentication(Authentication::ApiKey(secret.clone()))
        .expect("authentication");
    let https_model =
        GeminiModelConfig::try_new("gemini-fixture", 1_000_000, 128_000, 4_096).expect("model");
    let https_provider =
        GeminiProvider::try_new(https_config, vec![https_model]).expect("https provider builds");
    assert!(!format!("{https_provider:?}").contains(canary));

    let (base_url, server) = serve_response("200 OK", "text/event-stream", Vec::new(), false).await;
    let plaintext_config = GeminiConfig::try_new(&base_url)
        .expect("config")
        .with_authentication(Authentication::ApiKey(secret))
        .expect("authentication");
    let plaintext_model =
        GeminiModelConfig::try_new("gemini-fixture", 1_000_000, 128_000, 4_096).expect("model");
    let error = GeminiProvider::try_new(plaintext_config, vec![plaintext_model])
        .expect_err("plaintext HTTP with credentials must be rejected");
    assert_eq!(error.code(), "gemini_config_invalid");
    assert!(!format!("{error:?}").contains(canary));
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_aborts_stream() {
    let (base_url, server) = serve_response(
        "200 OK",
        "text/event-stream",
        b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"partial\"}]},\"index\":0}],\"responseId\":\"resp-open\"}".to_vec(),
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
    assert_eq!(error.code(), "gemini_cancelled");
    server.abort();
}

fn provider(base_url: &str) -> GeminiProvider {
    let model =
        GeminiModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096).expect("model");
    GeminiProvider::try_new(
        GeminiConfig::try_new(base_url).expect("config"),
        vec![model],
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
        model: finstack_ai_runtime::ports::model::ModelName::try_new("fixture-model")
            .expect("model"),
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
        deferral: finstack_ai_runtime::ports::model::ToolDeferralSupport::Never,
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
                relation_depth: 0,
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
