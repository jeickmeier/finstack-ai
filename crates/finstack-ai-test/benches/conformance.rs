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
    AcceptRun, AllocatedIds, AppendBatchId, BudgetPropagation, CancellationPropagation,
    CommittedBatch, ContentBlock, DeadlinePropagation, Decision, Digest, EffectId, EventId, Kernel,
    KernelInput, KernelState, LaneId, Message, MessageId, MessageRole, Metadata, ModelRequestId,
    OperationLocator, OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson,
    RecordDraft, RecordEnvelope, RecordId, RunAccepted, RunId, RunLimits, RunPropagationPolicy,
    RunRelation, RunSecurityContext, SessionId, TextBlock, Timestamp, TransitionEnv, Usage,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, InputCapabilities, Model, ModelCallContext,
    ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError, ModelEventStream,
    ModelName, ModelRequest, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamAssembler, ModelStreamItem, ModelStreamLimits, ModelTokenEstimate, PortFuture,
    RunCallContext, StructuredOutputCapability, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    ToolError, ToolEventStream, ToolResult, ToolStreamAssembler, ToolStreamItem, UsageDelta,
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

fn accept_run_input() -> (Kernel, TransitionEnv, KernelInput) {
    let run_id = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run");
    let accepted = RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("root"),
        RunSecurityContext::try_new(
            "tenant",
            PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"agent":"bench"}"#),
        None,
    )
    .expect("accepted");
    let env = TransitionEnv {
        now: Timestamp::from_unix_ms(1_000).expect("now"),
        ids: AllocatedIds::try_new(
            vec![RecordId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("record")],
            vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("event")],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .expect("ids"),
    };
    let input = KernelInput::AcceptRun(AcceptRun {
        session_id: SessionId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("session"),
        lane_id: LaneId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("lane"),
        accepted,
    });
    (Kernel::default(), env, input)
}

fn kernel_micro(c: &mut Criterion) {
    let mut group = c.benchmark_group("kernel_micro");
    configure(&mut group);
    group.bench_function("kernel_default_init", |bencher| {
        bencher.iter(|| black_box(Kernel::default()));
    });
    let (kernel, env, input) = accept_run_input();
    group.bench_function("decide_accept_run", |bencher| {
        bencher.iter(|| {
            let decision = kernel
                .decide(black_box(&env), black_box(input.clone()))
                .expect("decide");
            black_box(decision);
        });
    });
    group.bench_function("raw_json_parse_small", |bencher| {
        bencher.iter(|| {
            black_box(RawJson::parse(br#"{"a":1,"b":2}"#).expect("json"));
        });
    });
    let decision = kernel
        .decide(&env, input.clone())
        .expect("decide accept-run for apply setup");
    let batch = commit_decision(&decision);
    group.bench_function("apply_accept_run", |bencher| {
        bencher.iter(|| {
            let mut apply_kernel = Kernel::default();
            let events = apply_kernel
                .apply(black_box(&batch), 0)
                .expect("apply accept-run");
            black_box(events);
        });
    });
    group.bench_function("try_restore_default", |bencher| {
        let state = KernelState::default();
        bencher.iter(|| {
            black_box(Kernel::try_restore(black_box(state.clone())).expect("restore"));
        });
    });
    group.bench_function("record_draft_json_roundtrip", |bencher| {
        let draft = decision
            .records
            .first()
            .expect("accept-run decision has a record")
            .clone();
        bencher.iter(|| {
            let encoded = serde_json::to_vec(black_box(&draft)).expect("encode");
            let decoded: RecordDraft = serde_json::from_slice(&encoded).expect("decode");
            black_box(decoded);
        });
    });
    group.bench_function("message_history_view", |bencher| {
        let state = state_with_messages(128);
        bencher.iter(|| {
            let view: usize = state
                .messages
                .iter()
                .map(|message| message.content().len())
                .sum();
            black_box(view);
        });
    });
    group.finish();
}

fn commit_decision(decision: &Decision) -> CommittedBatch {
    let records = decision
        .records
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            let offset = u64::try_from(index).expect("index fits u64");
            let sequence = decision.expected_sequence.saturating_add(offset);
            RecordEnvelope::try_new(
                draft.format_version(),
                draft.kind_version(),
                draft.record_id(),
                draft.session_id(),
                draft.lane_id(),
                draft.run_id(),
                sequence,
                draft.timestamp(),
                Some(Timestamp::from_unix_ms(1_001).expect("committed_at")),
                Digest::raw_json(b"payload"),
                None,
                Digest::raw_json(b"checksum"),
                draft.derived_event_ids().to_vec(),
                draft.body().clone(),
            )
            .expect("envelope")
        })
        .collect::<Vec<_>>();
    let last_sequence = decision
        .expected_sequence
        .saturating_add(u64::try_from(records.len().saturating_sub(1)).expect("len fits u64"));
    CommittedBatch::try_new(
        AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("batch"),
        decision.expected_sequence,
        last_sequence,
        records,
    )
    .expect("committed batch")
}

