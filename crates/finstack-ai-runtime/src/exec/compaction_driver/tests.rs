use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendBatchId, AppendRequest, BudgetPropagation,
    CancellationPropagation, CommittedBatch, CompactionAuthorization, ContentBlock,
    DeadlinePropagation, Digest, Id, IdTag, KernelInput, LaneTag, Message, MessageRole, Metadata,
    PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, RecordBody, RecordEnvelope,
    ReducerStageOutcome, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
    RunSecurityContext, Sensitivity, SessionTag, Stage, StageCursor, StageSettled, TextBlock,
    Timestamp, TransitionEnv, Version,
};
use futures_core::Stream;

use crate::Usage;
use crate::commit::CommitCoordinator;
use crate::compaction_driver::resume_pending_compaction_model;
use crate::context::{ContextAuthority, ContextItem, ContextItemKind, ContextProvenance};
use crate::ids::{ExternalClock, IdGenerationError, RandomSource};
use crate::middleware::{
    BeforeModelInput, CompactionEvidence, CompactionResult, MiddlewareDescriptor, MiddlewareOrder,
    MiddlewareRegistration, MiddlewareRole, OrderTier, PromptCacheImpact, ResolvedMiddlewareChain,
    StageInput, StageMask, StageOutcome,
};
use crate::middleware_driver::StageDriver;
use crate::ports::PortFuture;
use crate::ports::journal::{
    JournalStore, LoadRequest, LoadedSession, SnapshotReceipt, SnapshotRequest, StoreError,
    StoreHealth,
};
use crate::ports::model::{
    CancellationSignal, InputCapabilities, Model, ModelCapabilities, ModelContextProfile,
    ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelRequest, ModelRequestDraft,
    ModelResponse, ModelSettings, ModelStreamItem, ModelTokenEstimate, ModelWarmupContext,
    StructuredOutputCapability, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    resolve_model_context_profile,
};
use crate::settlement::SettlementSources;
use crate::stage_settlement::settle_facade_stage_with_model;

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

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

#[expect(clippy::too_many_arguments, reason = "mirrors AllocatedIds' own bags")]
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
        .expect("allocated ids"),
    }
}

fn acceptance(limits: RunLimits) -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security")
        .with_compaction_authorization(CompactionAuthorization::new(
            finstack_ai_kernel::ComponentRef::new(
                finstack_ai_kernel::ComponentId::parse("fixture.child-model").expect("component"),
                None,
            ),
            Sensitivity::Internal,
            Digest::raw_json(b"residency"),
        )),
        None,
        limits,
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"agent":"fixture"}"#),
        None,
    )
    .expect("acceptance")
}

fn acceptance_without_compaction_auth(limits: RunLimits) -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        None,
        limits,
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"agent":"fixture"}"#),
        None,
    )
    .expect("acceptance")
}

fn accept_input(limits: RunLimits) -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: acceptance(limits),
    })
}

fn accept_input_without_compaction_auth(limits: RunLimits) -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: acceptance_without_compaction_auth(limits),
    })
}

fn user_message(ordinal: u64, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        MessageRole::User,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn message_text(message: &Message) -> String {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

fn descriptor(component: &str) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: finstack_ai_kernel::ComponentInvocation {
            component: finstack_ai_kernel::ComponentId::parse(component).expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery: finstack_ai_kernel::InvocationRecovery::RecomputeSafe,
        },
        stages: StageMask::from_stages([Stage::BeforeModel]),
        order: MiddlewareOrder {
            tier: OrderTier::ContextCompaction,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        role: MiddlewareRole::ContextCompactor {
            strategy_id: Arc::from("finstack.compaction.summarize"),
            strategy_version: 1,
        },
        metadata: Metadata::empty(),
    }
}

fn test_profile() -> crate::ports::model::LockedModelContextProfile {
    resolve_model_context_profile(
        ModelContextProfile {
            provider: Arc::from("fixture-provider"),
            model: ModelName::try_new("fixture-model").expect("model"),
            hard_input_bytes: 1_000_000,
            context_window_tokens: 10_000,
            max_output_tokens: 1_000,
            reserved_output_tokens: 1_000,
            provider_overhead_tokens: 500,
            estimator: TokenEstimatorRef {
                id: Arc::from("fixture-estimator"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ProjectExact,
            },
        },
        None,
        None,
        false,
    )
    .expect("locked profile")
}

fn request_draft(messages: Vec<Message>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: ModelName::try_new("fixture-model").expect("model"),
        messages: messages.into(),
        tools: Arc::from([]),
        output: finstack_ai_kernel::OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: crate::model::ModelRequestLimits {
            max_input_bytes: 1_000_000,
            max_input_tokens: 8_500,
            max_output_tokens: 1_000,
        },
    }
}

fn model_request_settled(draft: &ModelRequestDraft) -> StageSettled {
    StageSettled {
        cursor: StageCursor {
            cycle: 0,
            stage: Stage::BeforeModel,
        },
        outcome: ReducerStageOutcome::ModelRequestPrepared {
            request: RawJson::parse(draft.canonical_bytes().expect("canonical bytes"))
                .expect("request"),
            component: None,
            output_contract: finstack_ai_kernel::EffectOutputContract {
                kind: finstack_ai_kernel::EffectOutputKind::ModelResponse,
                schema_version: 1,
                schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
            },
            retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
            deadline: None,
        },
    }
}

fn before_model_env() -> TransitionEnv {
    env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104)
}

