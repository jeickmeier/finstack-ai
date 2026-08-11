//! Criterion microbenchmarks for conformance and PR-009 reducer hot paths.
//!
//! The reducer group executes a deterministic scripted model trace with no
//! provider, network, storage, or runtime latency.
//!
//! Sampling is configured for regression gating rather than a quick smoke
//! reading: the previous 20-sample/1-second settings produced confidence
//! intervals wide enough to report "improved" and "regressed" on an unchanged
//! binary, which makes the numbers unusable as a merge gate.
//!
//! The `state_scaling` group varies message count so growth-shaped regressions
//! (anything quadratic in conversation length) show up as a changing slope
//! rather than hiding inside a single point measurement.

use std::collections::VecDeque;
use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    ContentBlock, KernelState, Message, MessageId, MessageRole, Metadata, ProviderIds, RawJson,
    TextBlock, Timestamp, Usage,
};
use finstack_ai_runtime::{
    ModelError, ModelEventStream, ModelResponse, ModelStreamAssembler, ModelStreamItem,
    ModelStreamLimits, TextDelta, ToolError, ToolEventStream, ToolResult, ToolStreamAssembler,
    ToolStreamItem, UsageDelta,
};
use finstack_ai_test::{
    ConformanceRunner, NoOpRustAdapter, ReducerRustAdapter, compare_normalized_bytes,
    compatibility_fixture, execute_reducer_trace, load_golden_trace, load_noop_trace,
    normalize_json_value,
};
use futures_core::Stream;

/// Sample count giving a tight enough interval to gate on.
const GATE_SAMPLE_SIZE: usize = 100;
/// Measurement window per benchmark.
const GATE_MEASUREMENT: Duration = Duration::from_secs(5);
/// Warm-up window per benchmark.
const GATE_WARM_UP: Duration = Duration::from_secs(1);

fn configure(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    group.sample_size(GATE_SAMPLE_SIZE);
    group.warm_up_time(GATE_WARM_UP);
    group.measurement_time(GATE_MEASUREMENT);
}

fn conformance_noop(c: &mut Criterion) {
    let mut group = c.benchmark_group("conformance_noop");
    configure(&mut group);

    group.bench_function("load_normalize_compare", |bencher| {
        bencher.iter(|| {
            let trace = load_noop_trace().expect("load noop");
            let value = serde_json::to_value(&trace).expect("serialize");
            let bytes = normalize_json_value(&value);
            compare_normalized_bytes(&value, &value).expect("compare");
            black_box(bytes);
        });
    });

    group.bench_function("runner_noop_adapter", |bencher| {
        let runner = ConformanceRunner::new();
        let adapter = NoOpRustAdapter;
        bencher.iter(|| {
            let report = runner
                .run_trace(&load_noop_trace().expect("load"), &adapter)
                .expect("run");
            assert!(report.passed);
            black_box(report);
        });
    });

    group.finish();
}

fn scripted_model_reducer(c: &mut Criterion) {
    let mut group = c.benchmark_group("scripted_model_reducer");
    configure(&mut group);

    let trace = load_golden_trace(compatibility_fixture(
        "golden-trace/v1/trace/valid--pr009-model-completed.json",
    ))
    .expect("load PR-009 reducer trace");
    let runner = ConformanceRunner::new();
    let adapter = ReducerRustAdapter;
    group.bench_function("execute_model_only_completion", |bencher| {
        bencher.iter(|| {
            let execution =
                execute_reducer_trace(black_box(&trace)).expect("execute reducer trace");
            black_box(execution);
        });
    });
    group.bench_function("runner_model_only_completion", |bencher| {
        bencher.iter(|| {
            let report = runner
                .run_trace(black_box(&trace), &adapter)
                .expect("run reducer trace");
            assert!(report.passed);
            black_box(report);
        });
    });

    group.finish();
}

