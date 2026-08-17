use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetChargeReceipt, BudgetChargeRequest,
    BudgetPropagation, BudgetReleaseReceipt, BudgetReleaseRequest, CancellationPropagation,
    ContentBlock, DeadlinePropagation, Digest, EffectCompleted, EffectOutputContract,
    EffectOutputKind, Id, IdTag, LaneCreated, LaneTag, Message, MessageRole, Metadata,
    ModelSettled, ModelSettlement, PostCommitAction, PrincipalPropagation, PrincipalRef,
    ProviderIds, RawJson, RecordBody, RecordEnvelope, ReducerStageOutcome, RetrySafety,
    RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext, SessionTag,
    Stage, StageCursor, TextBlock, Timestamp, Usage,
};

use super::session_commit::session_draft;
use super::*;
use crate::{
    AgentInvokeError, AgentInvoker, AgentRef, AuthorizationContext, BudgetCoordinator, BudgetError,
    BudgetLedger, BudgetOperationIds, BudgetRequest, BudgetReservationReceipt,
    BudgetReservationState, BudgetReserveRequest, ChildCoordinationIds, ChildPlacement,
    ChildRunContext, ChildRunCoordinator, ChildRunHandle, ChildRunLocator, ChildRunRequest,
    CompositionError, LoadRequest, LoadedSession, OperationLocator, PortFuture, SnapshotReceipt,
    SnapshotRequest, StateSnapshotRequest, StoreHealth, child_relation_digest,
};

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
        .expect("allocated ids"),
    }
}

fn acceptance() -> RunAccepted {
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
    .expect("acceptance")
}

fn accept_input() -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: acceptance(),
    })
}

fn create_compatible_lane(commit: &mut CommitCoordinator, lane: u64, batch: u64, record: u64) {
    block_on(commit.commit_session_records(
        id(batch),
        vec![session_draft(
                id(record),
                id::<SessionTag>(1),
                id(lane),
                timestamp(1_050),
                RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
            )
            .expect("lane draft")],
    ))
    .expect("create child lane");
}

fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

fn context_message() -> Message {
    Message::try_new(
        id(4),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("Say hello.").expect("text"),
        )],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

async fn drive_to_model_request(coordinator: &mut CommitCoordinator) -> CommitOutcome {
    coordinator
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )
        .await
        .expect("accept");
    drive_accepted_to_model_request(coordinator).await
}

async fn drive_accepted_to_model_request(coordinator: &mut CommitCoordinator) -> CommitOutcome {
    coordinator
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    coordinator
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([context_message()]),
                },
            ),
        )
        .await
        .expect("context");
    coordinator
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: RawJson::parse(
                        r#"{"messages":[{"role":"user","text":"Say hello."}]}"#,
                    )
                    .expect("request"),
                    component: None,
                    output_contract: output_contract(),
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(8_000)),
                },
            ),
        )
        .await
        .expect("model request")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FakeMode {
    Normal,
    ConflictOnce,
    ConflictAlways,
    AmbiguousOnce,
    AmbiguousAlways,
    IntegrityAlways,
}

struct FakeStore {
    inner: Mutex<FakeInner>,
    composition_log: Option<Arc<Mutex<Vec<&'static str>>>>,
}

struct FakeInner {
    mode: FakeMode,
    append_calls: usize,
    batches: Vec<CommittedBatch>,
    requests: BTreeMap<finstack_ai_kernel::AppendBatchId, AppendRequest>,
}

#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
struct StallingSnapshotStore {
    inner: FakeStore,
}

impl JournalStore for StallingSnapshotStore {
    fn append(
        &self,
        request: finstack_ai_kernel::AppendRequest,
    ) -> PortFuture<Result<finstack_ai_kernel::CommittedBatch, StoreError>> {
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

    fn write_state_snapshot(
        &self,
        _request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            #[cfg(feature = "native-tokio")]
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Err(StoreError::Unavailable {
                reason_code: "snapshot_write_stalled",
            })
        })
    }
}

impl FakeStore {
    fn new(mode: FakeMode) -> Self {
        Self {
            inner: Mutex::new(FakeInner {
                mode,
                append_calls: 0,
                batches: Vec::new(),
                requests: BTreeMap::new(),
            }),
            composition_log: None,
        }
    }

    fn with_composition_log(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
        let mut store = Self::new(FakeMode::Normal);
        store.composition_log = Some(log);
        store
    }

    fn append_calls(&self) -> usize {
        self.inner.lock().expect("lock").append_calls
    }
}