#[derive(Default)]
struct CountingRandom(AtomicU64);

impl RandomSource for CountingRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        let counter = self.0.fetch_add(1, Ordering::Relaxed);
        let bytes = counter.to_be_bytes();
        for (index, slot) in buf.iter_mut().enumerate() {
            *slot = bytes[index % bytes.len()];
        }
        Ok(())
    }
}

fn test_sources() -> SettlementSources<ExternalClock, CountingRandom> {
    test_sources_from(0)
}

fn test_sources_from(start: u64) -> SettlementSources<ExternalClock, CountingRandom> {
    SettlementSources::try_new(
        ExternalClock::new(timestamp(1_000)),
        CountingRandom(AtomicU64::new(start)),
    )
    .expect("sources")
}

struct MemoryStore {
    inner: Mutex<MemoryInner>,
}

struct MemoryInner {
    batches: Vec<CommittedBatch>,
    requests: BTreeMap<AppendBatchId, AppendRequest>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryInner {
                batches: Vec::new(),
                requests: BTreeMap::new(),
            }),
        }
    }
}

fn commit_request(request: &AppendRequest) -> CommittedBatch {
    let records = request
        .records()
        .iter()
        .enumerate()
        .map(|(offset, draft)| {
            let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
            RecordEnvelope::try_new(
                draft.format_version(),
                draft.kind_version(),
                draft.record_id(),
                draft.session_id(),
                draft.lane_id(),
                draft.run_id(),
                sequence,
                draft.timestamp(),
                None,
                Digest::raw_json(format!("payload-{sequence}").as_bytes()),
                None,
                Digest::raw_json(format!("checksum-{sequence}").as_bytes()),
                draft.derived_event_ids().to_vec(),
                draft.body().clone(),
            )
            .expect("envelope")
        })
        .collect::<Vec<_>>();
    CommittedBatch::try_new(
        request.batch_id(),
        request.expected_sequence(),
        request.expected_sequence() + u64::try_from(records.len()).expect("count") - 1,
        records,
    )
    .expect("committed batch")
}

impl JournalStore for MemoryStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let mut inner = self.inner.lock().expect("lock");
        if let Some(existing) = inner.requests.get(&request.batch_id()) {
            if existing != &request {
                return Box::pin(async {
                    Err(StoreError::Corruption {
                        reason_code: "batch_reuse",
                    })
                });
            }
            let batch = inner
                .batches
                .iter()
                .find(|batch| batch.batch_id == request.batch_id())
                .cloned()
                .expect("committed");
            return Box::pin(async move { Ok(batch) });
        }
        let committed = commit_request(&request);
        inner.requests.insert(request.batch_id(), request);
        inner.batches.push(committed.clone());
        Box::pin(async move { Ok(committed) })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let inner = self.inner.lock().expect("lock");
        let batches = inner
            .batches
            .iter()
            .filter(|batch| {
                batch
                    .records
                    .first()
                    .is_some_and(|record| record.session_id() == request.session_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
        Box::pin(async move {
            Ok(LoadedSession {
                session_id: request.session_id,
                head_sequence,
                head_checksum: batches
                    .last()
                    .and_then(|batch| batch.records.last().map(RecordEnvelope::checksum)),
                metadata: Metadata::empty(),
                committed_batches: batches.into(),
                snapshot: None,
                accelerated: None,
            })
        })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "not_used",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("test"),
            })
        })
    }
}

