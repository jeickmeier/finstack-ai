// ---- BeforeToolBatch middleware --------------------------------------
//
// The one facade stage that never settles through `submit_command`:
// `prepare_tool_batch_if_ready` settles it, so its chain runs here and
// lands through `decide_plan`'s `middleware` parameter rather than as an
// aggregate `ReducerStageOutcome`.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::task::{Context as TaskContext, Poll, Waker};

use finstack_ai_kernel::{
    AcceptRun, AppendRequest, AssignedToolCall, CommittedBatch, ComponentInvocation,
    EffectOutputContract, InvocationRecovery, RecordBody, RecordDraft, RecordEnvelope,
    ToolBatchOpened, ToolExecutionMode, ToolId,
};

use crate::middleware::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRegistration, MiddlewareRole, OrderTier, ResolvedMiddlewareChain, StageInput,
    StageMask, StageOutcome,
};
use crate::middleware_driver::StageDriver;
use crate::{
    ApprovalMetadata, ApprovalRequirement, JournalStore, JsonSchemaToolValidatorCompiler,
    LoadRequest, LoadedSession, PortFuture, SideEffectClass, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth, ToolCallContext, ToolEventStream, ToolExecutionPolicy,
    ToolPolicyDecision, ToolSpec, ToolsetRegistration,
};

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = TaskContext::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
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
        now: fixed_timestamp(now),
        ids: AllocatedIds::try_new(
            records.iter().copied().map(fixed_id).collect(),
            events.iter().copied().map(fixed_id).collect(),
            effects.iter().copied().map(fixed_id).collect(),
            Vec::new(),
            messages.iter().copied().map(fixed_id).collect(),
            turns.iter().copied().map(fixed_id).collect(),
            model_requests.iter().copied().map(fixed_id).collect(),
            Vec::new(),
            Vec::new(),
            vec![fixed_id(append_batch)],
            Vec::new(),
        )
        .expect("allocated ids"),
    }
}

fn model_settled_env(now: i64, message: u64, tool_calls: &[u64]) -> TransitionEnv {
    TransitionEnv {
        now: fixed_timestamp(now),
        ids: AllocatedIds::try_new(
            vec![fixed_id(607), fixed_id(608)],
            vec![fixed_id(603), fixed_id(604)],
            Vec::new(),
            Vec::new(),
            vec![fixed_id(message)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tool_calls.iter().copied().map(fixed_id).collect(),
            vec![fixed_id(605)],
            Vec::new(),
        )
        .expect("model settled ids"),
    }
}

// -- in-memory journal --------------------------------------------------

struct MemoryStore {
    inner: Mutex<MemoryInner>,
}

#[derive(Default)]
struct MemoryInner {
    batches: Vec<CommittedBatch>,
    drafts: Vec<RecordDraft>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryInner::default()),
        }
    }

    /// The single durable `ToolBatchOpened`, i.e. the kernel's own record
    /// of the complete source-ordered assigned plan. Read from the journal
    /// rather than from `state.active_tool_batch` because a batch of
    /// nothing but synthetic closures opens and closes in one transition,
    /// leaving no active batch behind to inspect.
    fn opened_tool_batch(&self) -> Option<ToolBatchOpened> {
        self.inner
            .lock()
            .expect("lock")
            .drafts
            .iter()
            .find_map(|draft| match draft.body() {
                RecordBody::ToolBatchOpened(opened) => Some(opened.clone()),
                _ => None,
            })
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
        let committed = commit_request(&request);
        let mut inner = self.inner.lock().expect("lock");
        inner.drafts.extend(request.records().iter().cloned());
        inner.batches.push(committed.clone());
        Box::pin(async move { Ok(committed) })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let inner = self.inner.lock().expect("lock");
        let batches = inner.batches.clone();
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

/// These tests stop at batch opening, so nothing is ever executed — but a
/// coordinator with no dispatcher at all faults the moment the kernel
/// commits an effect, which would mask the behaviour under test.
struct NoopDispatcher;

impl crate::coordinator::PostCommitDispatcher for NoopDispatcher {
    fn dispatch(
        &self,
        _dispatch: crate::coordinator::RuntimeDispatch,
    ) -> PortFuture<Result<(), crate::coordinator::DispatchError>> {
        Box::pin(async { Ok(()) })
    }
}

// -- tool catalog -------------------------------------------------------

const TOOL_NAMES: [&str; 3] = ["alpha", "beta", "gamma"];

fn tool_id(name: &str) -> ToolId {
    ToolId::parse(format!("finstack.tools.{name}")).expect("tool id")
}

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        id: tool_id(name),
        model_name: Arc::from(name),
        title: Arc::from(name),
        description: Arc::from("fixture tool"),
        input_schema: RawJson::parse(br#"{"type":"object"}"#).expect("input schema"),
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
        deferral: crate::model::ToolDeferralSupport::Never,
    }
}

