//! Growth-shaped session and tool-settlement probes the empty-state gates miss.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use finstack_ai::Session;
use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendBatchId, BudgetPropagation, CancellationPropagation,
    CommittedBatch, ContentBlock, DeadlinePropagation, Decision, Digest, EffectCompleted, EffectId,
    EffectOutputContract, EffectOutputKind, EventId, Id, IdTag, Kernel, KernelInput, Message,
    MessageId, MessageRole, Metadata, ModelRequestId, ModelSettled, ModelSettlement,
    PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, RecordEnvelope, RecordId,
    ReducerStageOutcome, RetrySafety, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
    RunSecurityContext, Stage, StageCursor, StageSettled, TextBlock, Timestamp,
    ToolBatchContinuation, ToolBatchSettled, ToolBatchTag, ToolCallBlock, ToolCallPlan,
    ToolExecutionMode, ToolFailurePolicy, ToolId, ToolResultBlock, ToolSettlement, TransitionEnv,
    TurnId, ValidatedToolCall,
};
use finstack_ai_runtime::JournalStore;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_store_sqlite::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteJournalStore, SqliteStoreConfig,
    SqliteStoreLimits,
};
use tempfile::TempDir;

fn bench_quick() -> bool {
    std::env::args().any(|argument| argument == "--quick")
}

fn configure(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    if bench_quick() {
        group.sample_size(10);
        group.warm_up_time(Duration::from_millis(100));
        group.measurement_time(Duration::from_millis(200));
    } else {
        group.sample_size(30);
        group.warm_up_time(Duration::from_secs(1));
        group.measurement_time(Duration::from_secs(3));
    }
}

fn bench_id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn session_histories() -> &'static [usize] {
    if bench_quick() {
        &[16, 128, 1_024]
    } else {
        &[16, 128, 1_024, 10_000]
    }
}

fn memory_store(records: usize) -> Arc<dyn JournalStore> {
    // Criterion warmup/measurement can append tens of thousands of times once
    // the live session head is incremental. Keep a large headroom above the
    // seeded history so the probe measures append cost, not store limits.
    let ceiling = records.saturating_add(1_048_576).max(8);
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: ceiling,
            records_per_session: ceiling,
            snapshot_bytes: 64 * 1024,
        })
        .expect("memory store"),
    )
}

fn sqlite_store(dir: &TempDir, records: usize) -> Arc<dyn JournalStore> {
    let ceiling = records.saturating_add(1_048_576).max(8);
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: dir.path().join("session.sqlite"),
            durability: SqliteDurability::Relaxed {
                synchronous: finstack_ai_store_sqlite::SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 4,
                batches_per_session: ceiling,
                records_per_session: ceiling,
                snapshot_bytes: 256 * 1024,
            },
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("sqlite store"),
    )
}

async fn seed_session(store: Arc<dyn JournalStore>, prior_records: usize) -> Session {
    let session = Session::create(store, "tenant-bench")
        .await
        .expect("create session");
    let lane = session.lane("main").await.expect("main lane");
    // SessionCreated + LaneCreated already occupy two records.
    let mut remaining = prior_records.saturating_sub(2);
    let mut ordinal = 0_usize;
    while remaining > 0 {
        lane.append_text(&format!("prior {ordinal}"))
            .await
            .expect("seed append");
        remaining = remaining.saturating_sub(2);
        ordinal += 1;
    }
    session
}

fn session_append_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("session_append_scaling");
    configure(&mut group);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");

    for &history in session_histories() {
        let memory = runtime.block_on(seed_session(memory_store(history + 8), history));
        let memory_lane = runtime.block_on(memory.lane("main")).expect("lane");
        group.bench_with_input(
            BenchmarkId::new("memory", history),
            &history,
            |bencher, _| {
                bencher.iter(|| {
                    runtime
                        .block_on(memory_lane.append_text("structural append"))
                        .expect("memory append");
                });
            },
        );

        let dir = TempDir::new().expect("tempdir");
        let sqlite = runtime.block_on(seed_session(sqlite_store(&dir, history + 8), history));
        let sqlite_lane = runtime.block_on(sqlite.lane("main")).expect("lane");
        group.bench_with_input(
            BenchmarkId::new("sqlite", history),
            &history,
            |bencher, _| {
                bencher.iter(|| {
                    runtime
                        .block_on(sqlite_lane.append_text("structural append"))
                        .expect("sqlite append");
                });
            },
        );
        black_box(dir);
    }
    group.finish();
}