impl JournalStore for FakeStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        if let Some(log) = &self.composition_log {
            for record in request.records() {
                let label = match record.body() {
                    RecordBody::ChildRunPrepared(_) => Some("prepare_committed"),
                    RecordBody::BudgetReservationRequested(_) => Some("reservation_requested"),
                    RecordBody::BudgetReservationSettled(_) => Some("reservation_settled"),
                    _ => None,
                };
                if let Some(label) = label {
                    log.lock().expect("log").push(label);
                }
            }
        }
        let mut inner = self.inner.lock().expect("lock");
        inner.append_calls += 1;
        if let Some(existing) = inner.requests.get(&request.batch_id()) {
            if existing != &request {
                return Box::pin(async {
                    Err(StoreError::Corruption {
                        reason_code: "batch_reuse",
                    })
                });
            }
            let committed = inner
                .batches
                .iter()
                .find(|batch| batch.batch_id == request.batch_id())
                .cloned()
                .expect("indexed batch");
            if inner.mode == FakeMode::AmbiguousAlways {
                return Box::pin(async { Err(StoreError::AmbiguousAcknowledgement) });
            }
            return Box::pin(async move { Ok(committed) });
        }
        if inner.mode == FakeMode::IntegrityAlways {
            return Box::pin(async {
                Err(StoreError::Integrity {
                    reason_code: "checksum_mismatch",
                })
            });
        }
        if inner.mode == FakeMode::ConflictAlways
            || (inner.mode == FakeMode::ConflictOnce && inner.append_calls == 1)
        {
            return Box::pin(async {
                Err(StoreError::Conflict {
                    expected_sequence: 1,
                    actual_next_sequence: 1,
                })
            });
        }
        let committed = commit_request(&request);
        inner.requests.insert(request.batch_id(), request);
        inner.batches.push(committed.clone());
        if matches!(
            inner.mode,
            FakeMode::AmbiguousOnce | FakeMode::AmbiguousAlways
        ) {
            return Box::pin(async { Err(StoreError::AmbiguousAcknowledgement) });
        }
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

struct AmbiguousReserveLedger {
    log: Arc<Mutex<Vec<&'static str>>>,
    receipt: BudgetReservationReceipt,
    charge_receipt: Option<finstack_ai_kernel::BudgetChargeReceipt>,
    release_receipt: Option<finstack_ai_kernel::BudgetReleaseReceipt>,
    reserved: Mutex<bool>,
    fail_after_reserve_once: Mutex<bool>,
    reserve_calls: Mutex<usize>,
    charge_calls: Mutex<usize>,
    release_calls: Mutex<usize>,
}

impl BudgetLedger for AmbiguousReserveLedger {
    fn reserve(
        &self,
        request: BudgetReserveRequest,
    ) -> PortFuture<Result<BudgetReservationReceipt, BudgetError>> {
        self.log.lock().expect("log").push("reserve");
        *self.reserve_calls.lock().expect("calls") += 1;
        *self.reserved.lock().expect("reserved") = true;
        let fail = std::mem::take(&mut *self.fail_after_reserve_once.lock().expect("fail"));
        let receipt = self.receipt.clone();
        assert_eq!(request.request_digest, receipt.request_digest);
        Box::pin(async move {
            if fail {
                Err(BudgetError::Unavailable {
                    message: Arc::from("ambiguous reserve acknowledgement"),
                })
            } else {
                Ok(receipt)
            }
        })
    }

    fn reconcile(
        &self,
        scope_id: finstack_ai_kernel::BudgetScopeId,
        reservation_id: finstack_ai_kernel::BudgetReservationId,
    ) -> PortFuture<Result<BudgetReservationState, BudgetError>> {
        self.log.lock().expect("log").push("reconcile");
        let reserved = *self.reserved.lock().expect("reserved");
        let receipt = self.receipt.clone();
        assert_eq!(scope_id, receipt.scope_id);
        assert_eq!(reservation_id, receipt.reservation_id);
        Box::pin(async move {
            Ok(if reserved {
                BudgetReservationState::Reserved(receipt)
            } else {
                BudgetReservationState::NotFound
            })
        })
    }

    fn charge(
        &self,
        request: finstack_ai_kernel::BudgetChargeRequest,
    ) -> PortFuture<Result<finstack_ai_kernel::BudgetChargeReceipt, BudgetError>> {
        *self.charge_calls.lock().expect("calls") += 1;
        let receipt = self.charge_receipt.clone();
        Box::pin(async move {
            receipt.map_or_else(
                || {
                    Err(BudgetError::InvalidRequest {
                        message: Arc::from("charge not configured"),
                    })
                },
                |receipt| {
                    assert_eq!(receipt.effect_id, request.effect_id);
                    Ok(receipt)
                },
            )
        })
    }

    fn release(
        &self,
        request: finstack_ai_kernel::BudgetReleaseRequest,
    ) -> PortFuture<Result<finstack_ai_kernel::BudgetReleaseReceipt, BudgetError>> {
        *self.release_calls.lock().expect("calls") += 1;
        let receipt = self.release_receipt.clone();
        Box::pin(async move {
            receipt.map_or_else(
                || {
                    Err(BudgetError::InvalidRequest {
                        message: Arc::from("release not configured"),
                    })
                },
                |receipt| {
                    assert_eq!(receipt.terminal_run_id, request.terminal_run_id);
                    Ok(receipt)
                },
            )
        })
    }
}

