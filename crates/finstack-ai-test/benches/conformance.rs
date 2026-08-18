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
    CommittedBatch, ContentBlock, DeadlinePropagation, Decision, Digest, EffectId, EventId, Id,
    IdTag, Kernel, KernelInput, KernelState, LaneId, Message, MessageId, MessageRole, Metadata,
    ModelRequestId, OperationLocator, OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds,
    RAW_JSON_MAX_BYTES, RawJson, RecordDraft, RecordEnvelope, RecordId, ReducerStageOutcome,
    RunAccepted, RunId, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext,
    SessionId, Stage, StageCursor, StageSettled, TextBlock, Timestamp, ToolCallBlock,
    ToolCallIdentity, ToolCallTag, TransitionEnv, TurnId, TurnTag, Usage,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, InputCapabilities, Model, ModelCallContext,
    ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError, ModelEventStream,
    ModelName, ModelRequest, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamAssembler, ModelStreamItem, ModelStreamLimits, ModelTokenEstimate, PortFuture,
    RunCallContext, StructuredOutputCapability, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    ToolDeferralSupport, ToolError, ToolEventStream, ToolResult, ToolStreamAssembler,
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

fn bench_quick() -> bool {
    std::env::args().any(|argument| argument == "--quick")
}

fn bench_id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
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

