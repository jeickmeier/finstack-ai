//! PR-015 provider-neutral Model port and runtime acceptance proofs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancelRequested,
    CancellationInitiator, CancellationPropagation, CancellationRequestTag, CommittedBatch,
    ComponentId, ContentBlock, Digest, Duration as KernelDuration, EffectInput,
    EffectOutputContract, EffectOutputKind, ErrorCategory, ExternalHandleRef, Id, IdTag,
    KernelInput, LaneTag, Message, MessageRole, Metadata, OperationLocator, OutputSpec,
    PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, ReconciliationPolicy,
    ReducerStageOutcome, RetryClassification, RetryDirective, RetrySafety, RunAccepted,
    RunEventBody, RunEventKind, RunLimits, RunPhase, RunPropagationPolicy, RunRelation,
    RunSecurityContext, Sensitivity, SessionTag, Stage, StageCursor, TextBlock, Timestamp,
    TransitionEnv, Usage,
};
use finstack_ai_kernel::{KernelState, RecordBody};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, CommitCoordinator, CommitCoordinatorError,
    EventBatchConfig, EventFilter, EventHubConfig, EventLagPolicy, EventSubscriptionCloseReason,
    EventSubscriptionConfig, IdGenerationError, JournalStore, LoadRequest, LoadedSession,
    LockedModelContextProfile, MODEL_RECONCILIATION_UNSUPPORTED, MODEL_RESPONSE_MISMATCH,
    MODEL_STREAM_DUPLICATE_COMPLETION, MODEL_STREAM_ERROR_AFTER_COMPLETION,
    MODEL_STREAM_ITEM_AFTER_COMPLETION, MODEL_STREAM_MISSING_COMPLETION,
    MODEL_TOOL_CALL_ARGUMENTS_INVALID, MODEL_TOOL_CALL_DELTA_INVALID, MODEL_TOOL_CALL_INCOMPLETE,
    MODEL_USAGE_INVALID, ManualDriveAction, Model, ModelCallContext, ModelContextProfile,
    ModelDeferral, ModelError, ModelProgress, ModelReconcileResult, ModelRequest,
    ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelResumeAction, ModelSettings,
    ModelStreamAssembler, ModelStreamItem, ModelStreamLimits, ModelTaskConfig, ModelTerminal,
    ModelToolCall, ModelWarmupContext, OpaqueProviderEvent, PortFuture, ProgressCoalescing,
    RandomSource, ReasoningDelta, RunCallContext, RunHandle, RunHandleError, RunStatus,
    RunTaskConfig, RunTaskOwner, ShutdownOutcome, SnapshotReceipt, SnapshotRequest, StoreError,
    StoreHealth, TextDelta, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, UsageDelta,
    model_resume_action, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    FixedClock, ScriptedInput, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedStep,
    ScriptedStepKind,
};

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: finstack_ai_runtime::ModelName::try_new("scripted-1").expect("model"),
        hard_input_bytes: 2_000_000,
        context_window_tokens: 3_000_000,
        max_output_tokens: 1_000,
        reserved_output_tokens: 1_000,
        provider_overhead_tokens: 0,
        estimator: TokenEstimatorRef {
            id: Arc::from("scripted-bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn locked_profile() -> LockedModelContextProfile {
    resolve_model_context_profile(profile(), None, None, false).expect("locked profile")
}

fn draft(messages: Arc<[Message]>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: profile().model,
        messages,
        tools: Arc::from([]),
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 2_000_000,
            max_input_tokens: 2_000_000,
            max_output_tokens: 1_000,
        },
    }
}

fn request(cancellation: CancellationSignal) -> ModelRequest {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    ModelRequest {
        call: ModelCallContext {
            run: RunCallContext {
                locator: OperationLocator::try_new(
                    "tenant-a",
                    id::<SessionTag>(1),
                    id::<LaneTag>(2),
                    id(3),
                )
                .expect("locator"),
                authorization: AuthorizationContext {
                    principal,
                    authentication_method: Arc::from("oidc"),
                    assurance_level: Arc::from("high"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("policy-v1"),
                    decision_id: Arc::from("decision-v1"),
                },
                effect_id: id(4),
                attempt: 1,
                deadline: None,
                budget_scope_id: None,
                cancellation,
            },
            request_id: id(5),
        },
        draft: draft(Arc::from([])),
        continuation_state: None,
    }
}

fn completed(text: &str) -> ModelResponse {
    ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new(text).expect("text"))]),
        tool_calls: Arc::from([]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::try_new(None::<&str>, Some("response-1"), None::<&str>)
            .expect("provider ids"),
        completion_id: Arc::from("completion-1"),
        continuation_state: None,
    }
}

fn chunks(count: usize, text: &str) -> ScriptedInput {
    let mut steps = (0..count)
        .map(|index| {
            let start = index * text.len() / count;
            let end = (index + 1) * text.len() / count;
            ScriptedStep {
                kind: ScriptedStepKind::ModelChunk,
                id: Some(format!("chunk-{index}")),
                text: Some(text[start..end].to_owned()),
                tool_name: None,
                arguments: None,
                result: None,
                error_code: None,
                message: None,
                duration_ms: None,
                payload_declaration: None,
            }
        })
        .collect::<Vec<_>>();
    steps.push(ScriptedStep {
        kind: ScriptedStepKind::ModelCompleted,
        id: Some("completion-1".into()),
        text: None,
        tool_name: None,
        arguments: None,
        result: None,
        error_code: None,
        message: None,
        duration_ms: None,
        payload_declaration: None,
    });
    ScriptedInput {
        format_version: 1,
        steps,
    }
}