struct IdempotentChildInvoker {
    log: Arc<Mutex<Vec<&'static str>>>,
    accepted: Mutex<BTreeMap<EffectId, Digest>>,
    physical_starts: Mutex<usize>,
}

impl AgentInvoker for IdempotentChildInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        self.log.lock().expect("log").push("invoke");
        let relation_digest = child_relation_digest(&context, &request).expect("relation");
        let mut accepted = self.accepted.lock().expect("accepted");
        match accepted.get(&context.parent_effect_id) {
            Some(existing) if *existing != request.request_digest => {
                let existing = *existing;
                let submitted = request.request_digest;
                return Box::pin(async move {
                    Err(AgentInvokeError::Conflict {
                        existing,
                        submitted,
                    })
                });
            }
            Some(_) => {}
            None => {
                accepted.insert(context.parent_effect_id, request.request_digest);
                *self.physical_starts.lock().expect("starts") += 1;
            }
        }
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
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
    .expect("batch")
}

#[derive(Default)]
struct RecordingDispatcher {
    actions: Mutex<Vec<PostCommitAction>>,
}

impl PostCommitDispatcher for RecordingDispatcher {
    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        self.actions.lock().expect("lock").push(dispatch.action);
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn append_and_apply_precede_test_dispatch() {
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let dispatcher = Arc::new(RecordingDispatcher::default());
    let mut coordinator =
        CommitCoordinator::with_test_dispatcher(store.clone(), dispatcher.clone());
    let outcome = block_on(drive_to_model_request(&mut coordinator));
    assert!(outcome.committed.is_some());
    assert_eq!(outcome.dispatched_actions, 1);
    assert!(outcome.fault.is_none());
    assert_eq!(store.append_calls(), 4);
    assert_eq!(dispatcher.actions.lock().expect("lock").len(), 1);
    assert_eq!(
        coordinator.state().phase,
        Some(finstack_ai_kernel::RunPhase::AwaitingModel)
    );
}

#[cfg(feature = "native-tokio")]
#[tokio::test]
async fn manual_drive_exposes_a_recoverable_committed_prefix_before_dispatch() {
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let dispatcher = Arc::new(RecordingDispatcher::default());
    let mut coordinator =
        CommitCoordinator::with_test_dispatcher(store.clone(), dispatcher.clone());
    let mut controller = coordinator.enable_manual_drive(1).expect("manual drive");

    let blocked = tokio::spawn(async move {
        let outcome = drive_to_model_request(&mut coordinator).await;
        (coordinator, outcome)
    });
    let permit = controller.next_effect().await.expect("paused dispatch");

    assert_eq!(permit.effect().action, crate::ManualDriveAction::Execute);
    assert_eq!(store.append_calls(), 4);
    assert!(dispatcher.actions.lock().expect("lock").is_empty());
    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover committed prefix");
    assert_eq!(
        recovered.state().phase,
        Some(finstack_ai_kernel::RunPhase::AwaitingModel)
    );
    assert!(recovered.state().pending_model_effect.is_some());

    blocked.abort();
    assert!(matches!(blocked.await, Err(error) if error.is_cancelled()));
    drop(permit);
    assert!(dispatcher.actions.lock().expect("lock").is_empty());
}

#[cfg(feature = "native-tokio")]
#[tokio::test]
async fn manual_drive_releases_exactly_the_observed_committed_action() {
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let dispatcher = Arc::new(RecordingDispatcher::default());
    let mut coordinator = CommitCoordinator::with_test_dispatcher(store, dispatcher.clone());
    let mut controller = coordinator.enable_manual_drive(1).expect("manual drive");
    let blocked = tokio::spawn(async move { drive_to_model_request(&mut coordinator).await });

    let permit = controller.next_effect().await.expect("paused dispatch");
    let effect = permit.effect();
    permit.continue_dispatch();
    let outcome = blocked.await.expect("join");

    assert!(outcome.fault.is_none());
    assert_eq!(outcome.dispatched_actions, 1);
    assert_eq!(
        dispatcher.actions.lock().expect("lock").as_slice(),
        &[PostCommitAction::ExecuteEffect {
            effect_id: effect.effect_id,
        }]
    );
}

#[cfg(feature = "native-tokio")]
#[tokio::test]
async fn snapshot_write_timeout_does_not_fail_or_stall_submit() {
    let store = Arc::new(StallingSnapshotStore {
        inner: FakeStore::new(FakeMode::Normal),
    });
    let mut coordinator = CommitCoordinator::new(store).with_snapshot_schedule(SnapshotSchedule {
        every_n_records: 1,
        write_timeout: std::time::Duration::from_millis(50),
    });
    let started = std::time::Instant::now();
    coordinator
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )
        .await
        .expect("accept");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(150),
        "snapshot write must not block submit beyond write_timeout"
    );
    assert!(coordinator.fault().is_none());
}

