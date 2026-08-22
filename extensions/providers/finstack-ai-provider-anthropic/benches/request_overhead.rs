//! Warm pooled-client request-overhead benchmark.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    EffectId, LaneId, Metadata, ModelRequestId, OperationLocator, OutputSpec, PrincipalRef,
    RawJson, RunId, SessionId,
};
use finstack_ai_provider_anthropic::{AnthropicConfig, AnthropicModelConfig, AnthropicProvider};
use finstack_ai_runtime::ports::model::{
    AuthorizationContext, CancellationSignal, Model, ModelCallContext, ModelRequest,
    ModelRequestDraft, ModelRequestLimits, ModelSettings, RunCallContext,
};
use futures_util::StreamExt;

const RESPONSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"bench-1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

fn request_overhead(criterion: &mut Criterion) {
    let base_url = start_keep_alive_server();
    let provider = AnthropicProvider::try_new(
        AnthropicConfig::try_new(base_url).expect("config"),
        vec![
            AnthropicModelConfig::try_new("bench-model", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model"),
        ],
    )
    .expect("provider");
    let request = benchmark_request();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    criterion.bench_function("anthropic/warm-pooled-client-round-trip", |bencher| {
        bencher.to_async(&runtime).iter(|| async {
            let mut stream = provider
                .request(std::hint::black_box(request.clone()))
                .await
                .expect("request");
            while let Some(item) = stream.next().await {
                std::hint::black_box(item.expect("stream item"));
            }
        });
    });
}

fn benchmark_request() -> ModelRequest {
    ModelRequest {
        call: ModelCallContext {
            run: RunCallContext {
                locator: OperationLocator::try_new(
                    "bench",
                    SessionId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("session"),
                    LaneId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("lane"),
                    RunId::parse("01234567-89ab-7cde-89ab-0123456789b3").expect("run"),
                )
                .expect("locator"),
                authorization: AuthorizationContext {
                    principal: PrincipalRef::try_new("bench", "bench", Some("bench"))
                        .expect("principal"),
                    authentication_method: Arc::from("benchmark"),
                    assurance_level: Arc::from("benchmark"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([Arc::from("bench")]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("benchmark-v1"),
                    decision_id: Arc::from("benchmark-v1"),
                },
                effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789b4").expect("effect"),
                attempt: 1,
                deadline: None,
                budget_scope_id: None,
                cancellation: CancellationSignal::new(),
                relation_depth: 0,
            },
            request_id: ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789b5")
                .expect("request"),
        },
        draft: ModelRequestDraft {
            model: finstack_ai_runtime::ports::model::ModelName::try_new("bench-model")
                .expect("model"),
            messages: Arc::from([]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 128,
            },
        },
        continuation_state: None,
    }
}

fn start_keep_alive_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("benchmark listener");
    let address = listener.local_addr().expect("listener address");
    thread::spawn(move || {
        for socket in listener.incoming() {
            let mut socket = socket.expect("benchmark connection");
            let mut reader = BufReader::new(socket.try_clone().expect("clone connection"));
            loop {
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).expect("request line") == 0 {
                    break;
                }
                let mut content_length = 0_usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("request header");
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line
                        .to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                    {
                        content_length = value;
                    }
                }
                let mut body = vec![0_u8; content_length];
                reader.read_exact(&mut body).expect("request body");
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                    RESPONSE.len()
                );
                socket.write_all(headers.as_bytes()).expect("headers");
                socket.write_all(RESPONSE.as_bytes()).expect("body");
                socket.flush().expect("flush");
            }
        }
    });
    format!("http://{address}")
}

criterion_group!(benches, request_overhead);
criterion_main!(benches);