fn accepted_on(store: Arc<MemoryStore>) -> CommitCoordinator {
    accepted_with(store, accept_input(RunLimits::empty()))
}

fn accepted_without_compaction_auth(store: Arc<MemoryStore>) -> CommitCoordinator {
    accepted_with(
        store,
        accept_input_without_compaction_auth(RunLimits::empty()),
    )
}

fn accepted_with(store: Arc<MemoryStore>, accept: KernelInput) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(store);
    block_on(coordinator.submit(env(1_000, &[1], &[1], &[], &[], &[], &[], 101), accept))
        .expect("accept");
    block_on(coordinator.submit(
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    ))
    .expect("before run");
    block_on(coordinator.submit(
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([user_message(4, "hi")]),
            },
        }),
    ))
    .expect("context");
    coordinator
}

fn summarize_driver() -> StageDriver {
    let middleware: Arc<dyn crate::middleware::Middleware> = Arc::new(SummarizeCompactor {
        descriptor: descriptor("finstack.middleware.compaction"),
    });
    StageDriver::new(
        Arc::new(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                .expect("chain"),
        ),
        CancellationSignal::new(),
    )
}

struct SummarizeCompactor {
    descriptor: MiddlewareDescriptor,
}

impl crate::middleware::Middleware for SummarizeCompactor {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: crate::middleware::MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, crate::middleware::MiddlewareError>> {
        let descriptor = self.descriptor.clone();
        Box::pin(async move {
            let StageInput::BeforeModel(before) = input else {
                return Ok(StageOutcome::Continue);
            };
            if let Some(resume) = ctx.compaction_resume {
                return Ok(StageOutcome::CompactContext(Box::new(compacted(
                    &descriptor,
                    &before,
                    &resume.result,
                ))));
            }
            Ok(StageOutcome::RequestCompactionModel(Box::new(
                crate::middleware::CompactionModelRequest {
                    model: finstack_ai_kernel::ComponentRef::new(
                        finstack_ai_kernel::ComponentId::parse("fixture.child-model")
                            .expect("component"),
                        None,
                    ),
                    request: before.request.clone(),
                    budget_scope_id: id(9),
                    source_sensitivity: Sensitivity::Internal,
                    residency_policy_digest: Digest::raw_json(b"residency"),
                    resume_state: RawJson::parse(b"{}").expect("resume"),
                },
            )))
        })
    }
}

fn compacted(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    response: &ModelResponse,
) -> CompactionResult {
    let retained = input
        .source_entries
        .last()
        .expect("trailing user")
        .message
        .clone();
    let replacement_messages: Arc<[Message]> = Arc::from([retained.clone()]);
    let summary_text = response
        .assistant_content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let summary = ContextItem::try_new(
        ContextItemKind::DerivedSummary,
        vec![ContentBlock::Text(
            TextBlock::try_new(&summary_text).expect("text"),
        )],
        ContextProvenance {
            source_id: Arc::from("finstack.compaction.summarize"),
            source_ref: None,
            external: false,
        },
        ContextAuthority::Untrusted,
        0,
        4,
        Sensitivity::Internal,
        false,
    )
    .expect("summary");
    let by_message = |message: &Message| {
        input
            .source_entries
            .iter()
            .find(|entry| entry.message.id() == message.id())
            .expect("retained")
            .entry_id
    };
    CompactionResult {
        evidence: CompactionEvidence {
            strategy_id: match &descriptor.role {
                MiddlewareRole::ContextCompactor { strategy_id, .. } => Arc::clone(strategy_id),
                _ => Arc::from("finstack.compaction.summarize"),
            },
            strategy_version: 1,
            configuration_digest: Digest::raw_json(b"{}"),
            model_context_profile_digest: input.model_context_profile_digest,
            source_digest: crate::middleware::compaction_source_digest(&input.source_entries)
                .expect("source digest"),
            protected_item_set_digest: crate::middleware::compaction_protected_set_digest(
                &input
                    .source_entries
                    .iter()
                    .filter(|entry| entry.protected)
                    .map(|entry| entry.entry_id)
                    .collect::<Vec<_>>(),
            )
            .expect("protected digest"),
            covered_entry_ids: input
                .source_entries
                .iter()
                .map(|entry| entry.entry_id)
                .collect::<Vec<_>>()
                .into(),
            retained_entry_ids: Arc::from([by_message(&retained)]),
            projection_digest: crate::middleware::compaction_projection_digest(
                &replacement_messages,
            )
            .expect("projection digest"),
            estimated_tokens_before: 10,
            estimated_tokens_after: 5,
            summary_digest: None,
            cache_impact: PromptCacheImpact::CacheInvalidated,
        },
        replacement_messages,
        derived_summaries: Arc::from([summary]),
        checkpoint: None,
    }
}

