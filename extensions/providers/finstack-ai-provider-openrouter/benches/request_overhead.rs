//! Request-build, SSE parse+assembly, and catalog-parse benchmarks.
//!
//! Requires the `bench` feature (never enabled by default), which exposes
//! `finstack_ai_provider_openrouter::bench_support` — a `#[doc(hidden)]`
//! module re-exporting the crate-private request/stream internals so these
//! benches can exercise them directly instead of only the full HTTP
//! round-trip.

use std::collections::BTreeMap;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    ContentBlock, Message, MessageId, MessageRole, Metadata, OutputSpec, ProviderIds, RawJson,
    RetrySafety, TextBlock, Timestamp, ToolExecutionMode, ToolId,
};
use finstack_ai_provider_openrouter::bench_support::{
    CompletionAssembly, ResponsesRequest, SseParser, serialize_request,
};
use finstack_ai_provider_openrouter::{OpenRouterModelConfig, model_configs_from_catalog_json};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ModelName, ModelRequestDraft, ModelRequestLimits,
    ModelSettings, SideEffectClass, ToolDeferralSupport, ToolSpec,
};

fn request_build(criterion: &mut Criterion) {
    let model = bench_model();
    let draft = bench_draft();

    criterion.bench_function("openrouter/request-build", |bencher| {
        bencher.iter(|| {
            let request = ResponsesRequest::try_from_draft(
                std::hint::black_box(&draft),
                std::hint::black_box(&model),
                None,
                &BTreeMap::new(),
            )
            .expect("request");
            std::hint::black_box(serialize_request(&request).expect("serialize"));
        });
    });
}

fn sse_parse_and_assemble(criterion: &mut Criterion) {
    let chunks = bench_stream_chunks();

    criterion.bench_function("openrouter/sse-parse-and-assemble", |bencher| {
        bencher.iter(|| {
            let mut parser = SseParser::new(1_048_576, 8 * 1_048_576);
            let mut assembly = CompletionAssembly::new("bench-request".to_owned(), false);
            for chunk in &chunks {
                let events = parser
                    .push(std::hint::black_box(chunk.as_bytes()))
                    .expect("push");
                for data in events {
                    std::hint::black_box(assembly.consume(&data).expect("consume"));
                }
            }
        });
    });
}

fn catalog_parse(criterion: &mut Criterion) {
    let body = bench_catalog_json();

    criterion.bench_function("openrouter/catalog-parse", |bencher| {
        bencher.iter(|| {
            std::hint::black_box(
                model_configs_from_catalog_json(std::hint::black_box(body.as_bytes()), 1_000_000)
                    .expect("catalog"),
            );
        });
    });
}

fn bench_model() -> OpenRouterModelConfig {
    OpenRouterModelConfig::try_new("openai/gpt-bench", 1_000_000, 400_000, 128_000, 32_000, 256)
        .expect("model")
}

