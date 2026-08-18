//! Model-port conformance for native Ollama `/api/chat` (ported from the
//! deleted gateway `assert_protocol` / `check_model_conformance` caller).

use std::sync::Arc;

use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, ContentBlock, EffectId, LaneId, Message, MessageId,
    MessageRole, Metadata, Model, ModelCallContext, ModelName, ModelRequest, ModelRequestDraft,
    ModelRequestId, ModelRequestLimits, ModelSettings, ModelTerminal, OllamaChatAssembly,
    OperationLocator, OutputSpec, PrincipalRef, ProviderIds, RawJson, RunCallContext, RunId,
    SessionId, TextBlock, Timestamp,
};
use finstack_ai_test::{ModelConformanceCase, check_model_conformance};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const OLLAMA_NDJSON: &str = concat!(
    "{\"message\":{\"content\":\"hello \"},\"done\":false}\n",
    "{\"message\":{\"content\":\"world\"},\"done\":false}\n",
    "{\"message\":{\"content\":\"\"},\"done\":true,\"prompt_eval_count\":4,\"eval_count\":2}\n",
);
const REQUEST_ID: &str = "01234567-89ab-7cde-89ab-0123456789a5";

fn fixture_model() -> OllamaModelConfig {
    OllamaModelConfig::try_new("fixture-model", 1_000_000, 8_192, 1_024, 1_024, 64).expect("model")
}

fn provider(base_url: &str) -> OllamaProvider {
    let config = OllamaConfig::try_new(base_url).expect("config");
    OllamaProvider::try_new(config, vec![fixture_model()]).expect("provider")
}

fn request() -> ModelRequest {
    let selected = ModelName::try_new("fixture-model").expect("name");
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
        draft: ModelRequestDraft {
            model: selected,
            messages: Arc::from([Message::try_new(
                MessageId::parse("01234567-89ab-7cde-89ab-0123456789a6").expect("message"),
                MessageRole::User,
                vec![ContentBlock::Text(
                    TextBlock::try_new("hello").expect("text"),
                )],
                Timestamp::from_unix_ms(1).expect("ts"),
                None,
                ProviderIds::empty(),
                Metadata::empty(),
            )
            .expect("message")]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_024,
                max_input_tokens: 1_024,
                max_output_tokens: 128,
            },
        },
        continuation_state: None,
    }
}

fn expected_ollama() -> ModelTerminal {
    let mut assembly = OllamaChatAssembly::new(REQUEST_ID.to_owned(), false, None);
    for line in [
        r#"{"message":{"content":"hello "},"done":false}"#,
        r#"{"message":{"content":"world"},"done":false}"#,
        r#"{"message":{"content":""},"done":true,"prompt_eval_count":4,"eval_count":2}"#,
    ] {
        assembly.consume(line).expect("ollama line");
    }
    ModelTerminal::Completed(assembly.finish().expect("ollama complete"))
}

async fn serve_body(content_type: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let content_type = content_type.to_owned();
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
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

async fn assert_protocol(content_type: &str, body: &str, expected: ModelTerminal) {
    let (base, server) = serve_body(content_type, body).await;
    let model = provider(&base);
    let selected = model.descriptor().models[0].clone();
    check_model_conformance(
        &model,
        ModelConformanceCase {
            model: selected,
            request: request(),
            expected_terminal: expected,
            stream_limits: finstack_ai_runtime::ModelStreamLimits::default(),
        },
    )
    .await
    .expect("public Model contract");
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn ollama_chat_conformance() {
    assert_protocol("application/x-ndjson", OLLAMA_NDJSON, expected_ollama()).await;
}