struct ScriptedModel {
    profile: ModelContextProfile,
    requests: AtomicU64,
    fail_first: bool,
}

impl ScriptedModel {
    fn new(fail_first: bool) -> Self {
        Self {
            profile: test_profile().profile,
            requests: AtomicU64::new(0),
            fail_first,
        }
    }
}

impl Model for ScriptedModel {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            provider: Arc::clone(&self.profile.provider),
            models: Arc::from([self.profile.model.clone()]),
            metadata: Metadata::empty(),
        }
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: self.profile.clone(),
            native_tool_calls: false,
            parallel_tool_calls: false,
            structured_output: StructuredOutputCapability::Unsupported,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: true,
            native_capabilities: BTreeSet::new(),
        }
    }

    fn estimate_input_tokens(
        &self,
        _model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len()).unwrap_or(1).max(1),
            estimator: self.profile.estimator.clone(),
        })
    }

    fn warmup(&self, _ctx: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
        Box::pin(async { Ok(()) })
    }

    fn request(&self, _request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        let attempt = self.requests.fetch_add(1, Ordering::AcqRel) + 1;
        if self.fail_first && attempt == 1 {
            return Box::pin(async {
                Err(ModelError::try_new(
                    "scripted_crash_after_request",
                    finstack_ai_kernel::ErrorCategory::Internal,
                    false,
                    "simulated crash after the compaction request committed",
                    Metadata::empty(),
                )
                .expect("error"))
            });
        }
        Box::pin(async {
            Ok(Box::pin(OnceStream::new(vec![
                Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("COMPACTION-SUMMARY"),
                })),
                Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([ContentBlock::Text(
                        TextBlock::try_new("COMPACTION-SUMMARY").expect("text"),
                    )]),
                    tool_calls: Arc::from([]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::empty(),
                    completion_id: Arc::from("compaction-completion"),
                    continuation_state: None,
                })),
            ])) as ModelEventStream)
        })
    }
}

struct OnceStream<T> {
    items: VecDeque<T>,
}

impl<T> Unpin for OnceStream<T> {}

impl<T> OnceStream<T> {
    fn new(items: Vec<T>) -> Self {
        Self {
            items: items.into(),
        }
    }
}

impl<T> Stream for OnceStream<T> {
    type Item = T;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().items.pop_front())
    }
}

fn committed_draft(coordinator: &CommitCoordinator) -> ModelRequestDraft {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending primary model");
    let finstack_ai_kernel::EffectInput::Model { request } = pending.requested.input() else {
        panic!("pending must be a model effect");
    };
    serde_json::from_slice(request.as_bytes()).expect("draft")
}

fn summary_occurrences(draft: &ModelRequestDraft) -> usize {
    draft
        .messages
        .iter()
        .filter(|message| message_text(message).contains("COMPACTION-SUMMARY"))
        .count()
}