#[test]
fn unsupported_driver_faults_only_after_committed_request() {
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let mut coordinator = CommitCoordinator::new(store.clone());
    let outcome = block_on(drive_to_model_request(&mut coordinator));
    assert_eq!(
        outcome.fault,
        Some(RunFault {
            code: "effect_driver_unavailable"
        })
    );
    assert_eq!(store.append_calls(), 4);
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_400, &[], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )),
        Err(CommitCoordinatorError::Faulted {
            code: "effect_driver_unavailable"
        })
    ));
}

#[test]
fn duplicate_decision_skips_append_and_recovery_matches_live_state() {
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let mut coordinator = CommitCoordinator::new(store.clone());
    let accepted_env = env(1_000, &[1], &[1], &[], &[], &[], &[], 101);
    block_on(coordinator.submit(accepted_env, accept_input())).expect("accept");
    let stage_env = env(1_100, &[2], &[], &[], &[], &[], &[], 102);
    let stage_input = stage(Stage::BeforeRun, ReducerStageOutcome::Continue);
    block_on(coordinator.submit(stage_env.clone(), stage_input.clone())).expect("before run");
    let calls = store.append_calls();
    let duplicate = block_on(coordinator.submit(stage_env, stage_input)).expect("duplicate");
    assert!(duplicate.committed.is_none());
    assert_eq!(store.append_calls(), calls);
    let live_hash = coordinator.state().state_hash().expect("hash");
    let recovered =
        block_on(CommitCoordinator::recover(store, id::<SessionTag>(1))).expect("recover");
    assert_eq!(recovered.state().state_hash().expect("hash"), live_hash);
}