async fn assemble_plan(plan: ScriptedModelPlan) -> Result<ModelTerminal, ModelError> {
    let model = ScriptedModel::from_plans(profile(), vec![plan]);
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    ModelStreamAssembler::new(ModelStreamLimits::default())
        .expect("assembler")
        .assemble(stream)
        .await
        .map(|value| value.terminal)
}

#[tokio::test]
async fn one_ten_hundred_and_thousand_chunks_assemble_identically() {
    let text = "x".repeat(10_000);
    let mut terminals = Vec::new();
    for count in [1, 10, 100, 1_000] {
        let model = ScriptedModel::from_inputs(profile(), vec![chunks(count, &text)]);
        let stream = model
            .request(request(CancellationSignal::new()))
            .await
            .expect("stream");
        let assembled = ModelStreamAssembler::new(ModelStreamLimits::default())
            .expect("assembler")
            .assemble(stream)
            .await
            .expect("assembled");
        terminals.push(assembled.terminal);
    }
    assert!(terminals.windows(2).all(|pair| pair[0] == pair[1]));
}

#[tokio::test]
async fn one_warmed_scripted_handle_is_reused_for_multiple_requests() {
    let model =
        ScriptedModel::from_inputs(profile(), vec![chunks(1, "first"), chunks(1, "second")]);
    model
        .warmup(ModelWarmupContext {
            cancellation: CancellationSignal::new(),
            deadline: None,
            metadata: Metadata::empty(),
        })
        .await
        .expect("warmup");
    let assembler = ModelStreamAssembler::new(ModelStreamLimits::default()).expect("assembler");
    for expected in ["first", "second"] {
        let stream = model
            .request(request(CancellationSignal::new()))
            .await
            .expect("stream");
        let result = assembler.assemble(stream).await.expect("assembled");
        let ModelTerminal::Completed(response) = result.terminal else {
            panic!("expected completion");
        };
        assert_eq!(
            response.assistant_content.as_ref(),
            [ContentBlock::Text(
                TextBlock::try_new(expected).expect("text")
            )]
        );
    }
    assert_eq!(model.warmup_count(), 1);
    assert_eq!(model.request_count(), 2);
    assert_eq!(model.active_stream_count(), 0);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the legal transition proof keeps every normalized stream item and final aggregate visible"
)]
async fn legal_progress_tool_usage_completion_and_deferral_transitions_are_normalized() {
    let final_usage =
        Usage::try_new(Some(1), Some(1), Some(2), None, BTreeMap::new()).expect("final usage");
    let response = ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new("ok").expect("text"))]),
        tool_calls: Arc::from([
            ModelToolCall {
                name: Arc::from("calc"),
                arguments: RawJson::parse(br#"{"a":1}"#).expect("arguments"),
            },
            ModelToolCall {
                name: Arc::from("lookup"),
                arguments: RawJson::parse(br#"{"x":1}"#).expect("arguments"),
            },
        ]),
        usage: final_usage.clone(),
        provider_ids: ProviderIds::try_new(None::<&str>, Some("response-legal"), None::<&str>)
            .expect("provider ids"),
        completion_id: Arc::from("completion-legal"),
        continuation_state: Some(RawJson::parse(br#"{"cursor":"next"}"#).expect("state")),
    };
    let plan = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from("ok"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ReasoningDelta(ReasoningDelta {
                text: Arc::from("private"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 2,
                name: Some(Arc::from("calc")),
                arguments_delta: Arc::from("{\"a\":"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("lookup")),
                arguments_delta: Arc::from("{\"x\":"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 2,
                name: None,
                arguments_delta: Arc::from("1}"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: None,
                arguments_delta: Arc::from("1}"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta {
                usage: Usage::try_new(Some(1), Some(0), Some(1), None, BTreeMap::new())
                    .expect("usage"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta {
                usage: final_usage,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Heartbeat(Metadata::empty()))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ProviderEvent(OpaqueProviderEvent {
                namespace: Arc::from("scripted.raw"),
                payload: RawJson::parse(br#"{"kind":"debug"}"#).expect("event"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response.clone()))),
        ],
    };
    let model = ScriptedModel::from_plans(profile(), vec![plan]);
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    let assembled = ModelStreamAssembler::new(ModelStreamLimits::default())
        .expect("assembler")
        .assemble(stream)
        .await
        .expect("assembled");
    assert_eq!(
        assembled.progress.as_ref(),
        [
            ModelProgress::Text(Arc::from("ok")),
            ModelProgress::Reasoning(Arc::from("private")),
            ModelProgress::Heartbeat(Metadata::empty()),
        ]
    );
    assert_eq!(assembled.terminal, ModelTerminal::Completed(response));

    let provider = ComponentId::parse("finstack.model.scripted").expect("component");
    let deferral = ModelDeferral {
        handle: ExternalHandleRef::try_new(
            provider,
            "wait-1",
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::Poll,
        next_poll_at: Some(timestamp(2_500)),
        expires_at: Some(timestamp(5_000)),
    };
    assert_eq!(
        assemble_plan(ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
                deferral.clone(),
            )))],
        })
        .await
        .expect("deferred"),
        ModelTerminal::Deferred(deferral)
    );
}

#[tokio::test]
async fn malformed_terminal_transitions_have_exact_stable_codes() {
    let complete = ModelStreamItem::Completed(completed("a"));
    let cases = [
        (vec![], MODEL_STREAM_MISSING_COMPLETION),
        (
            vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(complete.clone())),
                ScriptedModelAction::Emit(Ok(complete.clone())),
            ],
            MODEL_STREAM_DUPLICATE_COMPLETION,
        ),
        (
            vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(complete.clone())),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Heartbeat(Metadata::empty()))),
            ],
            MODEL_STREAM_ITEM_AFTER_COMPLETION,
        ),
        (
            vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(complete.clone())),
                ScriptedModelAction::Emit(Err(ModelError::try_new(
                    "provider_error",
                    finstack_ai_kernel::ErrorCategory::Model,
                    false,
                    "late provider error",
                    Metadata::empty(),
                )
                .expect("error"))),
            ],
            MODEL_STREAM_ERROR_AFTER_COMPLETION,
        ),
    ];
    for (actions, code) in cases {
        let error = assemble_plan(ScriptedModelPlan { actions })
            .await
            .expect_err(code);
        assert_eq!(error.code(), code);
        assert_eq!(
            error.category(),
            finstack_ai_kernel::ErrorCategory::Validation
        );
        assert!(!error.retryable());
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the malformed transition table keeps exact error classifications visible"
)]
async fn malformed_tool_usage_and_response_sequences_are_fail_closed() {
    let incomplete = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: None,
                arguments_delta: Arc::from("{}"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
        ],
    };
    assert_eq!(
        assemble_plan(incomplete)
            .await
            .expect_err("incomplete")
            .code(),
        MODEL_TOOL_CALL_INCOMPLETE
    );

    let invalid_arguments = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("lookup")),
                arguments_delta: Arc::from("{"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
        ],
    };
    assert_eq!(
        assemble_plan(invalid_arguments)
            .await
            .expect_err("arguments")
            .code(),
        MODEL_TOOL_CALL_ARGUMENTS_INVALID
    );

    let mutated_name = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("first")),
                arguments_delta: Arc::from("{"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("second")),
                arguments_delta: Arc::from("}"),
            }))),
        ],
    };
    assert_eq!(
        assemble_plan(mutated_name)
            .await
            .expect_err("mutated tool name")
            .code(),
        MODEL_TOOL_CALL_DELTA_INVALID
    );

    let first = Usage::try_new(Some(2), Some(2), Some(4), None, BTreeMap::new()).expect("usage");
    let second = Usage::try_new(Some(1), Some(2), Some(3), None, BTreeMap::new()).expect("usage");
    let usage_regression = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta { usage: first }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta { usage: second }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
        ],
    };
    assert_eq!(
        assemble_plan(usage_regression)
            .await
            .expect_err("usage")
            .code(),
        MODEL_USAGE_INVALID
    );

    let mismatch = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from("stream"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("final")))),
        ],
    };
    assert_eq!(
        assemble_plan(mismatch).await.expect_err("mismatch").code(),
        MODEL_RESPONSE_MISMATCH
    );

    let model = ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("a")))),
            ],
        }],
    );
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    let error = ModelStreamAssembler::new(ModelStreamLimits {
        max_items: 1,
        max_bytes: 32,
        max_tool_calls: 1,
    })
    .expect("assembler")
    .assemble(stream)
    .await
    .expect_err("item limit");
    assert_eq!(
        error.code(),
        finstack_ai_runtime::MODEL_STREAM_LIMIT_EXCEEDED
    );
    assert_eq!(error.category(), finstack_ai_kernel::ErrorCategory::Limit);
    assert!(!error.retryable());
}