fn compaction_requests(
    coordinator: &CommitCoordinator,
) -> Vec<finstack_ai_kernel::EffectRequested> {
    let session_id = coordinator.state().session_id.expect("session");
    let loaded =
        block_on(coordinator.journal_store().load(LoadRequest { session_id })).expect("load");
    loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .filter_map(|record| match record.body() {
            RecordBody::EffectRequested(requested) if requested.is_compaction_summary() => {
                Some(requested.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn summarize_completes_against_a_scripted_model_without_duplicating_on_retry() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = accepted_on(Arc::clone(&store) as Arc<MemoryStore>);
    let sources = test_sources();
    let driver = summarize_driver();
    let model = ScriptedModel::new(false);
    let draft = request_draft(vec![user_message(4, "hi")]);

    block_on(settle_facade_stage_with_model(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
        Some(&model),
    ))
    .expect("summarize must land");

    assert_eq!(
        model.requests.load(Ordering::SeqCst),
        1,
        "one metered model call"
    );
    assert_eq!(summary_occurrences(&committed_draft(&coordinator)), 1);
    let requested = compaction_requests(&coordinator);
    assert_eq!(requested.len(), 1, "one child model effect");
    let parent = coordinator
        .last_middleware_effect_id()
        .expect("durable middleware parent");
    assert_eq!(
        requested[0]
            .relation()
            .map(|relation| relation.parent_effect_id),
        Some(parent),
        "parent linkage is the committed BeforeModel middleware effect"
    );
    assert!(
        coordinator
            .state()
            .model_settlements
            .contains_key(&requested[0].effect_id()),
        "child effect is settled and replayable"
    );

    let session_id = coordinator.state().session_id.expect("session");
    let replayed = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        session_id,
    ))
    .expect("replay");
    assert!(
        replayed
            .state()
            .model_settlements
            .contains_key(&requested[0].effect_id()),
        "replay restores the compaction settlement"
    );
    assert_eq!(summary_occurrences(&committed_draft(&replayed)), 1);
    assert_eq!(
        model.requests.load(Ordering::SeqCst),
        1,
        "replay must not call the model"
    );
    assert_eq!(compaction_requests(&replayed).len(), 1);
    recover_summarize_after_crash(&driver, &draft);
}

fn recover_summarize_after_crash(driver: &StageDriver, draft: &ModelRequestDraft) {
    let crash_store = Arc::new(MemoryStore::new());
    let mut crashing = accepted_on(Arc::clone(&crash_store) as Arc<MemoryStore>);
    let crashing_model = ScriptedModel::new(true);
    let crash_error = block_on(settle_facade_stage_with_model(
        &mut crashing,
        Some(driver),
        &test_sources(),
        &test_profile(),
        before_model_env(),
        model_request_settled(draft),
        Some(&crashing_model),
    ))
    .expect_err("first attempt crashes after the request commits");
    assert!(matches!(
        crash_error,
        crate::run::RunHandleError::Middleware { .. }
    ));
    assert!(
        crashing
            .state()
            .pending_model_effect
            .as_ref()
            .is_some_and(|pending| pending.requested.is_compaction_summary()),
        "crash leaves the committed child pending"
    );
    let crash_session = crashing.state().session_id.expect("session");
    let mut recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&crash_store) as Arc<dyn JournalStore>,
        crash_session,
    ))
    .expect("recover");
    let recovered_model = ScriptedModel::new(false);
    block_on(settle_facade_stage_with_model(
        &mut recovered,
        Some(driver),
        &test_sources_from(10_000),
        &test_profile(),
        env(1_400, &[15, 16], &[12], &[113], &[], &[112], &[], 114),
        model_request_settled(draft),
        Some(&recovered_model),
    ))
    .expect("recover at-least-once");
    assert_eq!(recovered_model.requests.load(Ordering::SeqCst), 1);
    assert_eq!(compaction_requests(&recovered).len(), 1);
    assert_eq!(summary_occurrences(&committed_draft(&recovered)), 1);
}

#[test]
fn host_task_resume_settles_pending_compaction_without_a_new_user_turn() {
    let store = Arc::new(MemoryStore::new());
    let mut crashing = accepted_on(Arc::clone(&store) as Arc<MemoryStore>);
    let crashing_model = ScriptedModel::new(true);
    let draft = request_draft(vec![user_message(4, "hi")]);
    block_on(settle_facade_stage_with_model(
        &mut crashing,
        Some(&summarize_driver()),
        &test_sources(),
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
        Some(&crashing_model),
    ))
    .expect_err("crash after compaction request");
    let session_id = crashing.state().session_id.expect("session");
    let mut recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        session_id,
    ))
    .expect("recover");
    let recovered_model = ScriptedModel::new(false);
    let resumed = block_on(resume_pending_compaction_model(
        &mut recovered,
        &test_sources_from(10_000),
        &test_profile(),
        &recovered_model,
        &CancellationSignal::new(),
    ))
    .expect("resume");
    assert!(resumed, "pending compaction summary must resume");
    assert_eq!(recovered_model.requests.load(Ordering::SeqCst), 1);
    assert_eq!(compaction_requests(&recovered).len(), 1);
    assert!(
        recovered
            .state()
            .model_settlements
            .contains_key(&compaction_requests(&recovered)[0].effect_id()),
        "summary settles once without a new user turn"
    );
}

