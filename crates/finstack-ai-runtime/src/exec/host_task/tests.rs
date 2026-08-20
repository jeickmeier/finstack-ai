use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancellationPropagation,
    CommittedBatch, ContentBlock, Digest, Id, IdTag, KernelInput, Message, MessageRole, Metadata,
    OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, ReducerStageOutcome,
    RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, Stage,
    StageCursor, StageSettled, TextBlock, Timestamp, TransitionEnv,
};
use futures_core::Stream;

use super::*;
use crate::coordinator::CommitCoordinator;
use crate::event_hub::{
    EventBatchConfig, EventFilter, EventHubConfig, EventLagPolicy, EventSubscriptionConfig,
    ProgressCoalescing,
};
use crate::host_driver;
use crate::{
    ApprovalGrantMode, Clock, InputCapabilities, JournalStore, LoadRequest, LoadedSession, Model,
    ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError, ModelEventStream,
    ModelName, ModelProgress, ModelRequest, ModelRequestDraft, ModelResponse, ModelSettings,
    ModelStreamItem, ModelTaskConfig, ModelTokenEstimate, ModelWarmupContext, PortFuture,
    RandomSource, RunTaskConfig, SameIdentityRetryPolicy, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth, StructuredOutputCapability, TextDelta, TokenEstimatorRef,
    TokenEstimatorSource, Usage, resolve_model_context_profile,
};

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::noop().clone();
    let mut cx = Context::from_waker(&waker);
    loop {
        host_driver::drive_local();
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        host_driver::drive_local();
    }
}

struct MemoryStore {
    batches: Mutex<Vec<CommittedBatch>>,
    requests: Mutex<BTreeMap<finstack_ai_kernel::AppendBatchId, AppendRequest>>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            batches: Mutex::new(Vec::new()),
            requests: Mutex::new(BTreeMap::new()),
        }
    }
}

impl JournalStore for MemoryStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let mut requests = self.requests.lock().expect("requests");
        if let Some(existing) = requests.get(&request.batch_id()) {
            if existing != &request {
                return Box::pin(async {
                    Err(StoreError::Corruption {
                        reason_code: "batch_reuse",
                    })
                });
            }
            let committed = self
                .batches
                .lock()
                .expect("batches")
                .iter()
                .find(|batch| batch.batch_id == request.batch_id())
                .cloned()
                .expect("indexed batch");
            return Box::pin(async move { Ok(committed) });
        }
        let committed = commit_request(&request);
        requests.insert(request.batch_id(), request);
        self.batches
            .lock()
            .expect("batches")
            .push(committed.clone());
        Box::pin(async move { Ok(committed) })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let batches = self
            .batches
            .lock()
            .expect("batches")
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
                head_checksum: batches.last().and_then(|batch| {
                    batch
                        .records
                        .last()
                        .map(finstack_ai_kernel::RecordEnvelope::checksum)
                }),
                metadata: finstack_ai_kernel::Metadata::empty(),
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

struct CompletingModel {
    profile: ModelContextProfile,
    warmup: AtomicU64,
    requests: AtomicU64,
}

impl CompletingModel {
    fn new(profile: ModelContextProfile) -> Self {
        Self {
            profile,
            warmup: AtomicU64::new(0),
            requests: AtomicU64::new(0),
        }
    }
}

