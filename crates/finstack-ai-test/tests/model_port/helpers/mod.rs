//! PR-015 provider-neutral Model port and runtime acceptance proofs.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancellationPropagation,
    CommittedBatch, ContentBlock, Digest, EffectOutputContract, EffectOutputKind, Id, IdTag,
    KernelInput, LaneTag, Message, MessageRole, Metadata, OperationLocator, OutputSpec,
    PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, ReducerStageOutcome, RetrySafety,
    RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, SessionTag,
    Stage, StageCursor, TextBlock, Timestamp, TransitionEnv, Usage,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, IdGenerationError, JournalStore, LoadRequest,
    LoadedSession, LockedModelContextProfile, Model, ModelCallContext, ModelContextProfile,
    ModelError, ModelRequest, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamAssembler, ModelStreamLimits, ModelTerminal, PortFuture, RandomSource,
    RunCallContext, RunHandle, SnapshotReceipt, SnapshotRequest, StoreError, StoreHealth,
    TokenEstimatorRef, TokenEstimatorSource, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ScriptedInput, ScriptedModel, ScriptedModelPlan, ScriptedStep, ScriptedStepKind,
};

mod resume;
pub(crate) use resume::*;

pub(crate) fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

pub(crate) fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

pub(crate) fn profile() -> ModelContextProfile {
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

pub(crate) fn locked_profile() -> LockedModelContextProfile {
    resolve_model_context_profile(profile(), None, None, false).expect("locked profile")
}

pub(crate) fn draft(messages: Arc<[Message]>) -> ModelRequestDraft {
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

pub(crate) fn request(cancellation: CancellationSignal) -> ModelRequest {
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

pub(crate) fn completed(text: &str) -> ModelResponse {
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

pub(crate) fn chunks(count: usize, text: &str) -> ScriptedInput {
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

pub(crate) async fn assemble_plan(plan: ScriptedModelPlan) -> Result<ModelTerminal, ModelError> {
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

pub(crate) struct CounterRandom(pub(crate) AtomicU64);

impl RandomSource for CounterRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let value = self.0.fetch_add(1, Ordering::AcqRel).to_be_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = value[index % value.len()];
        }
        Ok(())
    }
}

pub(crate) struct FailFourthAppendStore {
    inner: MemoryJournalStore,
    appends: AtomicUsize,
}

impl FailFourthAppendStore {
    pub(crate) fn new() -> Self {
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
pub(crate) fn env(
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

pub(crate) fn accepted() -> RunAccepted {
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

pub(crate) fn user_message() -> Message {
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

pub(crate) fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

pub(crate) async fn drive_to_active_model_request(handle: &RunHandle) {
    drive_to_active_model_request_with(handle, RetrySafety::SafeToRetry).await;
}

pub(crate) async fn drive_to_active_model_request_with(
    handle: &RunHandle,
    retry_safety: RetrySafety,
) {
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