#[test]
fn missing_compaction_authorization_fails_before_commit() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = accepted_without_compaction_auth(Arc::clone(&store) as Arc<MemoryStore>);
    let draft = request_draft(vec![user_message(4, "hi")]);
    let error = block_on(settle_facade_stage_with_model(
        &mut coordinator,
        Some(&summarize_driver()),
        &test_sources(),
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
        Some(&ScriptedModel::new(false)),
    ))
    .expect_err("missing lock must deny");
    assert!(
        matches!(
            &error,
            crate::run::RunHandleError::Middleware { code }
                if code.as_ref() == crate::ports::middleware::COMPACTION_MODEL_NOT_AUTHORIZED
        ),
        "expected compaction_model_not_authorized, got {error:?}"
    );
    assert!(
        compaction_requests(&coordinator).is_empty(),
        "unauthorized request must not commit a child effect"
    );
    assert!(
        coordinator.state().pending_model_effect.is_none(),
        "no pending model effect before commit"
    );
}

#[test]
fn mismatched_compaction_authorization_fails_before_commit() {
    let run_id = id(3);
    let accept = KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security")
            .with_compaction_authorization(CompactionAuthorization::new(
                finstack_ai_kernel::ComponentRef::new(
                    finstack_ai_kernel::ComponentId::parse("fixture.other-model")
                        .expect("component"),
                    None,
                ),
                Sensitivity::Internal,
                Digest::raw_json(b"residency"),
            )),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"agent":"fixture"}"#),
            None,
        )
        .expect("acceptance"),
    });
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = accepted_with(Arc::clone(&store) as Arc<MemoryStore>, accept);
    let draft = request_draft(vec![user_message(4, "hi")]);
    let error = block_on(settle_facade_stage_with_model(
        &mut coordinator,
        Some(&summarize_driver()),
        &test_sources(),
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
        Some(&ScriptedModel::new(false)),
    ))
    .expect_err("mismatched lock must deny");
    assert!(
        matches!(
            &error,
            crate::run::RunHandleError::Middleware { code }
                if code.as_ref() == crate::ports::middleware::COMPACTION_MODEL_NOT_AUTHORIZED
        ),
        "expected compaction_model_not_authorized, got {error:?}"
    );
    assert!(compaction_requests(&coordinator).is_empty());
}

#[test]
fn authorize_compaction_model_request_rejects_sensitivity_and_digest_mismatches() {
    let model = finstack_ai_kernel::ComponentRef::new(
        finstack_ai_kernel::ComponentId::parse("fixture.child-model").expect("component"),
        None,
    );
    let auth = CompactionAuthorization::new(
        model.clone(),
        Sensitivity::Internal,
        Digest::raw_json(b"residency"),
    );
    let ok = crate::middleware::CompactionModelRequest {
        model: model.clone(),
        request: request_draft(vec![user_message(4, "hi")]),
        budget_scope_id: id(9),
        source_sensitivity: Sensitivity::Internal,
        residency_policy_digest: Digest::raw_json(b"residency"),
        resume_state: RawJson::parse(b"{}").expect("resume"),
    };
    crate::ports::middleware::authorize_compaction_model_request(Some(&auth), &ok)
        .expect("authorized");
    assert!(
        crate::ports::middleware::authorize_compaction_model_request(None, &ok).is_err(),
        "absence denies"
    );
    let too_sensitive = crate::middleware::CompactionModelRequest {
        source_sensitivity: Sensitivity::Confidential,
        ..ok.clone()
    };
    assert!(
        crate::ports::middleware::authorize_compaction_model_request(Some(&auth), &too_sensitive)
            .is_err()
    );
    let wrong_digest = crate::middleware::CompactionModelRequest {
        residency_policy_digest: Digest::raw_json(b"other"),
        ..ok
    };
    assert!(
        crate::ports::middleware::authorize_compaction_model_request(Some(&auth), &wrong_digest)
            .is_err()
    );
}