#[test]
fn one_conflict_reloads_and_repeated_conflict_faults() {
    let once = Arc::new(FakeStore::new(FakeMode::ConflictOnce));
    let mut coordinator = CommitCoordinator::new(once.clone());
    block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("retry after conflict");
    assert_eq!(once.append_calls(), 2);

    let repeated = Arc::new(FakeStore::new(FakeMode::ConflictAlways));
    let mut coordinator = CommitCoordinator::new(repeated);
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "repeated_store_conflict"
        })
    ));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the crash-recovery scenario is clearer as one chronological proof"
)]
fn child_retry_reconciles_ambiguous_reservation_before_invoke() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(FakeStore::with_composition_log(log.clone()));
    let mut commit = CommitCoordinator::new(store);
    block_on(commit.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("accept parent");
    create_compatible_lane(&mut commit, 40, 190, 191);

    let parent = OperationLocator {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
    };
    let child_locator = ChildRunLocator {
        operation: OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(40),
            run_id: id(41),
        },
        remote: None,
    };
    let budget = BudgetRequest {
        input_tokens: Some(1_000),
        output_tokens: Some(250),
        cost: None,
        extension_counters: BTreeMap::new(),
    };
    let scope_id = id(42);
    let reservation_id = id(43);
    let reserve_digest = BudgetReserveRequest::compute_digest(
        scope_id,
        reservation_id,
        child_locator.operation.run_id,
        &budget,
    )
    .expect("reserve digest");
    let reserve = BudgetReserveRequest {
        scope_id,
        reservation_id,
        run_id: child_locator.operation.run_id,
        amount: budget.clone(),
        request_digest: reserve_digest,
    };
    let receipt = BudgetReservationReceipt {
        scope_id,
        reservation_id,
        reserved: budget.clone(),
        remaining: BudgetRequest::default(),
        request_digest: reserve_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
    };
    let ledger = Arc::new(AmbiguousReserveLedger {
        log: log.clone(),
        receipt,
        charge_receipt: None,
        release_receipt: None,
        reserved: Mutex::new(false),
        fail_after_reserve_once: Mutex::new(true),
        reserve_calls: Mutex::new(0),
        charge_calls: Mutex::new(0),
        release_calls: Mutex::new(0),
    });
    let invoker = Arc::new(IdempotentChildInvoker {
        log: log.clone(),
        accepted: Mutex::new(BTreeMap::new()),
        physical_starts: Mutex::new(0),
    });
    let coordinator = ChildRunCoordinator::new(invoker.clone()).with_budget_ledger(ledger.clone());
    let context = ChildRunContext {
        parent: parent.clone(),
        parent_effect_id: id(44),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authentication_method: Arc::from("oidc"),
            assurance_level: Arc::from("high"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("policy-v1"),
            decision_id: Arc::from("decision-v1"),
        },
    };
    let request = ChildRunRequest {
        agent: AgentRef {
            id: crate::AgentId::parse("finstack.agent.child").expect("agent id"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
        },
        input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("do the work").expect("text"),
        )]),
        placement: ChildPlacement::CompatibleLaneInParentSession,
        locator: child_locator,
        requested_deadline: Some(timestamp(5_000)),
        requested_budget: budget,
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    };
    let ids = ChildCoordinationIds {
        preparation_batch_id: id(201),
        preparation_record_id: id(202),
        reservation_request_record_id: Some(id(203)),
        reservation_settlement: Some(BudgetOperationIds {
            batch_id: id(204),
            record_id: id(205),
        }),
    };

    assert!(matches!(
        block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            Some(reserve.clone()),
            ids,
            timestamp(1_100),
        )),
        Err(CompositionError::Budget(BudgetError::Unavailable { .. }))
    ));
    assert!(!log.lock().expect("log").contains(&"invoke"));

    let first = block_on(coordinator.start_or_attach(
        &mut commit,
        context.clone(),
        request.clone(),
        Some(reserve.clone()),
        ids,
        timestamp(1_100),
    ))
    .expect("reconcile and invoke");
    let attached = block_on(coordinator.start_or_attach(
        &mut commit,
        context.clone(),
        request.clone(),
        Some(reserve.clone()),
        ids,
        timestamp(1_100),
    ))
    .expect("attach equal retry");
    assert_eq!(first, attached);
    assert_eq!(*ledger.reserve_calls.lock().expect("calls"), 1);
    assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
    assert_eq!(
        log.lock().expect("log").as_slice(),
        &[
            "prepare_committed",
            "reservation_requested",
            "reconcile",
            "reserve",
            "reconcile",
            "reservation_settled",
            "invoke",
            "invoke",
        ]
    );

    let mut conflicting = request;
    conflicting.request_digest = Digest::raw_json(br#"{"request":"child-b"}"#);
    assert!(matches!(
        block_on(coordinator.start_or_attach(
            &mut commit,
            context,
            conflicting,
            Some(reserve),
            ids,
            timestamp(1_100),
        )),
        Err(CompositionError::Commit(
            CommitCoordinatorError::SidecarConflict
        ))
    ));
    assert_eq!(*ledger.reserve_calls.lock().expect("calls"), 1);
    assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "all placement policies share one table-driven handshake proof"
)]
fn every_child_placement_converges_and_rejects_conflicting_digest() {
    let parent = OperationLocator {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
    };
    let remote = crate::RemoteRouteRef {
        service: crate::ComponentRef::new(
            crate::ComponentId::parse("finstack.remote.worker").expect("service"),
            Some(crate::Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        route: crate::ExternalHandleRef::try_new(
            crate::ComponentId::parse("finstack.remote.worker").expect("provider"),
            "route-a",
            RawJson::parse(r#"{"cluster":"a"}"#).expect("route metadata"),
        )
        .expect("route"),
    };
    let cases = [
        (
            ChildPlacement::CompatibleLaneInParentSession,
            ChildRunLocator {
                operation: OperationLocator {
                    tenant_scope: Arc::from("tenant-a"),
                    session_id: id(1),
                    lane_id: id(50),
                    run_id: id(51),
                },
                remote: None,
            },
        ),
        (
            ChildPlacement::IsolatedChildSession,
            ChildRunLocator {
                operation: OperationLocator {
                    tenant_scope: Arc::from("tenant-a"),
                    session_id: id(60),
                    lane_id: id(61),
                    run_id: id(62),
                },
                remote: None,
            },
        ),
        (
            ChildPlacement::RemoteChildSession,
            ChildRunLocator {
                operation: OperationLocator {
                    tenant_scope: Arc::from("tenant-a"),
                    session_id: id(70),
                    lane_id: id(71),
                    run_id: id(72),
                },
                remote: Some(remote),
            },
        ),
    ];

    for (index, (placement, locator)) in cases.into_iter().enumerate() {
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let mut commit = CommitCoordinator::new(store);
        block_on(commit.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        ))
        .expect("accept parent");
        if placement == ChildPlacement::CompatibleLaneInParentSession {
            create_compatible_lane(&mut commit, 50, 190, 191);
        }
        let invoker = Arc::new(IdempotentChildInvoker {
            log: Arc::new(Mutex::new(Vec::new())),
            accepted: Mutex::new(BTreeMap::new()),
            physical_starts: Mutex::new(0),
        });
        let coordinator = ChildRunCoordinator::new(invoker.clone());
        let context = ChildRunContext {
            parent: parent.clone(),
            parent_effect_id: id(80 + u64::try_from(index).expect("index")),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: Arc::from("oidc"),
                assurance_level: Arc::from("high"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
        };
        let request = ChildRunRequest {
            agent: AgentRef {
                id: crate::AgentId::parse("finstack.agent.placement").expect("agent id"),
                bundle: None,
                spec_digest: Digest::raw_json(br#"{"agent":"placement"}"#),
            },
            input: Arc::from([]),
            placement,
            locator,
            requested_deadline: None,
            requested_budget: BudgetRequest::default(),
            delegation_id: None,
            metadata: Metadata::empty(),
            request_digest: Digest::raw_json(format!(r#"{{"placement":{index}}}"#).as_bytes()),
        };
        let ordinal = 500 + u64::try_from(index).expect("index") * 10;
        let ids = ChildCoordinationIds {
            preparation_batch_id: id(ordinal),
            preparation_record_id: id(ordinal + 1),
            reservation_request_record_id: None,
            reservation_settlement: None,
        };
        let first = block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            None,
            ids,
            timestamp(1_100),
        ))
        .expect("start child");
        let attached = block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            None,
            ids,
            timestamp(1_100),
        ))
        .expect("attach child");
        assert_eq!(first, attached);
        assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
        assert_eq!(
            commit
                .state()
                .child_preparations
                .get(&context.parent_effect_id)
                .map(|prepared| &prepared.child),
            Some(&request.locator)
        );

        let mut conflicting = request;
        conflicting.request_digest = Digest::raw_json(b"conflicting child request");
        assert!(matches!(
            block_on(coordinator.start_or_attach(
                &mut commit,
                context,
                conflicting,
                None,
                ids,
                timestamp(1_100),
            )),
            Err(CompositionError::Commit(
                CommitCoordinatorError::SidecarConflict
            ))
        ));
        assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the post-commit charge and release proof intentionally covers one lifecycle"
)]
fn budget_charge_and_release_are_post_commit_and_idempotent() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let dispatcher = Arc::new(RecordingDispatcher::default());
    let mut commit = CommitCoordinator::with_test_dispatcher(store, dispatcher);
    block_on(commit.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("accept parent");
    create_compatible_lane(&mut commit, 340, 390, 391);

    let parent = OperationLocator {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
    };
    let child_locator = ChildRunLocator {
        operation: OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(340),
            run_id: id(341),
        },
        remote: None,
    };
    let budget = BudgetRequest {
        input_tokens: Some(1_000),
        output_tokens: Some(250),
        cost: None,
        extension_counters: BTreeMap::new(),
    };
    let scope_id = id(342);
    let reservation_id = id(343);
    let reserve_digest = BudgetReserveRequest::compute_digest(
        scope_id,
        reservation_id,
        child_locator.operation.run_id,
        &budget,
    )
    .expect("reserve digest");
    let reserve = BudgetReserveRequest {
        scope_id,
        reservation_id,
        run_id: child_locator.operation.run_id,
        amount: budget.clone(),
        request_digest: reserve_digest,
    };
    let reserve_receipt = BudgetReservationReceipt {
        scope_id,
        reservation_id,
        reserved: budget.clone(),
        remaining: BudgetRequest::default(),
        request_digest: reserve_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
    };
    let usage = Usage::try_new(Some(20), Some(10), Some(30), None, BTreeMap::new()).expect("usage");
    let usage_digest = Digest::effect_output(&usage.canonical_bytes().expect("usage bytes"));
    let charge_request = BudgetChargeRequest {
        scope_id,
        reservation_id,
        effect_id: id(103),
        usage: usage.clone(),
        usage_digest,
    };
    let charge_receipt = BudgetChargeReceipt {
        scope_id,
        reservation_id,
        effect_id: id(103),
        charged_usage: usage.clone(),
        cumulative_usage: usage.clone(),
        usage_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"charge"}"#),
    };
    let release_digest = BudgetReleaseRequest::compute_digest(scope_id, reservation_id, id(3))
        .expect("release digest");
    let release_request = BudgetReleaseRequest {
        scope_id,
        reservation_id,
        terminal_run_id: id(3),
        request_digest: release_digest,
    };
    let release_receipt = BudgetReleaseReceipt {
        scope_id,
        reservation_id,
        terminal_run_id: id(3),
        released_unused: BudgetRequest::default(),
        request_digest: release_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"release"}"#),
    };
    let ledger = Arc::new(AmbiguousReserveLedger {
        log: log.clone(),
        receipt: reserve_receipt,
        charge_receipt: Some(charge_receipt.clone()),
        release_receipt: Some(release_receipt.clone()),
        reserved: Mutex::new(false),
        fail_after_reserve_once: Mutex::new(false),
        reserve_calls: Mutex::new(0),
        charge_calls: Mutex::new(0),
        release_calls: Mutex::new(0),
    });
    let invoker = Arc::new(IdempotentChildInvoker {
        log,
        accepted: Mutex::new(BTreeMap::new()),
        physical_starts: Mutex::new(0),
    });
    let child_coordinator = ChildRunCoordinator::new(invoker).with_budget_ledger(ledger.clone());
    let child_context = ChildRunContext {
        parent: parent.clone(),
        parent_effect_id: id(344),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authentication_method: Arc::from("oidc"),
            assurance_level: Arc::from("high"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("policy-v1"),
            decision_id: Arc::from("decision-v1"),
        },
    };
    let child_request = ChildRunRequest {
        agent: AgentRef {
            id: crate::AgentId::parse("finstack.agent.budget-child").expect("agent id"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"budget-child"}"#),
        },
        input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("budgeted work").expect("text"),
        )]),
        placement: ChildPlacement::CompatibleLaneInParentSession,
        locator: child_locator,
        requested_deadline: None,
        requested_budget: budget,
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"budget-child"}"#),
    };
    block_on(child_coordinator.start_or_attach(
        &mut commit,
        child_context,
        child_request,
        Some(reserve),
        ChildCoordinationIds {
            preparation_batch_id: id(401),
            preparation_record_id: id(402),
            reservation_request_record_id: Some(id(403)),
            reservation_settlement: Some(BudgetOperationIds {
                batch_id: id(404),
                record_id: id(405),
            }),
        },
        timestamp(1_050),
    ))
    .expect("prepare budgeted child");

    let budget_coordinator = BudgetCoordinator::new(ledger.clone());
    assert!(matches!(
        block_on(budget_coordinator.charge_committed(
            &mut commit,
            &parent,
            charge_request.clone(),
            BudgetOperationIds {
                batch_id: id(406),
                record_id: id(407),
            },
            timestamp(1_350),
        )),
        Err(CompositionError::InvalidRequest {
            code: "effect_usage_not_committed"
        })
    ));
    assert_eq!(*ledger.charge_calls.lock().expect("calls"), 0);

    block_on(drive_accepted_to_model_request(&mut commit));
    let completion = EffectCompleted::try_new(
        id(103),
        output_contract(),
        RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
        Some(usage),
        vec![],
        ProviderIds::empty(),
        Some("budget-completion"),
        None,
    )
    .expect("completion");
    let assistant_message = Message::try_new(
        id(504),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant message");
    block_on(commit.submit(
        env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[504], 105),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: id(101),
            model_request_id: id(102),
            outcome: ModelSettlement::Completed {
                completion,
                assistant_message,
            },
        }),
    ))
    .expect("settle model");

    let charge_ids = BudgetOperationIds {
        batch_id: id(406),
        record_id: id(407),
    };
    let charged = block_on(budget_coordinator.charge_committed(
        &mut commit,
        &parent,
        charge_request.clone(),
        charge_ids,
        timestamp(1_450),
    ))
    .expect("charge committed usage");
    assert_eq!(charged, charge_receipt);
    assert_eq!(
        block_on(budget_coordinator.charge_committed(
            &mut commit,
            &parent,
            charge_request,
            charge_ids,
            timestamp(1_450),
        ))
        .expect("equal charge retry"),
        charge_receipt
    );
    assert_eq!(*ledger.charge_calls.lock().expect("calls"), 1);

    block_on(commit.submit(
        env(1_500, &[9], &[], &[], &[], &[], &[], 106),
        stage(Stage::AfterModel, ReducerStageOutcome::Continue),
    ))
    .expect("after model");
    block_on(commit.submit(
        env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 107),
        stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
    ))
    .expect("terminal commit");

    let release_ids = BudgetOperationIds {
        batch_id: id(408),
        record_id: id(409),
    };
    let released = block_on(budget_coordinator.release_committed(
        &mut commit,
        &parent,
        release_request.clone(),
        release_ids,
        timestamp(1_650),
    ))
    .expect("release after terminal");
    assert_eq!(released, release_receipt);
    assert_eq!(
        block_on(budget_coordinator.release_committed(
            &mut commit,
            &parent,
            release_request,
            release_ids,
            timestamp(1_650),
        ))
        .expect("equal release retry"),
        release_receipt
    );
    assert_eq!(*ledger.release_calls.lock().expect("calls"), 1);
}