impl Model for CompletingModel {
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
        self.warmup.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Ok(()) })
    }

    fn request(&self, _request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        self.requests.fetch_add(1, Ordering::AcqRel);
        Box::pin(async {
            Ok(Box::pin(OnceStream::new(vec![
                Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("hello"),
                })),
                Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([ContentBlock::Text(
                        TextBlock::try_new("hello").expect("text"),
                    )]),
                    tool_calls: Arc::from([]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::empty(),
                    completion_id: Arc::from("host-completion"),
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

struct TestClock {
    now: AtomicU64,
}

impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, crate::IdGenerationError> {
        let ms = self.now.fetch_add(1, Ordering::AcqRel);
        Ok(Timestamp::from_unix_ms(i64::try_from(ms).expect("ms")).expect("timestamp"))
    }
}

struct TestRandom {
    next: AtomicU64,
}

impl RandomSource for TestRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), crate::IdGenerationError> {
        for byte in buf {
            *byte = u8::try_from(self.next.fetch_add(1, Ordering::AcqRel) & 0xff).expect("byte");
        }
        Ok(())
    }
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("scripted-1").expect("model"),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 1_048_576,
        max_output_tokens: 256,
        reserved_output_tokens: 256,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: Arc::from("bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn take_ids<T: IdTag>(next: &mut u64, count: usize) -> Vec<Id<T>> {
    (0..count)
        .map(|_| {
            let value = id(*next);
            *next += 1;
            value
        })
        .collect()
}

fn env(
    ordinal: u64,
    records: usize,
    events: usize,
    effects: usize,
    turns: usize,
    model_requests: usize,
    messages: usize,
) -> TransitionEnv {
    let mut next = ordinal;
    TransitionEnv {
        now: Timestamp::from_unix_ms(1_000).expect("timestamp"),
        ids: AllocatedIds::try_new(
            take_ids(&mut next, records),
            take_ids(&mut next, events),
            take_ids(&mut next, effects),
            Vec::new(),
            take_ids(&mut next, messages),
            take_ids(&mut next, turns),
            take_ids(&mut next, model_requests),
            Vec::new(),
            Vec::new(),
            take_ids(&mut next, 1),
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

fn run_config() -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: 8,
        event_hub: EventHubConfig {
            source_capacity: 8,
            max_subscribers: 4,
        },
        shutdown_deadline: Duration::from_millis(50),
        approval_grant: ApprovalGrantMode::PerCall,
    }
}

fn model_config() -> ModelTaskConfig {
    ModelTaskConfig {
        job_capacity: 8,
        result_capacity: 8,
        stream_limits: crate::ModelStreamLimits::default(),
        warmup_deadline: None,
        warmup_metadata: Metadata::empty(),
        same_identity_retry: SameIdentityRetryPolicy::default(),
    }
}

fn commit_request(request: &AppendRequest) -> CommittedBatch {
    let records = request
        .records()
        .iter()
        .enumerate()
        .map(|(offset, draft)| {
            let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
            finstack_ai_kernel::RecordEnvelope::try_new(
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
    .expect("batch")
}

fn subscription() -> EventSubscriptionConfig {
    EventSubscriptionConfig {
        queue_capacity: 8,
        filter: EventFilter {
            include_durable: true,
            include_transient: true,
            kinds: Arc::from([]),
            max_sensitivity: finstack_ai_kernel::Sensitivity::Confidential,
        },
        batching: EventBatchConfig {
            flush_count: 32,
            flush_bytes: 64 * 1_024,
            flush_interval: Duration::from_millis(10),
        },
        progress_coalescing: ProgressCoalescing::Enabled,
        lag_policy: EventLagPolicy::DropProgress {
            durable_timeout: Duration::from_secs(2),
        },
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "end-to-end host submit fixture stays in one test"
)]
fn submit_dispatches_model_and_publishes_one_event_batch() {
    let clock: Arc<dyn Clock> = Arc::new(TestClock {
        now: AtomicU64::new(1_700_000_000_000),
    });
    let random: Arc<dyn RandomSource> = Arc::new(TestRandom {
        next: AtomicU64::new(1),
    });
    host_driver::install_clock(clock);
    host_driver::install_random(random);
    let store = Arc::new(MemoryStore::new());
    let model_profile = profile();
    let locked =
        resolve_model_context_profile(model_profile.clone(), None, None, false).expect("profile");
    let model = Arc::new(CompletingModel::new(model_profile));
    let mut owner = block_on(RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store),
        run_config(),
        model_config(),
        model.clone(),
        locked.clone(),
        host_driver::InstalledClock,
        host_driver::InstalledRandom,
    ))
    .expect("owner");
    assert_eq!(model.warmup.load(Ordering::Acquire), 1);
    let handle = owner.handle();
    let mut events = block_on(handle.subscribe_events(subscription())).expect("subscribe");

    let session_id = id(1);
    let lane_id = id(2);
    block_on(handle.submit(
        env(10, 1, 1, 0, 0, 0, 0),
        KernelInput::AcceptRun(AcceptRun {
            session_id,
            lane_id,
            accepted: accepted(),
        }),
    ))
    .expect("accept");
    block_on(handle.submit(
        env(20, 1, 0, 0, 0, 0, 0),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    ))
    .expect("before run");

    let message = Message::try_new(
        id(30),
        MessageRole::User,
        vec![ContentBlock::Text(TextBlock::try_new("hi").expect("text"))],
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message");
    block_on(handle.submit(
        env(40, 2, 0, 0, 1, 0, 0),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([message]),
            },
        }),
    ))
    .expect("context");

    let draft = ModelRequestDraft {
        model: locked.profile.model.clone(),
        messages: Arc::from([]),
        tools: Arc::from([]),
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: crate::ModelRequestLimits {
            max_input_bytes: locked.profile.hard_input_bytes,
            max_input_tokens: locked
                .profile
                .context_window_tokens
                .saturating_sub(locked.profile.reserved_output_tokens)
                .saturating_sub(locked.profile.provider_overhead_tokens),
            max_output_tokens: locked.profile.reserved_output_tokens,
        },
    };
    let request_json = RawJson::parse(draft.canonical_bytes().expect("canonical")).expect("json");
    block_on(handle.submit(
        env(50, 2, 1, 1, 0, 1, 0),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeModel,
            },
            outcome: ReducerStageOutcome::ModelRequestPrepared {
                request: request_json,
                component: None,
                output_contract: crate::EffectOutputContract {
                    kind: crate::EffectOutputKind::ModelResponse,
                    schema_version: 1,
                    schema_digest: Digest::raw_json(
                        b"{\"kind\":\"model_response\",\"schema_version\":1}",
                    ),
                },
                retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
                deadline: None,
            },
        }),
    ))
    .expect("model request");
    assert_eq!(model.requests.load(Ordering::Acquire), 1);

    let batch = block_on(events.next_batch()).expect("event batch");
    assert!(!batch.events().is_empty());
    assert!(batch.first_sequence() <= batch.last_sequence());
    let _ = block_on(owner.shutdown());
    let _ = ModelProgress::Text(Arc::from("keep"));
}