#[allow(clippy::too_many_arguments)]
fn empty_ids(
    records: Vec<RecordId>,
    events: Vec<EventId>,
    effects: Vec<EffectId>,
    messages: Vec<MessageId>,
    turns: Vec<TurnId>,
    requests: Vec<ModelRequestId>,
    batches: Vec<finstack_ai_kernel::ToolBatchId>,
    calls: Vec<finstack_ai_kernel::ToolCallId>,
) -> AllocatedIds {
    AllocatedIds::try_new(
        records,
        events,
        effects,
        vec![],
        messages,
        turns,
        requests,
        batches,
        calls,
        vec![],
        vec![],
    )
    .expect("ids")
}

fn env_at(ms: i64, ids: AllocatedIds) -> TransitionEnv {
    TransitionEnv {
        now: Timestamp::from_unix_ms(ms).expect("now"),
        ids,
    }
}

fn apply_input(kernel: &mut Kernel, env: &TransitionEnv, input: KernelInput) -> Decision {
    let decision = kernel.decide(env, input).expect("decide");
    let batch = commit_decision(&decision);
    kernel.apply(&batch, 0).expect("apply");
    decision
}

fn bag<T: IdTag>(start: u64, count: usize) -> Vec<Id<T>> {
    (0..u64::try_from(count).expect("count"))
        .map(|index| bench_id(start + index))
        .collect()
}

/// Exact bags for one tool completion: `EffectCompleted`, then any source-order
/// `ToolCallSettled` drain, then `EffectRequested` (next sequential group) or
/// `ToolBatchClosed` (batch complete). Event counts follow `derived_event_count`.
fn settlement_id_counts(
    sequential: bool,
    width: usize,
    source_index: usize,
    already_settled: &[bool],
) -> (usize, usize, usize) {
    if sequential {
        return if source_index + 1 == width {
            (3, 3, 1)
        } else {
            (3, 4, 1)
        };
    }
    let next_source = already_settled
        .iter()
        .position(|settled| !settled)
        .unwrap_or(width);
    if source_index != next_source {
        return (1, 1, 0);
    }
    let mut finalize = 1_usize;
    while next_source + finalize < width && already_settled[next_source + finalize] {
        finalize += 1;
    }
    let close = usize::from(next_source + finalize == width);
    (1 + finalize + close, 1 + 2 * finalize, finalize)
}

fn apply_settlement(
    kernel: &mut Kernel,
    now_ms: i64,
    id_base: u64,
    records: usize,
    events: usize,
    messages: usize,
    input: KernelInput,
) {
    apply_input(
        kernel,
        &env_at(
            now_ms,
            empty_ids(
                bag(id_base, records),
                bag(id_base + 1_024, events),
                vec![],
                bag(id_base + 2_048, messages),
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        ),
        input,
    );
}

fn commit_decision(decision: &Decision) -> CommittedBatch {
    let records = decision
        .records
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            let offset = u64::try_from(index).expect("index");
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
        .saturating_add(u64::try_from(records.len().saturating_sub(1)).expect("len"));
    CommittedBatch::try_new(
        AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789c0").expect("batch"),
        decision.expected_sequence,
        last_sequence,
        records,
    )
    .expect("committed")
}

fn root_acceptance() -> RunAccepted {
    let run_id = bench_id::<finstack_ai_kernel::RunTag>(3);
    RunAccepted::try_new(
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
    .expect("accepted")
}

fn stage(cycle: u64, stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(StageSettled {
        cursor: StageCursor { cycle, stage },
        outcome,
    })
}

fn model_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

fn tool_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ToolResult,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"tool_result"}"#),
    }
}