#[tokio::test]
async fn named_block_acknowledges_exact_effect_cancellation_without_sleep() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Block(Arc::from("slow")),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
            ],
        }],
    ));
    let control = model.control();
    let cancellation = CancellationSignal::new();
    let stream = model
        .request(request(cancellation.clone()))
        .await
        .expect("stream");
    let assembler = ModelStreamAssembler::new(ModelStreamLimits::default()).expect("assembler");
    let task = tokio::spawn(async move { assembler.assemble(stream).await });
    while control.entries("slow") == 0 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();
    let error = task.await.expect("join").expect_err("cancelled");
    assert_eq!(error.code(), "model_cancelled");
    assert_eq!(model.cancellation_acknowledgement_count(), 1);
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
}

#[tokio::test]
async fn dropping_a_slow_stream_consumer_releases_the_stream() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::BlockUninterruptibly(Arc::from(
                "consumer-drop",
            ))],
        }],
    ));
    let control = model.control();
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    let assembler = ModelStreamAssembler::new(ModelStreamLimits::default()).expect("assembler");
    let task = tokio::spawn(async move { assembler.assemble(stream).await });
    while control.entries("consumer-drop") == 0 {
        tokio::task::yield_now().await;
    }
    task.abort();
    assert!(
        task.await
            .expect_err("consumer task aborted")
            .is_cancelled()
    );
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
}