fn tool_call_block(ordinal: u64) -> ToolCallBlock {
    ToolCallBlock::try_new(
        bench_id::<ToolCallTag>(ordinal),
        "lookup_price",
        RawJson::parse(format!(r#"{{"ordinal":{ordinal}}}"#)).expect("arguments"),
    )
    .expect("tool call")
}

/// Messages plus authored tool identities so `validate_tool_state` runs.
fn activated_state(tool_count: usize, message_count: usize) -> KernelState {
    let tools = (0..tool_count)
        .map(|index| {
            let ordinal = u64::try_from(index + 1).expect("ordinal");
            let call = tool_call_block(ordinal);
            let identity = ToolCallIdentity {
                cycle: 0,
                turn_id: bench_id::<TurnTag>(3),
                source_message_id: bench_id(1_000 + ordinal),
                tool_batch_id: None,
                effect_id: None,
                call: call.clone(),
            };
            (call, identity)
        })
        .collect::<Vec<_>>();
    let mut messages = (0..message_count).map(filler_message).collect::<Vec<_>>();
    for (call, identity) in &tools {
        messages.push(
            Message::try_new(
                identity.source_message_id,
                MessageRole::Assistant,
                vec![ContentBlock::ToolCall(call.clone())],
                Timestamp::from_unix_ms(2_000).expect("ts"),
                None,
                ProviderIds::empty(),
                Metadata::empty(),
            )
            .expect("authored"),
        );
    }
    KernelState {
        state_version: 2,
        messages: messages.into(),
        tool_calls: tools
            .into_iter()
            .map(|(call, identity)| (*call.tool_call_id(), identity))
            .collect(),
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

    let tool_counts: &[usize] = if bench_quick() {
        &[0, 16, 64]
    } else {
        &[0, 64, 256]
    };
    let message_counts: &[usize] = if bench_quick() {
        &[16, 128, 1_024]
    } else {
        &[16, 1_024, 4_096]
    };
    let (empty_kernel, accept_env, accept_input) = accept_run_input();
    let accept_decision = empty_kernel
        .decide(&accept_env, accept_input)
        .expect("accept decision");
    let accept_batch = commit_decision(&accept_decision);
    for &tools in tool_counts {
        for &messages in message_counts {
            let state = activated_state(tools, messages);
            state.validate().expect("activated state must validate");
            let label = format!("tools{tools}_messages{messages}");
            group.bench_with_input(
                BenchmarkId::new("activated_validate", &label),
                &state,
                |bencher, state| {
                    bencher.iter(|| state.validate().expect("validate"));
                },
            );
            group.bench_with_input(
                BenchmarkId::new("activated_state_hash", &label),
                &state,
                |bencher, state| {
                    bencher.iter(|| black_box(state.state_hash().expect("state hash")));
                },
            );
            group.bench_with_input(
                BenchmarkId::new("activated_apply_accept", &label),
                &state,
                |bencher, state| {
                    bencher.iter(|| {
                        let mut kernel =
                            Kernel::try_restore(black_box(state.clone())).expect("restore");
                        let events = kernel.apply(black_box(&accept_batch), 0);
                        black_box(events)
                    });
                },
            );
            let mut rejected = state.clone();
            rejected.last_applied_sequence = 32;
            group.bench_with_input(
                BenchmarkId::new("activated_apply_rollback", &label),
                &rejected,
                |bencher, state| {
                    bencher.iter(|| {
                        let mut kernel =
                            Kernel::try_restore(black_box(state.clone())).expect("restore");
                        let error = kernel
                            .apply(black_box(&accept_batch), 0)
                            .expect_err("sequence must fail");
                        black_box(error)
                    });
                },
            );
        }
    }
    let tool_state = activated_state(16, 128);
    group.bench_function("state_hash_v6_tools", |bencher| {
        bencher.iter(|| black_box(tool_state.state_hash().expect("v6 tools hash")));
    });

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
                .block_on(tool_assembler.assemble(
                    tool_stream(ITEMS),
                    None,
                    1_024,
                    ToolDeferralSupport::Never,
                ))
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
    kernel_micro_extras(&mut group);
    group.finish();
}

fn kernel_micro_extras(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
) {
    let json_64kib = json_object_bytes(64 * 1024);
    let json_1mib = json_object_bytes(RAW_JSON_MAX_BYTES);
    let json_map_64kib = json_map_bytes(64 * 1024);
    group.bench_function("raw_json_parse_64kib", |bencher| {
        bencher.iter(|| black_box(RawJson::parse(black_box(&json_64kib)).expect("64kib")));
    });
    group.bench_function("raw_json_parse_1mib", |bencher| {
        bencher.iter(|| black_box(RawJson::parse(black_box(&json_1mib)).expect("1mib")));
    });
    group.bench_function("raw_json_de_map_64kib", |bencher| {
        bencher.iter(|| black_box(RawJson::parse(black_box(&json_map_64kib)).expect("map")));
    });
    let result_ids: Arc<[MessageId]> = (0..64_u64).map(bench_id).collect::<Vec<_>>().into();
    group.bench_function("tool_result_ids_append", |bencher| {
        bencher.iter(|| {
            let mut copied = result_ids.to_vec();
            copied.push(bench_id(99));
            black_box(Arc::<[MessageId]>::from(copied));
        });
    });
    let (context_kernel, context_env, context_input) = preparing_context_kernel();
    group.bench_function("decide_context_prepared", |bencher| {
        bencher.iter(|| {
            let decision = context_kernel
                .decide(black_box(&context_env), black_box(context_input.clone()))
                .expect("decide context");
            black_box(decision);
        });
    });
    let context_decision = context_kernel
        .decide(&context_env, context_input.clone())
        .expect("context decision");
    let context_batch = commit_decision(&context_decision);
    group.bench_function("apply_context_prepared", |bencher| {
        bencher.iter(|| {
            let mut apply_kernel = context_kernel.clone();
            let events = apply_kernel
                .apply(black_box(&context_batch), 0)
                .expect("apply context");
            black_box(events);
        });
    });
}

fn json_object_bytes(target: usize) -> Vec<u8> {
    let overhead = 8;
    let pad = target.saturating_sub(overhead);
    let mut bytes = Vec::with_capacity(target);
    bytes.extend_from_slice(br#"{"d":""#);
    bytes.extend(std::iter::repeat_n(b'a', pad));
    bytes.extend_from_slice(br#""}"#);
    bytes
}

fn json_map_bytes(target: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(target);
    bytes.push(b'{');
    let mut index = 0_u32;
    while bytes.len() + 24 < target {
        if index > 0 {
            bytes.push(b',');
        }
        bytes.extend(format!(r#""k{index}":{index}"#).into_bytes());
        index += 1;
    }
    bytes.push(b'}');
    bytes
}

fn apply_input(kernel: &mut Kernel, env: &TransitionEnv, input: KernelInput) {
    let decision = kernel.decide(env, input).expect("decide");
    let batch = commit_decision(&decision);
    kernel.apply(&batch, 0).expect("apply");
}

fn empty_ids(records: Vec<RecordId>, events: Vec<EventId>, turns: Vec<TurnId>) -> AllocatedIds {
    AllocatedIds::try_new(
        records,
        events,
        vec![],
        vec![],
        vec![],
        turns,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    )
    .expect("ids")
}

fn preparing_context_kernel() -> (Kernel, TransitionEnv, KernelInput) {
    let (mut kernel, env, input) = accept_run_input();
    apply_input(&mut kernel, &env, input);
    apply_input(
        &mut kernel,
        &TransitionEnv {
            now: Timestamp::from_unix_ms(1_100).expect("now"),
            ids: empty_ids(
                vec![RecordId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("record")],
                vec![],
                vec![],
            ),
        },
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    );
    let context_env = TransitionEnv {
        now: Timestamp::from_unix_ms(1_200).expect("now"),
        ids: empty_ids(
            vec![
                RecordId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("record"),
                RecordId::parse("01234567-89ab-7cde-89ab-0123456789b3").expect("record"),
            ],
            vec![],
            vec![TurnId::parse("01234567-89ab-7cde-89ab-0123456789b4").expect("turn")],
        ),
    };
    let context_input = KernelInput::StageSettled(StageSettled {
        cursor: StageCursor {
            cycle: 0,
            stage: Stage::PrepareContext,
        },
        outcome: ReducerStageOutcome::ContextPrepared {
            messages: Arc::from([filler_message(0)]),
        },
    });
    (kernel, context_env, context_input)
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