/// A registered but never-dispatched toolset: these tests stop at batch
/// opening, which is where the `BeforeToolBatch` decision lands.
struct FixtureToolset {
    specs: Arc<[ToolSpec]>,
}

impl crate::Toolset for FixtureToolset {
    fn descriptor(&self) -> crate::ToolsetDescriptor {
        crate::ToolsetDescriptor {
            name: Arc::from("fixture.toolset"),
            metadata: Metadata::empty(),
        }
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.specs)
    }

    fn call(
        &self,
        _ctx: ToolCallContext,
        _call: finstack_ai_kernel::ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        Box::pin(async {
            Err(ToolError::stable(
                "fixture_tool_never_dispatched",
                "the fixture stops at batch opening",
            ))
        })
    }
}

fn catalog() -> ResolvedToolCatalog {
    let specs: Arc<[ToolSpec]> = TOOL_NAMES.iter().copied().map(tool_spec).collect();
    let policies = specs
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                ToolExecutionPolicy {
                    failure_policy: ToolFailurePolicy::ReturnToModel,
                    approval: ToolPolicyDecision::Allow,
                    max_concurrency: 1,
                },
            )
        })
        .collect();
    ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset: Arc::new(FixtureToolset { specs }),
            policies,
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &JsonSchemaToolValidatorCompiler,
    )
    .expect("catalog")
}

// -- middleware ---------------------------------------------------------

fn descriptor(component: &str, stage: Stage) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: ComponentInvocation {
            component: ComponentId::parse(component).expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery: InvocationRecovery::RecomputeSafe,
        },
        stages: StageMask::from_stages([stage]),
        order: MiddlewareOrder {
            tier: OrderTier::Standard,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        role: MiddlewareRole::Standard,
        metadata: Metadata::empty(),
    }
}

/// A component that returns one fixed outcome and counts its invocations,
/// so a test can tell "ran and contributed nothing" from "never ran".
struct Fixed {
    descriptor: MiddlewareDescriptor,
    outcome: StageOutcome,
    calls: Arc<AtomicUsize>,
}

impl Middleware for Fixed {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let outcome = self.outcome.clone();
        Box::pin(async move { Ok(outcome) })
    }
}

fn driver_for(stage: Stage, outcome: StageOutcome, calls: &Arc<AtomicUsize>) -> StageDriver {
    let middleware: Arc<dyn Middleware> = Arc::new(Fixed {
        descriptor: descriptor("fixture.tool-policy", stage),
        outcome,
        calls: Arc::clone(calls),
    });
    StageDriver::new(
        Arc::new(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                .expect("chain"),
        ),
        CancellationSignal::new(),
    )
}

fn retain(names: &[&str]) -> StageOutcome {
    StageOutcome::FilterTools(names.iter().copied().map(tool_id).collect())
}

// -- driving a coordinator to BeforeToolBatch ---------------------------

fn tool_call(ordinal: u64, name: &str) -> ToolCallBlock {
    ToolCallBlock::try_new(
        fixed_id(ordinal),
        name,
        RawJson::parse(b"{}").expect("arguments"),
    )
    .expect("tool call")
}

fn model_output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