struct CounterRandom(AtomicU64);

impl RandomSource for CounterRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let value = self.0.fetch_add(1, Ordering::AcqRel).to_be_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = value[index % value.len()];
        }
        Ok(())
    }
}

struct FailFourthAppendStore {
    inner: MemoryJournalStore,
    appends: AtomicUsize,
}

impl FailFourthAppendStore {
    fn new() -> Self {
        Self {
            inner: MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 1,
                batches_per_session: 16,
                records_per_session: 32,
                snapshot_bytes: 1_024,
            })
            .expect("store"),
            appends: AtomicUsize::new(0),
        }
    }
}

impl JournalStore for FailFourthAppendStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let call = self.appends.fetch_add(1, Ordering::AcqRel) + 1;
        if call == 4 {
            return Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "model_request_append_failed",
                })
            });
        }
        self.inner.append(request)
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.inner.load(request)
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        self.inner.write_snapshot(request)
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        self.inner.health()
    }
}

#[allow(clippy::too_many_arguments)]
fn env(
    now: i64,
    records: &[u64],
    events: &[u64],
    effects: &[u64],
    turns: &[u64],
    model_requests: &[u64],
    messages: &[u64],
    append_batch: u64,
) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            records.iter().copied().map(id).collect(),
            events.iter().copied().map(id).collect(),
            effects.iter().copied().map(id).collect(),
            Vec::new(),
            messages.iter().copied().map(id).collect(),
            turns.iter().copied().map(id).collect(),
            model_requests.iter().copied().map(id).collect(),
            Vec::new(),
            Vec::new(),
            vec![id(append_batch)],
            Vec::new(),
        )
        .expect("ids"),
    }
}

fn accepted() -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
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
            deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(b"agent"),
        None,
    )
    .expect("accepted")
}

fn user_message() -> Message {
    Message::try_new(
        id(44),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

async fn drive_to_active_model_request(handle: &RunHandle) {
    drive_to_active_model_request_with(handle, RetrySafety::SafeToRetry).await;
}

async fn drive_to_active_model_request_with(handle: &RunHandle, retry_safety: RetrySafety) {
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: raw,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety,
                    deadline: Some(timestamp(5_000)),
                },
            ),
        )
        .await
        .expect("model request");
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the proof keeps live progress, lag closure, finalization, and journal recovery contiguous"
)]
async fn runtime_publishes_validated_progress_before_terminal_settlement() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("early progress"),
                }))),
                ScriptedModelAction::Block(Arc::from("terminal-gate")),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(
                    "early progress",
                )))),
            ],
        }],
    ));
    let control = model.control();
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 4,
                max_subscribers: 2,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(500)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    let mut subscription = handle
        .subscribe_events(EventSubscriptionConfig {
            queue_capacity: 2,
            filter: EventFilter {
                include_durable: false,
                include_transient: true,
                kinds: Arc::from([RunEventKind::ModelTextDelta]),
                max_sensitivity: Sensitivity::Confidential,
            },
            batching: EventBatchConfig {
                flush_count: 8,
                flush_bytes: 64 * 1_024,
                flush_interval: StdDuration::from_secs(1),
            },
            progress_coalescing: ProgressCoalescing::Disabled,
            lag_policy: EventLagPolicy::BlockBounded {
                timeout: StdDuration::from_millis(100),
            },
        })
        .await
        .expect("subscription");
    let mut stalled_durable = handle
        .subscribe_events(EventSubscriptionConfig {
            queue_capacity: 1,
            filter: EventFilter {
                include_durable: true,
                include_transient: false,
                kinds: Arc::from([]),
                max_sensitivity: Sensitivity::Confidential,
            },
            batching: EventBatchConfig {
                flush_count: 1,
                flush_bytes: 64 * 1_024,
                flush_interval: StdDuration::from_secs(1),
            },
            progress_coalescing: ProgressCoalescing::Enabled,
            lag_policy: EventLagPolicy::DropProgress {
                durable_timeout: StdDuration::from_millis(1),
            },
        })
        .await
        .expect("stalled durable subscription");

    drive_to_active_model_request(&handle).await;
    while control.entries("terminal-gate") == 0 {
        tokio::task::yield_now().await;
    }
    let batch = subscription.next_batch().await.expect("progress batch");
    assert_eq!(batch.events().len(), 1);
    let RunEventBody::ModelTextDelta(delta) = batch.events()[0].body() else {
        panic!("expected model text progress");
    };
    assert_eq!(delta.text(), "early progress");
    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover before terminal");
    assert!(recovered.state().pending_model_effect.is_some());
    assert!(recovered.state().model_settlements.is_empty());

    control.release("terminal-gate");
    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover after terminal");
        if recovered.state().model_settlements.len() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    handle
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    handle
        .submit(
            env(2_200, &[8, 9], &[3], &[], &[], &[], &[], 106),
            stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        )
        .await
        .expect("finalize");
    let recovered = CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover completed run");
    assert!(recovered.state().terminal.is_some());
    assert_eq!(
        stalled_durable.status().close_reason,
        Some(EventSubscriptionCloseReason::MissedDurable)
    );
    let delivered_before_lag = stalled_durable
        .next_batch()
        .await
        .expect("first durable batch");
    assert!(
        !matches!(
            delivered_before_lag
                .events()
                .last()
                .map(finstack_ai_kernel::RunEvent::kind),
            Some(RunEventKind::RunCompleted | RunEventKind::RunFailed | RunEventKind::RunCancelled)
        ),
        "the terminal event was not delivered to the disconnected subscriber"
    );
    assert!(stalled_durable.next_batch().await.is_none());
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[derive(Debug, PartialEq, Eq)]
struct RuntimeProjection {
    record_bodies: Vec<Vec<u8>>,
    assistant_message: Vec<u8>,
    state_hash: Digest,
    warmups: usize,
    requests: usize,
    status: RunStatus,
}