/// Deterministic filler message; ordinal keeps ids and text distinct.
fn filler_message(ordinal: usize) -> Message {
    let ordinal64 = u64::try_from(ordinal).expect("ordinal fits u64");
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[10..].copy_from_slice(&ordinal64.to_be_bytes()[2..]);
    Message::try_new(
        MessageId::from_bytes(bytes),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new(format!("message body {ordinal}")).expect("text"),
        )],
        Timestamp::from_unix_ms(1_000 + i64::try_from(ordinal).expect("ordinal fits i64"))
            .expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn state_with_messages(count: usize) -> KernelState {
    KernelState {
        messages: (0..count).map(filler_message).collect::<Vec<_>>().into(),
        ..KernelState::default()
    }
}

/// Growth-shaped benchmarks over conversation length.
///
/// Both operations run on every committed batch, so any regression that scales
/// with message count compounds across a run. Compare the ratio between sizes,
/// not just the absolute numbers.
fn state_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_scaling");
    configure(&mut group);
    // Larger inputs need proportionally fewer iterations to stay in budget.
    group.sample_size(50);

    for count in [16_usize, 128, 1_024] {
        let state = state_with_messages(count);
        group.bench_with_input(
            BenchmarkId::new("state_hash", count),
            &state,
            |bencher, state| {
                bencher.iter(|| black_box(state.state_hash().expect("state hash")));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("validate", count),
            &state,
            |bencher, state| {
                bencher.iter(|| {
                    state.validate().expect("validate");
                });
            },
        );
    }

    group.finish();
}

struct ReadyStream<T, E> {
    items: VecDeque<Result<T, E>>,
}

impl<T, E> Stream for ReadyStream<T, E> {
    type Item = Result<T, E>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().items.pop_front())
    }
}

impl<T, E> Unpin for ReadyStream<T, E> {}

fn completed_model(text: &str) -> ModelResponse {
    ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new(text).expect("text"))]),
        tool_calls: Arc::from([]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("benchmark-completion"),
        continuation_state: None,
    }
}

fn model_stream(item_count: usize) -> ModelEventStream {
    let mut items = (0..item_count)
        .map(|_| {
            Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from("abcdefgh"),
            }))
        })
        .collect::<VecDeque<Result<_, ModelError>>>();
    items.push_back(Ok(ModelStreamItem::Completed(completed_model(
        &"abcdefgh".repeat(item_count),
    ))));
    Box::pin(ReadyStream { items })
}

fn tool_stream(item_count: usize) -> ToolEventStream {
    let mut items = (0..item_count)
        .map(|_| {
            Ok(ToolStreamItem::Usage(UsageDelta {
                usage: Usage::empty(),
            }))
        })
        .collect::<VecDeque<Result<_, ToolError>>>();
    items.push_back(Ok(ToolStreamItem::Completed(ToolResult {
        output: RawJson::parse(br#"{"ok":true}"#).expect("tool result"),
        is_error: false,
    })));
    Box::pin(ReadyStream { items })
}

fn stream_throughput(c: &mut Criterion) {
    const ITEMS: usize = 256;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("benchmark runtime");

    let mut model_group = c.benchmark_group("model_stream_throughput");
    configure(&mut model_group);
    let model_assembler =
        ModelStreamAssembler::new(ModelStreamLimits::default()).expect("model stream assembler");
    model_group.throughput(criterion::Throughput::Elements(ITEMS as u64));
    model_group.bench_function("assemble_256_text_items", |bencher| {
        bencher.iter(|| {
            let assembled = runtime
                .block_on(model_assembler.assemble(model_stream(ITEMS)))
                .expect("model stream");
            black_box(assembled);
        });
    });
    model_group.finish();

    let mut tool_group = c.benchmark_group("tool_stream_throughput");
    configure(&mut tool_group);
    let tool_assembler = ToolStreamAssembler::default();
    tool_group.throughput(criterion::Throughput::Elements(ITEMS as u64));
    tool_group.bench_function("assemble_256_usage_items", |bencher| {
        bencher.iter(|| {
            let assembled = runtime
                .block_on(tool_assembler.assemble(tool_stream(ITEMS), None, 1_024))
                .expect("tool stream");
            black_box(assembled);
        });
    });
    tool_group.finish();
}

criterion_group!(
    benches,
    conformance_noop,
    scripted_model_reducer,
    state_scaling,
    stream_throughput
);
criterion_main!(benches);