fn bench_draft() -> ModelRequestDraft {
    let mut messages = Vec::with_capacity(7);
    messages.push(text_message(
        MessageRole::System,
        "You are a precise financial research assistant. Cite sources. Be brief.",
    ));
    let turns = [
        (MessageRole::User, "What moved oil prices this week?"),
        (
            MessageRole::Assistant,
            "OPEC+ signaled a modest supply increase, and inventories rose more than expected.",
        ),
        (MessageRole::User, "How does that compare to last quarter?"),
        (
            MessageRole::Assistant,
            "Prices are down roughly 8% quarter over quarter on softer demand forecasts.",
        ),
        (MessageRole::User, "Summarize the three biggest risks."),
        (
            MessageRole::Assistant,
            "Demand softness in Asia, OPEC+ compliance, and US shale supply elasticity.",
        ),
    ];
    for (role, text) in turns {
        messages.push(text_message(role, text));
    }

    ModelRequestDraft {
        model: ModelName::try_new("openai/gpt-bench").expect("model name"),
        messages: Arc::from(messages),
        tools: Arc::from([
            bench_tool("lookup_price_series", br#"{"type":"object","properties":{"symbol":{"type":"string"}},"required":["symbol"]}"#),
            bench_tool("lookup_news", br#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]}"#),
            bench_tool("compute_variance", br#"{"type":"object","properties":{"series":{"type":"array","items":{"type":"number"}}},"required":["series"]}"#),
        ]),
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(
                br#"{"provider":{"order":["openai","anthropic"],"allow_fallbacks":true,"data_collection":"deny"},"temperature":0.7,"top_p":0.9}"#,
            )
            .expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 1_000_000,
            max_input_tokens: 100_000,
            max_output_tokens: 4_096,
        },
    }
}

fn text_message(role: MessageRole, text: &str) -> Message {
    Message::try_new(
        MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
        role,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        Timestamp::from_unix_ms(0).expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn bench_tool(name: &str, schema: &[u8]) -> ToolSpec {
    ToolSpec {
        id: ToolId::parse("finstack.tools.bench").expect("tool id"),
        model_name: Arc::from(name),
        title: Arc::from("Bench tool"),
        description: Arc::from("A deterministic benchmark fixture tool."),
        input_schema: RawJson::parse(schema).expect("schema"),
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

/// A recorded (synthetic but shaped like a real trace) multi-event
/// `OpenRouter` Responses SSE stream: several text deltas, a tool call
/// (added + argument deltas), then `response.completed`.
fn bench_stream_chunks() -> Vec<String> {
    vec![
        sse(r#"{"type":"response.output_text.delta","sequence_number":1,"delta":"Oil "}"#),
        sse(r#"{"type":"response.output_text.delta","sequence_number":2,"delta":"prices "}"#),
        sse(r#"{"type":"response.output_text.delta","sequence_number":3,"delta":"moved "}"#),
        sse(r#"{"type":"response.output_text.delta","sequence_number":4,"delta":"lower "}"#),
        sse(r#"{"type":"response.output_text.delta","sequence_number":5,"delta":"this "}"#),
        sse(r#"{"type":"response.output_text.delta","sequence_number":6,"delta":"week."}"#),
        sse(
            r#"{"type":"response.output_item.added","sequence_number":7,"output_index":1,"item":{"type":"function_call","call_id":"call_bench","name":"lookup_price_series","arguments":""}}"#,
        ),
        sse(
            r#"{"type":"response.function_call_arguments.delta","sequence_number":8,"output_index":1,"delta":"{\"symbol\":"}"#,
        ),
        sse(
            r#"{"type":"response.function_call_arguments.delta","sequence_number":9,"output_index":1,"delta":"\"CL1\"}"}"#,
        ),
        sse(
            r#"{"type":"response.completed","sequence_number":10,"response":{"id":"resp-bench","output":[{"type":"message"},{"arguments":"{\"symbol\":\"CL1\"}","call_id":"call_bench","name":"lookup_price_series","type":"function_call"}],"usage":{"input_tokens":42,"output_tokens":18,"total_tokens":60}}}"#,
        ),
    ]
}

fn sse(data: &str) -> String {
    format!("data: {data}\n\n")
}

/// A synthetic `GET /api/v1/models` catalog body with ~200 entries, shaped
/// like a real `OpenRouter` response (context window, completion ceiling,
/// supported parameters, and input modalities all vary per entry).
fn bench_catalog_json() -> String {
    let mut entries = Vec::with_capacity(200);
    for index in 0..200 {
        let context_length = 32_768 + (index % 8) * 32_768;
        let max_completion_tokens = 4_096 + (index % 4) * 4_096;
        let supported_parameters = match index % 3 {
            0 => r#"["tools","reasoning","structured_outputs"]"#,
            1 => r#"["tools"]"#,
            _ => r#"["temperature"]"#,
        };
        let modalities = if index % 2 == 0 {
            r#"["text","image"]"#
        } else {
            r#"["text"]"#
        };
        entries.push(format!(
            r#"{{"id":"vendor-{index}/model-{index}","name":"Vendor {index} Model {index}","context_length":{context_length},"pricing":{{"prompt":"0.000001","completion":"0.00001"}},"top_provider":{{"max_completion_tokens":{max_completion_tokens}}},"supported_parameters":{supported_parameters},"architecture":{{"input_modalities":{modalities}}}}}"#
        ));
    }
    format!(r#"{{"data":[{}]}}"#, entries.join(","))
}

criterion_group!(
    benches,
    request_build,
    sse_parse_and_assemble,
    catalog_parse
);
criterion_main!(benches);