#[expect(
    clippy::too_many_lines,
    reason = "the acceptance helper keeps the complete commit-before-dispatch lifecycle visible"
)]
async fn run_runtime_chunks(count: usize, response_text: &str) -> RuntimeProjection {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_inputs(
        profile(),
        vec![chunks(count, response_text)],
    ));
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(1)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let request_draft = draft(Arc::from([message]));
    let raw =
        RawJson::parse(request_draft.canonical_bytes().expect("canonical")).expect("raw request");
    handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: raw,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(5_000)),
                },
            ),
        )
        .await
        .expect("model request");

    while model.request_count() == 0 {
        tokio::task::yield_now().await;
    }
    let (assistant_message, state_hash) = loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().messages.len() == 1 {
            assert!(recovered.state().pending_model_effect.is_none());
            let message = &recovered.state().messages[0];
            assert_eq!(
                message
                    .content()
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.text()),
                        _ => None,
                    })
                    .collect::<String>(),
                response_text
            );
            break (
                serde_json::to_vec(message).expect("message bytes"),
                recovered.state().state_hash().expect("state hash"),
            );
        }
        tokio::task::yield_now().await;
    };
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("loaded");
    let record_bodies = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(|record| serde_json::to_vec(record.body()).expect("record body"))
        .collect();
    let warmups = model.warmup_count();
    let requests = model.request_count();
    owner.shutdown().await;
    RuntimeProjection {
        record_bodies,
        assistant_message,
        state_hash,
        warmups,
        requests,
        status: handle.status(),
    }
}

#[tokio::test]
async fn runtime_warms_reuses_and_settles_only_after_committed_dispatch() {
    let projection = run_runtime_chunks(10, "runtime response").await;
    assert_eq!(projection.warmups, 1);
    assert_eq!(projection.requests, 1);
    assert_eq!(projection.status, RunStatus::Stopped);
}

#[tokio::test]
async fn durable_runtime_outcome_is_identical_for_all_required_chunk_counts() {
    let response = "x".repeat(10_000);
    let baseline = run_runtime_chunks(1, &response).await;
    for count in [10, 100, 1_000] {
        assert_eq!(run_runtime_chunks(count, &response).await, baseline);
    }
}