#[test]
fn ambiguous_ack_retries_identical_request_once() {
    let once = Arc::new(FakeStore::new(FakeMode::AmbiguousOnce));
    let mut coordinator = CommitCoordinator::new(once.clone());
    block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("ambiguous recovery");
    assert_eq!(once.append_calls(), 2);

    let repeated = Arc::new(FakeStore::new(FakeMode::AmbiguousAlways));
    let mut coordinator = CommitCoordinator::new(repeated);
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
}

#[test]
fn sidecar_composition_ambiguous_ack_faults_later_submit() {
    let store = Arc::new(FakeStore::new(FakeMode::AmbiguousAlways));
    let mut coordinator = CommitCoordinator::new(store);
    let prepared = finstack_ai_kernel::ChildRunPrepared {
        parent_run_id: id(1),
        parent_effect_id: id(2),
        child: ChildRunLocator {
            operation: OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator"),
            remote: None,
        },
        request_digest: Digest::raw_json(b"child"),
        placement: ChildPlacement::CompatibleLaneInParentSession,
        budget_reservation_id: None,
    };
    assert!(matches!(
        block_on(coordinator.commit_composition_records(
            id(394),
            vec![session_draft(
                    id(395),
                    id::<SessionTag>(1),
                    id::<LaneTag>(2),
                    timestamp(1_050),
                    RecordBody::ChildRunPrepared(prepared),
                )
                .expect("composition draft")],
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
    assert_eq!(
        coordinator.fault().map(|fault| fault.code),
        Some("continued_ambiguous_acknowledgement")
    );
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )),
        Err(CommitCoordinatorError::Faulted {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
}

#[test]
fn sidecar_ambiguous_ack_faults_later_submit() {
    let store = Arc::new(FakeStore::new(FakeMode::AmbiguousAlways));
    let mut coordinator = CommitCoordinator::new(store);
    assert!(matches!(
        block_on(coordinator.commit_session_records(
            id(390),
            vec![session_draft(
                    id(391),
                    id::<SessionTag>(1),
                    id::<LaneTag>(2),
                    timestamp(1_050),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane draft")],
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
    assert_eq!(
        coordinator.fault().map(|fault| fault.code),
        Some("continued_ambiguous_acknowledgement")
    );
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )),
        Err(CommitCoordinatorError::Faulted {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
}

#[test]
fn store_integrity_keeps_public_code_and_retains_reason() {
    let store = Arc::new(FakeStore::new(FakeMode::IntegrityAlways));
    let mut coordinator = CommitCoordinator::new(store);
    assert!(matches!(
        block_on(coordinator.commit_session_records(
            id(392),
            vec![session_draft(
                    id(393),
                    id::<SessionTag>(1),
                    id::<LaneTag>(2),
                    timestamp(1_050),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane draft")],
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "store_integrity_uncertain"
        })
    ));
    assert_eq!(
        coordinator.fault().map(|fault| fault.code),
        Some("store_integrity_uncertain")
    );
    assert_eq!(
        coordinator.last_store_reason(),
        Some("store integrity failure: checksum_mismatch")
    );
}

fn poison_mutex<T>(mutex: &Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock().expect("lock");
        panic!("poison");
    }));
}

#[test]
fn cancel_registered_effect_poison_returns_dispatch_error() {
    let active = Mutex::new(BTreeMap::new());
    poison_mutex(&active);
    let error = cancel_registered_effect(&active, id(1), "model_effect_registry_unavailable")
        .expect_err("poisoned registry");
    assert_eq!(error.code, "model_effect_registry_unavailable");
}

#[test]
fn cancel_registered_effect_missing_id_is_idempotent() {
    let active = Mutex::new(BTreeMap::new());
    assert!(
        cancel_registered_effect(&active, id(1), "model_effect_registry_unavailable")
            .expect("lock")
            .is_none()
    );
}

#[test]
fn cancel_registered_effect_returns_registered_signal() {
    let signal = crate::CancellationSignal::new();
    let mut map = BTreeMap::new();
    map.insert(id(7), signal.clone());
    let active = Mutex::new(map);
    let found = cancel_registered_effect(&active, id(7), "model_effect_registry_unavailable")
        .expect("lock")
        .expect("registered");
    found.cancel();
    assert!(signal.is_cancelled());
}

#[test]
fn installed_chain_is_retrievable() {
    let mut coordinator = CommitCoordinator::new(Arc::new(FakeStore::new(FakeMode::Normal)));
    assert!(coordinator.middleware_chain().is_none());
    let chain = Arc::new(ResolvedMiddlewareChain::try_new(Vec::new()).expect("empty chain"));
    coordinator.install_middleware_chain(Arc::clone(&chain));
    assert!(coordinator.middleware_chain().is_some());
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[test]
fn stage_dispatch_seed_matches_pending_model_seed_security_fields() {
    let mut coordinator = CommitCoordinator::new(Arc::new(FakeStore::new(FakeMode::Normal)));
    block_on(drive_to_model_request(&mut coordinator));

    let model_seed = coordinator
        .pending_model_seed()
        .expect("pending model seed");
    let stage_seed = coordinator
        .stage_dispatch_seed()
        .expect("stage dispatch seed");

    assert_eq!(stage_seed.locator, model_seed.locator);
    assert_eq!(stage_seed.authorization, model_seed.authorization);
    assert_eq!(stage_seed.budget_scope_id, model_seed.budget_scope_id);
    assert_eq!(
        stage_seed.attempt, model_seed.attempt,
        "both derive attempt as state.retry.attempts + 1"
    );
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[test]
fn stage_dispatch_seed_is_none_before_a_run_is_accepted() {
    let coordinator = CommitCoordinator::new(Arc::new(FakeStore::new(FakeMode::Normal)));
    assert!(coordinator.stage_dispatch_seed().is_none());
}