/// Drive a fresh coordinator all the way to `RunPhase::BeforeToolBatch`
/// with an assistant message carrying one tool call per name in `names`.
fn coordinator_at_before_tool_batch(
    store: &Arc<MemoryStore>,
    names: &[&str],
    deadline: Option<Timestamp>,
) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(Arc::clone(store) as Arc<dyn JournalStore>);
    coordinator.install_dispatcher(Arc::new(NoopDispatcher));
    block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        KernelInput::AcceptRun(AcceptRun {
            session_id: fixed_id::<SessionTag>(1),
            lane_id: fixed_id::<LaneTag>(2),
            accepted: acceptance(deadline),
        }),
    ))
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
    let user = Message::try_new(
        fixed_id(4),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("call the tools").expect("text"),
        )],
        fixed_timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("user message");
    block_on(coordinator.submit(
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([user]),
            },
        }),
    ))
    .expect("context");
    block_on(coordinator.submit(
        env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeModel,
            },
            outcome: ReducerStageOutcome::ModelRequestPrepared {
                request: RawJson::parse(br#"{"messages":[]}"#).expect("request"),
                component: None,
                output_contract: model_output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
        }),
    ))
    .expect("model request");
    settle_model_with_tool_calls(&mut coordinator, names);
    block_on(coordinator.submit(
        env(1_500, &[609], &[], &[], &[], &[], &[], 606),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::AfterModel,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    ))
    .expect("after model");
    assert_eq!(
        coordinator.state().phase,
        Some(RunPhase::BeforeToolBatch),
        "the fixture must park the run exactly at the BeforeToolBatch cursor"
    );
    coordinator
}

/// Settle the outstanding model effect with an assistant message carrying
/// one tool call per name, ordinals 301, 302, ... in source order.
fn settle_model_with_tool_calls(coordinator: &mut CommitCoordinator, names: &[&str]) {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending model effect")
        .clone();
    let call_ordinals = (0..names.len())
        .map(|index| 301 + u64::try_from(index).expect("index"))
        .collect::<Vec<_>>();
    let mut content = vec![ContentBlock::Text(
        TextBlock::try_new("calling").expect("text"),
    )];
    content.extend(
        names
            .iter()
            .zip(&call_ordinals)
            .map(|(name, ordinal)| ContentBlock::ToolCall(tool_call(*ordinal, name))),
    );
    let assistant = Message::try_new(
        fixed_id(617),
        MessageRole::Assistant,
        content,
        fixed_timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant message");
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        model_output_contract(),
        RawJson::parse(br#"{"text":"calling"}"#).expect("output"),
        None,
        Vec::new(),
        ProviderIds::empty(),
        Some("cmpl-tool"),
        None,
    )
    .expect("completion");
    block_on(coordinator.submit(
        model_settled_env(1_400, 617, &call_ordinals),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            outcome: ModelSettlement::Completed {
                completion,
                assistant_message: assistant,
            },
        }),
    ))
    .expect("model settled");
}

/// Drive to `BeforeToolBatch` and run `prepare_tool_batch_if_ready` with
/// `driver`, returning the durable source-ordered plan the kernel opened.
fn prepare_with(
    names: &[&str],
    driver: Option<&StageDriver>,
) -> (Vec<AssignedToolCall>, CommitCoordinator, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, names, None);
    let sources = sources_at(1_600);
    block_on(prepare_tool_batch_if_ready(
        &mut coordinator,
        &catalog(),
        &sources,
        driver,
    ))
    .expect("tool batch preparation");
    let plans = store
        .opened_tool_batch()
        .expect("a tool batch must have been opened")
        .calls
        .to_vec();
    (plans, coordinator, store)
}

fn planned_call_ids(plans: &[AssignedToolCall]) -> Vec<ToolCallId> {
    plans
        .iter()
        .map(|assigned| *assigned.plan.call().tool_call_id())
        .collect()
}

fn source_call_ids(count: usize) -> Vec<ToolCallId> {
    (0..count)
        .map(|index| fixed_id(301 + u64::try_from(index).expect("index")))
        .collect()
}