#[tokio::test]
async fn failed_model_request_append_never_executes_the_model() {
    let store = Arc::new(FailFourthAppendStore::new());
    let model = Arc::new(ScriptedModel::from_inputs(
        profile(),
        vec![chunks(1, "must not execute")],
    ));
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(200)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    let error = handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: raw,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(5_000)),
                },
            ),
        )
        .await
        .expect_err("append failure");
    assert!(matches!(
        error,
        RunHandleError::Coordinator(CommitCoordinatorError::Store(StoreError::Unavailable {
            reason_code: "model_request_append_failed"
        }))
    ));
    assert_eq!(model.warmup_count(), 1);
    assert_eq!(model.request_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn malformed_stream_settles_as_failure_without_partial_durable_success() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("streamed"),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("different")))),
            ],
        }],
    ));
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(250)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;

    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().model_settlements.len() == 1 {
            assert!(recovered.state().pending_model_effect.is_none());
            assert!(recovered.state().messages.is_empty());
            assert!(recovered.state().completion_identities.is_empty());
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.active_stream_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn an_expired_committed_deadline_prevents_provider_execution() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_inputs(
        profile(),
        vec![chunks(1, "too late")],
    ));
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(6_000)),
        CounterRandom(AtomicU64::new(275)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;

    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().model_settlements.len() == 1 {
            assert!(recovered.state().pending_model_effect.is_none());
            assert!(recovered.state().messages.is_empty());
            assert!(recovered.state().completion_identities.is_empty());
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(model.request_count(), 0);
    assert_eq!(model.active_stream_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the restart acceptance keeps the pre-crash and recovered owner states visible"
)]
async fn persisted_retry_timer_resumes_once_after_runtime_restart() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 32,
            records_per_session: 64,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let retryable = ModelError::try_new(
        "temporary_model_failure",
        ErrorCategory::Model,
        true,
        "temporary model failure",
        Metadata::empty(),
    )
    .expect("retryable error");
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable))],
        }],
    ));
    let model_port: Arc<dyn Model> = model;
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(400)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;
    let settled = tokio::time::timeout(StdDuration::from_secs(1), async {
        loop {
            let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
                .await
                .expect("recover failure");
            if recovered.state().phase == Some(RunPhase::BeforeFinalize) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if settled.is_err() {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover diagnostic");
        panic!(
            "model failure did not settle; status={:?}; phase={:?}; settlements={}",
            handle.status(),
            recovered.state().phase,
            recovered.state().model_settlements.len()
        );
    }
    handle
        .submit(
            env(2_300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                ReducerStageOutcome::Retry(
                    RetryDirective::try_new(
                        RetryClassification::Model,
                        KernelDuration::from_millis(10_000),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    let sleeping = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover sleeping");
    assert_eq!(sleeping.state().phase, Some(RunPhase::Sleeping));
    assert_eq!(sleeping.state().retry.attempts, 1);
    assert_eq!(
        sleeping
            .state()
            .retry
            .pending
            .as_ref()
            .expect("timer")
            .attempt,
        1
    );
    tokio::time::timeout(StdDuration::from_secs(1), owner.shutdown())
        .await
        .expect("first owner shutdown");

    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover runtime");
    let replacement: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(profile(), Vec::new()));
    let mut replacement_owner = RunTaskOwner::spawn_with_model(
        recovered,
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        replacement,
        locked_profile(),
        FixedClock::new(timestamp(20_000)),
        CounterRandom(AtomicU64::new(500)),
    )
    .await
    .expect("replacement owner");
    tokio::time::timeout(StdDuration::from_secs(1), async {
        loop {
            let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
                .await
                .expect("recover fired timer");
            if recovered.state().phase == Some(RunPhase::PreparingContext) {
                assert_eq!(recovered.state().retry.attempts, 1);
                assert!(recovered.state().retry.pending.is_none());
                assert_eq!(recovered.state().retry.timer_firings.len(), 1);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timer did not fire; status={:?}",
            replacement_owner.handle().status()
        )
    });
    assert_eq!(
        replacement_owner.handle().timer_diagnostics().already_due,
        1
    );
    tokio::time::timeout(StdDuration::from_secs(1), replacement_owner.shutdown())
        .await
        .expect("replacement owner shutdown");
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the cancellation acceptance keeps exact committed identities and signalling visible"
)]
async fn runtime_routes_cancel_effect_to_only_the_active_model_task() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Block(Arc::from("runtime-slow"))],
        }],
    ));
    let control = model.control();
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(100)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: raw,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(5_000)),
                },
            ),
        )
        .await
        .expect("model request");
    while control.entries("runtime-slow") == 0 {
        tokio::task::yield_now().await;
    }

    let cancel_env = TransitionEnv {
        now: timestamp(1_400),
        ids: AllocatedIds::try_new(
            vec![id(7)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(105)],
            vec![id::<CancellationRequestTag>(700)],
        )
        .expect("cancel ids"),
    };
    handle
        .submit(
            cancel_env,
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("test cancellation")),
            }),
        )
        .await
        .expect("cancel");
    while model.cancellation_acknowledgement_count() == 0 {
        tokio::task::yield_now().await;
    }
    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover cancellation reconciliation");
        if recovered.state().phase == Some(RunPhase::Cancelled) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.cancellation_acknowledgement_count(), 1);
    let report = owner.shutdown().await;
    assert_eq!(report.outcome, ShutdownOutcome::Graceful);
    assert_eq!(handle.status(), RunStatus::Stopped);
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
    let recovered = CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover cancelled run");
    assert!(recovered.state().messages.is_empty());
    assert!(recovered.state().completion_identities.is_empty());
}

#[tokio::test]
async fn shutdown_aborts_an_uncooperative_model_only_after_its_grace_deadline() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::BlockUninterruptibly(Arc::from(
                "forced-abort",
            ))],
        }],
    ));
    let control = model.control();
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(10),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(300)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;
    while control.entries("forced-abort") == 0 {
        tokio::task::yield_now().await;
    }

    let report = owner.shutdown().await;

    assert_eq!(handle.status(), RunStatus::Stopped);
    assert_eq!(report.outcome, ShutdownOutcome::Forced);
    assert!(report.aborted_tasks > 0);
    assert_eq!(model.cancellation_acknowledgement_count(), 0);
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
}

#[tokio::test]
async fn repeated_model_runs_settle_and_shutdown_without_stream_or_task_leaks() {
    for ordinal in 0..24_u64 {
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 1,
                batches_per_session: 16,
                records_per_session: 32,
                snapshot_bytes: 1_024,
            })
            .expect("store"),
        );
        let model = Arc::new(ScriptedModel::from_inputs(
            profile(),
            vec![chunks(1, "stress completion")],
        ));
        let model_port: Arc<dyn Model> = model.clone();
        let mut owner = RunTaskOwner::spawn_with_model(
            CommitCoordinator::new(store.clone()),
            RunTaskConfig {
                command_capacity: 2,
                event_hub: EventHubConfig {
                    source_capacity: 4,
                    max_subscribers: 2,
                },
                shutdown_deadline: StdDuration::from_millis(250),
            },
            ModelTaskConfig {
                job_capacity: 1,
                result_capacity: 1,
                stream_limits: ModelStreamLimits::default(),
                warmup_deadline: None,
                warmup_metadata: Metadata::empty(),
            },
            model_port,
            locked_profile(),
            FixedClock::new(timestamp(2_000)),
            CounterRandom(AtomicU64::new(10_000 + ordinal)),
        )
        .await
        .expect("owner");
        let handle = owner.handle();
        drive_to_active_model_request(&handle).await;
        tokio::time::timeout(StdDuration::from_secs(1), async {
            loop {
                let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
                    .await
                    .expect("recover");
                if recovered.state().model_settlements.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("model settlement timeout");

        let report = owner.shutdown().await;

        assert_eq!(report.outcome, ShutdownOutcome::Graceful);
        assert_eq!(report.aborted_tasks, 0);
        assert_eq!(handle.status(), RunStatus::Stopped);
        assert_eq!(model.request_count(), 1);
        assert_eq!(model.active_stream_count(), 0);
    }
}

fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 32,
            records_per_session: 64,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    )
}