fn tool_call(ordinal: u64) -> ToolCallBlock {
    ToolCallBlock::try_new(
        bench_id(300 + ordinal),
        "lookup_price",
        RawJson::parse(format!(r#"{{"ordinal":{ordinal}}}"#)).expect("args"),
    )
    .expect("call")
}

fn drive_to_awaiting_tools(
    width: usize,
    execution: ToolExecutionMode,
) -> (Kernel, Vec<ToolCallBlock>) {
    let (mut kernel, calls) = accept_through_tool_calls(width);
    open_tool_batch(&mut kernel, &calls, execution);
    (kernel, calls)
}

fn accept_through_tool_calls(width: usize) -> (Kernel, Vec<ToolCallBlock>) {
    let mut kernel = accept_to_before_model();
    let calls = complete_model_with_calls(&mut kernel, width);
    (kernel, calls)
}

#[allow(clippy::too_many_lines)]
fn accept_to_before_model() -> Kernel {
    let mut kernel = Kernel::default();
    apply_input(
        &mut kernel,
        &env_at(
            1_000,
            empty_ids(
                vec![bench_id(1)],
                vec![bench_id(1)],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        ),
        KernelInput::AcceptRun(AcceptRun {
            session_id: bench_id::<finstack_ai_kernel::SessionTag>(1),
            lane_id: bench_id::<finstack_ai_kernel::LaneTag>(2),
            accepted: root_acceptance(),
        }),
    );
    apply_input(
        &mut kernel,
        &env_at(
            1_100,
            empty_ids(
                vec![bench_id(2)],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        ),
        stage(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
    );
    apply_input(
        &mut kernel,
        &env_at(
            1_200,
            empty_ids(
                vec![bench_id(3), bench_id(4)],
                vec![],
                vec![],
                vec![],
                vec![bench_id::<finstack_ai_kernel::TurnTag>(101)],
                vec![],
                vec![],
                vec![],
            ),
        ),
        stage(
            0,
            Stage::PrepareContext,
            ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([Message::try_new(
                    bench_id::<finstack_ai_kernel::MessageTag>(4),
                    MessageRole::User,
                    vec![ContentBlock::Text(
                        TextBlock::try_new("call tools").expect("text"),
                    )],
                    Timestamp::from_unix_ms(900).expect("ts"),
                    None,
                    ProviderIds::empty(),
                    Metadata::empty(),
                )
                .expect("user")]),
            },
        ),
    );
    apply_input(
        &mut kernel,
        &env_at(
            1_300,
            empty_ids(
                vec![bench_id(5), bench_id(6)],
                vec![bench_id(2)],
                vec![bench_id::<finstack_ai_kernel::EffectTag>(103)],
                vec![],
                vec![],
                vec![bench_id::<finstack_ai_kernel::ModelRequestTag>(102)],
                vec![],
                vec![],
            ),
        ),
        stage(
            0,
            Stage::BeforeModel,
            ReducerStageOutcome::ModelRequestPrepared {
                request: RawJson::parse(r#"{"messages":[{"role":"user","text":"call tools"}]}"#)
                    .expect("request"),
                component: None,
                output_contract: model_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
        ),
    );
    kernel
}

fn complete_model_with_calls(kernel: &mut Kernel, width: usize) -> Vec<ToolCallBlock> {
    let calls = (0..width)
        .map(|index| tool_call(u64::try_from(index + 1).expect("call")))
        .collect::<Vec<_>>();
    let mut content = vec![ContentBlock::Text(
        TextBlock::try_new("calling tools").expect("text"),
    )];
    content.extend(calls.iter().cloned().map(ContentBlock::ToolCall));
    let assistant = Message::try_new(
        bench_id::<finstack_ai_kernel::MessageTag>(104),
        MessageRole::Assistant,
        content,
        Timestamp::from_unix_ms(1_400).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant");
    let completion = EffectCompleted::try_new(
        bench_id::<finstack_ai_kernel::EffectTag>(103),
        model_contract(),
        RawJson::parse(r#"{"text":"calling tools"}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("model-tools"),
        None,
    )
    .expect("completion");
    apply_input(
        kernel,
        &TransitionEnv {
            now: Timestamp::from_unix_ms(1_400).expect("now"),
            ids: empty_ids(
                vec![bench_id(7), bench_id(8)],
                vec![bench_id(3), bench_id(4)],
                vec![],
                vec![bench_id(104)],
                vec![],
                vec![],
                vec![],
                calls.iter().map(|call| *call.tool_call_id()).collect(),
            ),
        },
        KernelInput::ModelSettled(ModelSettled {
            turn_id: bench_id(101),
            model_request_id: bench_id(102),
            outcome: ModelSettlement::Completed {
                completion,
                assistant_message: assistant,
            },
        }),
    );
    apply_input(
        kernel,
        &TransitionEnv {
            now: Timestamp::from_unix_ms(1_500).expect("now"),
            ids: empty_ids(
                vec![bench_id(9)],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        },
        stage(0, Stage::AfterModel, ReducerStageOutcome::Continue),
    );
    calls
}

fn open_tool_batch(kernel: &mut Kernel, calls: &[ToolCallBlock], execution: ToolExecutionMode) {
    let plans = calls
        .iter()
        .map(|call| {
            ToolCallPlan::Execute(ValidatedToolCall {
                call: call.clone(),
                tool_id: ToolId::parse("finstack.tools.fixture").expect("tool"),
                component: None,
                output_contract: tool_contract(),
                retry_safety: RetrySafety::IdempotentWithKey,
                deadline: None,
                execution,
                failure_policy: ToolFailurePolicy::ReturnToModel,
            })
        })
        .collect::<Vec<_>>();
    let width64 = u64::try_from(calls.len()).expect("width");
    let first_group = match execution {
        ToolExecutionMode::Parallel => width64,
        ToolExecutionMode::Sequential | ToolExecutionMode::Barrier => 1,
    };
    apply_input(
        kernel,
        &env_at(
            1_600,
            empty_ids(
                (0..2 + first_group)
                    .map(|index| bench_id(1_000 + index))
                    .collect(),
                (0..first_group)
                    .map(|index| bench_id(1_000 + index))
                    .collect(),
                (0..width64)
                    .map(|index| bench_id::<finstack_ai_kernel::EffectTag>(401 + index))
                    .collect(),
                vec![],
                vec![],
                vec![],
                vec![bench_id::<ToolBatchTag>(400)],
                vec![],
            ),
        ),
        stage(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: plans.into(),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
}

fn settle_all(kernel: &mut Kernel, calls: &[ToolCallBlock], order: &[usize], sequential: bool) {
    let width = calls.len();
    let mut already_settled = vec![false; width];
    for (step, &index) in order.iter().enumerate() {
        let call = &calls[index];
        let ordinal = u64::try_from(index + 1).expect("ord");
        let step64 = u64::try_from(step).expect("step");
        let (records, events, messages) =
            settlement_id_counts(sequential, width, index, &already_settled);
        already_settled[index] = true;
        let effect = bench_id::<finstack_ai_kernel::EffectTag>(401 + ordinal - 1);
        let result = ToolResultBlock::try_new(
            *call.tool_call_id(),
            vec![ContentBlock::Text(
                TextBlock::try_new(format!("result-{ordinal}")).expect("text"),
            )],
            false,
        )
        .expect("result");
        let completion = EffectCompleted::try_new(
            effect,
            tool_contract(),
            RawJson::parse(serde_json::to_string(&result).expect("json")).expect("raw"),
            None,
            vec![],
            ProviderIds::empty(),
            Some(format!("tool-completion-{ordinal}")),
            None,
        )
        .expect("tool completion");
        apply_settlement(
            kernel,
            1_700 + i64::try_from(step).expect("step"),
            3_000 + step64 * 4_096,
            records,
            events,
            messages,
            KernelInput::ToolBatchSettled(ToolBatchSettled {
                tool_batch_id: bench_id(400),
                outcome: ToolSettlement::Completed(completion),
            }),
        );
    }
}

fn tool_settlement_width(c: &mut Criterion) {
    let mut group = c.benchmark_group("tool_settlement_width");
    configure(&mut group);
    let widths = if bench_quick() {
        vec![1_usize, 16]
    } else {
        // The parallel batch-open decision emits two lifecycle records plus
        // one effect record per tool, so 254 is the largest width that stays
        // within the 256-record append bound (and also remains below the
        // completion-identity bound once the model completion is included).
        vec![1, 16, 64, 254]
    };
    for width in widths {
        for (mode, execution, reverse) in [
            ("sequential", ToolExecutionMode::Sequential, false),
            ("parallel", ToolExecutionMode::Parallel, false),
            ("reverse", ToolExecutionMode::Parallel, true),
        ] {
            let (kernel, calls) = drive_to_awaiting_tools(width, execution);
            let order = if reverse {
                (0..width).rev().collect::<Vec<_>>()
            } else {
                (0..width).collect::<Vec<_>>()
            };
            let sequential = execution == ToolExecutionMode::Sequential;
            group.bench_with_input(BenchmarkId::new(mode, width), &width, |bencher, _| {
                bencher.iter(|| {
                    let mut clone = kernel.clone();
                    settle_all(&mut clone, black_box(&calls), black_box(&order), sequential);
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, session_append_scaling, tool_settlement_width);
criterion_main!(benches);