struct InstantModel;

impl Model for InstantModel {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            provider: Arc::from("instant"),
            models: Arc::from([ModelName::try_new("instant-1").expect("model")]),
            metadata: Metadata::empty(),
        }
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: false,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: ModelContextProfile {
                provider: Arc::from("instant"),
                model: ModelName::try_new("instant-1").expect("model"),
                hard_input_bytes: 1_024,
                context_window_tokens: 1_024,
                max_output_tokens: 16,
                reserved_output_tokens: 8,
                provider_overhead_tokens: 0,
                estimator: TokenEstimatorRef {
                    id: Arc::from("instant.bytes"),
                    version: Arc::from("1"),
                    source: TokenEstimatorSource::ConservativeUpperBound,
                },
            },
            native_tool_calls: false,
            parallel_tool_calls: false,
            structured_output: StructuredOutputCapability::Unsupported,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: true,
            native_capabilities: std::collections::BTreeSet::new(),
        }
    }

    fn estimate_input_tokens(
        &self,
        _model: &ModelName,
        _canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        Ok(ModelTokenEstimate {
            input_tokens: 1,
            estimator: TokenEstimatorRef {
                id: Arc::from("instant.bytes"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        })
    }

    fn request(&self, _request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        Box::pin(async {
            Ok(Box::pin(ReadyStream {
                items: VecDeque::from([Ok(ModelStreamItem::Completed(completed_model("ok")))]),
            }) as ModelEventStream)
        })
    }
}

fn instant_model_request() -> ModelRequest {
    let principal = PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal");
    ModelRequest {
        call: ModelCallContext {
            run: RunCallContext {
                locator: OperationLocator::try_new(
                    "tenant",
                    SessionId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("session"),
                    LaneId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("lane"),
                    RunId::parse("01234567-89ab-7cde-89ab-0123456789b3").expect("run"),
                )
                .expect("locator"),
                authorization: AuthorizationContext {
                    principal,
                    authentication_method: Arc::from("local"),
                    assurance_level: Arc::from("test"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([Arc::from("tenant")]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("policy-v1"),
                    decision_id: Arc::from("decision-v1"),
                },
                effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789b4").expect("effect"),
                attempt: 1,
                deadline: None,
                budget_scope_id: None,
                cancellation: CancellationSignal::new(),
            },
            request_id: ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789b5")
                .expect("request"),
        },
        draft: ModelRequestDraft {
            model: ModelName::try_new("instant-1").expect("model"),
            messages: Arc::from([]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_024,
                max_input_tokens: 1_024,
                max_output_tokens: 16,
            },
        },
        continuation_state: None,
    }
}

fn resolved_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolved_dispatch");
    configure(&mut group);
    let model: Arc<dyn Model> = Arc::new(InstantModel);
    group.bench_function("resolved_handle_descriptor", |bencher| {
        bencher.iter(|| black_box(model.descriptor()));
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("dispatch runtime");
    let request = instant_model_request();
    group.bench_function("instant_model_request_poll", |bencher| {
        bencher.iter(|| {
            let item = runtime.block_on(async {
                let mut stream = model
                    .request(black_box(request.clone()))
                    .await
                    .expect("dispatch");
                std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await
            });
            black_box(item);
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    conformance_noop,
    scripted_model_reducer,
    state_scaling,
    stream_throughput,
    kernel_micro,
    resolved_dispatch
);
criterion_main!(benches);