fn owner_run_config() -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: 4,
        event_hub: EventHubConfig {
            source_capacity: 16,
            max_subscribers: 8,
        },
        shutdown_deadline: StdDuration::from_millis(250),
    }
}

fn owner_model_config() -> ModelTaskConfig {
    ModelTaskConfig {
        job_capacity: 2,
        result_capacity: 2,
        stream_limits: ModelStreamLimits::default(),
        warmup_deadline: None,
        warmup_metadata: Metadata::empty(),
    }
}

async fn spawn_model_owner(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    clock_ms: i64,
    random: u64,
) -> Result<RunTaskOwner, RunHandleError> {
    RunTaskOwner::spawn_with_model(
        coordinator,
        owner_run_config(),
        owner_model_config(),
        model,
        locked_profile(),
        FixedClock::new(timestamp(clock_ms)),
        CounterRandom(AtomicU64::new(random)),
    )
    .await
}

fn completed_plan(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(text)))),
        ],
    }
}

fn deferred_plan(handle: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
            scripted_deferral(handle),
        )))],
    }
}

fn scripted_deferral(handle: &str) -> ModelDeferral {
    ModelDeferral {
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.model.scripted").expect("component"),
            handle,
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
    }
}

async fn recover_session(store: &Arc<MemoryJournalStore>) -> CommitCoordinator {
    CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover")
}

async fn wait_state(
    store: &Arc<MemoryJournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let recovered = recover_session(store).await;
            if predicate(recovered.state()) {
                return recovered;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("state wait")
}

async fn crash_before_dispatch(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    retry_safety: RetrySafety,
) -> CommitCoordinator {
    Box::pin(crash_before_dispatch_inner(store, model, retry_safety)).await
}

async fn crash_before_dispatch_inner(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    retry_safety: RetrySafety,
) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(store.clone());
    let mut drive = coordinator.enable_manual_drive(1).expect("manual drive");
    let owner = spawn_model_owner(coordinator, model, 2_000, 700)
        .await
        .expect("owner");
    let handle = owner.handle();
    let driving = tokio::spawn(async move {
        drive_to_active_model_request_with(&handle, retry_safety).await;
    });
    let permit = tokio::time::timeout(StdDuration::from_secs(1), drive.next_effect())
        .await
        .expect("paused")
        .expect("permit");
    assert_eq!(permit.effect().action, ManualDriveAction::Execute);
    drop(owner);
    driving.abort();
    let _ = driving.await;
    drop(permit);
    recover_session(&store).await
}

async fn journal_has_rejection(store: &Arc<MemoryJournalStore>) -> bool {
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    loaded.committed_batches.iter().any(|batch| {
        batch
            .records
            .iter()
            .any(|record| matches!(record.body(), RecordBody::ExternalCommandRejected(_)))
    })
}

#[tokio::test]
async fn model_resume_unstarted_crash_retries_same_identity() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("retried")])
            .with_reconcile_results(vec![ModelReconcileResult::NotStarted]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    assert_eq!(model.request_count(), 0);
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let pending = recovered
        .state()
        .pending_model_effect
        .clone()
        .expect("pending");
    let effect_id = pending.requested.effect_id();
    let request_id = pending.model_request_id;
    let input_digest = pending.requested.input_digest();
    let canonical = match pending.requested.input() {
        EffectInput::Model { request } => request.clone(),
        other => panic!("expected model input, got {other:?}"),
    };
    assert_eq!(
        input_digest,
        pending.requested.input().digest().expect("input digest")
    );
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 701)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.model_settlements.contains_key(&effect_id)
    })
    .await;
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.reconcile_count(), 1);
    let retried = model.last_request().expect("retried request");
    assert_eq!(retried.call.run.effect_id, effect_id);
    assert_eq!(retried.call.request_id, request_id);
    assert_eq!(
        retried
            .draft
            .canonical_bytes()
            .expect("canonical")
            .as_slice(),
        canonical.as_bytes()
    );
    let settled = recover_session(&store).await;
    assert_eq!(
        settled
            .state()
            .pending_model_effect
            .as_ref()
            .map(|pending| pending.requested.effect_id()),
        None
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_in_flight_still_running_defers_without_second_request() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(
            profile(),
            vec![ScriptedModelPlan {
                actions: vec![
                    ScriptedModelAction::Block(Arc::from("in-flight")),
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("late")))),
                ],
            }],
        )
        .with_reconcile_results(vec![ModelReconcileResult::StillRunning(
            scripted_deferral("job-1"),
        )]),
    );
    let control = model.control();
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        710,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    tokio::time::timeout(StdDuration::from_secs(1), async {
        while control.entries("in-flight") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("entered");
    assert_eq!(model.request_count(), 1);
    drop(owner);
    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 711)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.reconcile_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_in_flight_unknown_retries_same_effect_id() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(
            profile(),
            vec![
                ScriptedModelPlan {
                    actions: vec![
                        ScriptedModelAction::Block(Arc::from("in-flight-retry")),
                        ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(
                            "first",
                        )))),
                    ],
                },
                completed_plan("retried"),
            ],
        )
        .with_reconcile_results(vec![ModelReconcileResult::Unknown]),
    );
    let control = model.control();
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        720,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    tokio::time::timeout(StdDuration::from_secs(1), async {
        while control.entries("in-flight-retry") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("entered");
    let pending = recover_session(&store)
        .await
        .state()
        .pending_model_effect
        .clone()
        .expect("pending");
    let effect_id = pending.requested.effect_id();
    let request_id = pending.model_request_id;
    let input_digest = pending.requested.input_digest();
    let canonical = match pending.requested.input() {
        EffectInput::Model { request } => request.clone(),
        other => panic!("expected model input, got {other:?}"),
    };
    assert_eq!(
        input_digest,
        pending.requested.input().digest().expect("input digest")
    );
    drop(owner);
    let recovered = recover_session(&store).await;
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 721)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.model_settlements.contains_key(&effect_id)
    })
    .await;
    assert_eq!(model.request_count(), 2);
    let retried = model.last_request().expect("retried request");
    assert_eq!(retried.call.run.effect_id, effect_id);
    assert_eq!(retried.call.request_id, request_id);
    assert_eq!(
        retried
            .draft
            .canonical_bytes()
            .expect("canonical")
            .as_slice(),
        canonical.as_bytes()
    );
    assert!(
        recover_session(&store)
            .await
            .state()
            .model_settlements
            .contains_key(&effect_id)
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_completed_uncommitted_settles_without_request() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_reconcile_results(vec![ModelReconcileResult::Completed(completed("hello"))]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let effect_id = recovered
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending")
        .requested
        .effect_id();
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 731)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.model_settlements.contains_key(&effect_id)
    })
    .await;
    assert_eq!(model.request_count(), 0);
    assert_eq!(model.reconcile_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_deferred_same_handle_waits_and_completed_settles_externally() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![deferred_plan("job-1")]).with_reconcile_results(
            vec![
                ModelReconcileResult::StillRunning(scripted_deferral("job-1")),
                ModelReconcileResult::Completed(completed("external")),
            ],
        ),
    );
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        740,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);
    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let requests = model.request_count();
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 741)
        .await
        .expect("respawn wait");
    assert_eq!(model.request_count(), requests);
    assert_eq!(
        recover_session(&store).await.state().phase,
        Some(RunPhase::AwaitingExternal)
    );
    owner.shutdown().await;

    let recovered = recover_session(&store).await;
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_600, 742)
        .await
        .expect("respawn complete");
    wait_state(&store, |state| {
        state.phase != Some(RunPhase::AwaitingExternal) && state.pending_model_effect.is_none()
    })
    .await;
    assert_eq!(model.request_count(), requests);
    owner.shutdown().await;

    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::NoOutstanding
    );
    let reconciles = model.reconcile_count();
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_600, 743)
        .await
        .expect("equal completion idempotent");
    assert_eq!(model.request_count(), requests);
    assert_eq!(model.reconcile_count(), reconciles);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_non_resumable_suspends_without_request() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_reconcile_results(vec![ModelReconcileResult::Unknown]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::AtMostOnce).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 751).await else {
        panic!("suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(model.request_count(), 0);
    assert_eq!(
        recover_session(&store).await.state().phase,
        Some(RunPhase::AwaitingModel)
    );

    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_idempotent_requests(false)
            .with_reconcile_results(vec![ModelReconcileResult::Unknown]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 752).await else {
        panic!("non-idempotent suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(model.request_count(), 0);

    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_reconcile_results(vec![ModelReconcileResult::NonRepeatable]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 753).await else {
        panic!("non-repeatable suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(model.request_count(), 0);
}

#[tokio::test]
async fn model_resume_settled_effect_never_requests_or_reconciles() {
    let store = memory_store();
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let mut owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        760,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.pending_model_effect.is_none() && !state.model_settlements.is_empty()
    })
    .await;
    let requests = model.request_count();
    owner.shutdown().await;
    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::NoOutstanding
    );
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 761)
        .await
        .expect("respawn");
    assert_eq!(model.request_count(), requests);
    assert_eq!(model.reconcile_count(), 0);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_conflicting_deferred_handle_fails_closed() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![deferred_plan("job-1")]).with_reconcile_results(
            vec![ModelReconcileResult::StillRunning(scripted_deferral(
                "job-other",
            ))],
        ),
    );
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        770,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);
    let recovered = recover_session(&store).await;
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 771).await else {
        panic!("conflict");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert!(journal_has_rejection(&store).await);
    assert_eq!(
        recover_session(&store).await.state().phase,
        Some(RunPhase::AwaitingExternal)
    );
    assert_eq!(model.request_count(), 1);
}
