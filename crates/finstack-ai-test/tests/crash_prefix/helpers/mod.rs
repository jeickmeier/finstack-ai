//! PR-048 crash-prefix matrix: drop the owner, recover, assert a legal class.
#![allow(
    clippy::too_many_lines,
    reason = "each prefix cluster is one legal-class matrix"
)]
#![allow(
    clippy::large_futures,
    reason = "coordinator fixtures are large; boxing would hide the matrix"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetChargeReceipt, BudgetChargeRequest, BudgetPropagation,
    BudgetReleaseReceipt, BudgetReleaseRequest, BudgetRequest, BudgetReservationReceipt,
    BudgetReserveRequest, CancellationPropagation, ChildPlacement, ChildRunLocator, ComponentId,
    ComponentInvocation, ComponentRef, ContentBlock, DeadlinePropagation, Digest, EffectCompleted,
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested, Id, IdTag,
    InteractionKind, InteractionRequest, InteractionTag, InvocationRecovery, KernelInput, LaneTag,
    Message, MessageRole, Metadata, ModelSettled, ModelSettlement, OperationLocator,
    PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, RecordTag, ReducerStageOutcome,
    RetrySafety, RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation,
    RunRelationKind, RunSecurityContext, SessionTag, Stage, StageCursor, TextBlock, Timestamp,
    TransitionEnv, Usage, Version,
};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, AgentRef, AuthorizationContext, BudgetError, BudgetLedger,
    BudgetOperationIds, BudgetReservationState, ChildCoordinationIds, ChildRunContext,
    ChildRunHandle, ChildRunRequest, CommitCoordinator, IdempotencyHorizon, JournalStore,
    LoadRequest, PortFuture, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink, StateSnapshotRequest,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_test::{LegalRestore, classify_phase};

pub(crate) const PREFIX_IDS: &[&str] = &[
    "W1", "W2", "W3", "W4", "W5", "W6", "D1", "D2", "D3", "D4", "D5", "D6", "I1", "I2", "I3", "I4",
    "C1", "C2", "C3", "C4", "C5", "F1", "F2", "F3", "L1", "L2", "L3", "B1", "B2", "B3", "B4", "N1",
    "N2", "N3", "N4", "A1", "A2", "T1", "R1", "X1", "M1", "K1", "P1", "P2", "P3", "P4",
];

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

pub(crate) fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 256,
            records_per_session: 1024,
            snapshot_bytes: 256 * 1024,
        })
        .expect("store"),
    )
}

pub(crate) fn unique_sqlite_path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pr048-crash-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("journal.sqlite")
}

pub(crate) fn sqlite_store(path: PathBuf) -> Arc<SqliteJournalStore> {
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path,
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 8,
                batches_per_session: 256,
                records_per_session: 1024,
                snapshot_bytes: 256 * 1024,
            },
            busy_timeout: Duration::from_secs(2),
        })
        .expect("sqlite"),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the coordinator TransitionEnv fixture"
)]
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

pub(crate) fn simple_env(now: i64, record: u64, event: u64, batch: u64) -> TransitionEnv {
    env(now, &[record], &[event], &[], &[], &[], &[], batch)
}

#[allow(
    clippy::too_many_arguments,
    reason = "tool-batch decisions need the extra ID bags"
)]
pub(crate) fn env_tools(
    now: i64,
    records: &[u64],
    events: &[u64],
    effects: &[u64],
    messages: &[u64],
    tool_batches: &[u64],
    tool_calls: &[u64],
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
            Vec::new(),
            Vec::new(),
            tool_batches.iter().copied().map(id).collect(),
            tool_calls.iter().copied().map(id).collect(),
            vec![id(append_batch)],
            Vec::new(),
        )
        .expect("tool ids"),
    }
}

pub(crate) fn security() -> RunSecurityContext {
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
}

pub(crate) fn propagation() -> RunPropagationPolicy {
    RunPropagationPolicy {
        cancellation: CancellationPropagation::Cascade,
        deadline: DeadlinePropagation::MinimumOfParentAndChild,
        budget: BudgetPropagation::SharedScope,
        principal: PrincipalPropagation::Inherit,
    }
}

pub(crate) fn acceptance(run: u64) -> RunAccepted {
    let run_id = id(run);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        security(),
        None,
        RunLimits::empty(),
        propagation(),
        Digest::raw_json(br#"{"agent":"crash-prefix"}"#),
        None,
    )
    .expect("accepted")
}

pub(crate) fn accept_input() -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: acceptance(3),
    })
}

pub(crate) fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

pub(crate) fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

pub(crate) fn context_message() -> Message {
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

pub(crate) fn assistant_message(ordinal: u64, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        MessageRole::Assistant,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant")
}

pub(crate) async fn accept_run(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(store);
    coordinator
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )
        .await
        .expect("accept");
    coordinator
}

pub(crate) async fn drive_to_model_request(coordinator: &mut CommitCoordinator) {
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
        .expect("model request");
}

pub(crate) async fn settle_model_completed(coordinator: &mut CommitCoordinator) {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending")
        .clone();
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        output_contract(),
        RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("cmpl-1"),
        None,
    )
    .expect("completed");
    coordinator
        .submit(
            env(1_400, &[607, 608], &[603, 604], &[], &[], &[], &[617], 605),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message: assistant_message(617, "hello"),
                },
            }),
        )
        .await
        .expect("settle");
}

pub(crate) async fn recover(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover")
}

pub(crate) async fn drive_to_pending_model(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    let mut coordinator = accept_run(Arc::clone(&store)).await;
    drive_to_model_request(&mut coordinator).await;
    drop(coordinator);
    recover(store).await
}

pub(crate) async fn settle_and_recover(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    let mut coordinator = drive_to_pending_model(Arc::clone(&store)).await;
    settle_model_completed(&mut coordinator).await;
    drop(coordinator);
    recover(store).await
}

pub(crate) fn assert_legal(prefix: &str, phase: Option<RunPhase>, expected: LegalRestore) {
    let class = classify_phase(phase.expect("phase"));
    assert_eq!(class, expected, "{prefix} restored to {class:?}");
}

pub(crate) async fn write_snapshot(store: &Arc<dyn JournalStore>, coordinator: &CommitCoordinator) {
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    store
        .write_state_snapshot(StateSnapshotRequest {
            session_id: id::<SessionTag>(1),
            state: coordinator.state().clone(),
            head_checksum: loaded.head_checksum.expect("head"),
            pending_timer_scheduled_at: None,
            last_model_continuation: None,
        })
        .await
        .expect("snapshot");
}

pub(crate) fn horizon() -> IdempotencyHorizon {
    IdempotencyHorizon {
        expire_at: timestamp(10_000),
    }
}

#[derive(Default)]
pub(crate) struct RecordingSink {
    pub(crate) events: Mutex<Vec<SecurityAuditEvent>>,
}

impl SecurityAuditSink for RecordingSink {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
        let event_id = Arc::<str>::from(event.event_id());
        let recorded_at = event.timestamp();
        self.events.lock().expect("lock").push(event);
        Box::pin(async move {
            Ok(SecurityAuditReceipt {
                event_id,
                recorded_at,
            })
        })
    }

    fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
        Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
    }
}

pub(crate) struct RecordingInvoker;

impl AgentInvoker for RecordingInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let relation_digest =
            finstack_ai_runtime::child_relation_digest(&context, &request).expect("digest");
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

pub(crate) struct TestLedger {
    reserve: BudgetReservationReceipt,
    charge: BudgetChargeReceipt,
    release: BudgetReleaseReceipt,
    reserved: Mutex<bool>,
    reserve_calls: Mutex<usize>,
    charge_calls: Mutex<usize>,
    release_calls: Mutex<usize>,
}

impl TestLedger {
    pub(crate) fn new() -> Self {
        let reserve = reserve_request();
        let usage = charge_usage();
        Self {
            reserve: BudgetReservationReceipt {
                scope_id: reserve.scope_id,
                reservation_id: reserve.reservation_id,
                reserved: reserve.amount.clone(),
                remaining: BudgetRequest::default(),
                request_digest: reserve.request_digest,
                receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
            },
            charge: BudgetChargeReceipt {
                scope_id: reserve.scope_id,
                reservation_id: reserve.reservation_id,
                effect_id: id(103),
                charged_usage: usage.clone(),
                cumulative_usage: usage.clone(),
                usage_digest: Digest::effect_output(&usage.canonical_bytes().expect("bytes")),
                receipt_digest: Digest::raw_json(br#"{"receipt":"charge"}"#),
            },
            release: BudgetReleaseReceipt {
                scope_id: reserve.scope_id,
                reservation_id: reserve.reservation_id,
                terminal_run_id: id(3),
                released_unused: BudgetRequest::default(),
                request_digest: BudgetReleaseRequest::compute_digest(
                    reserve.scope_id,
                    reserve.reservation_id,
                    id(3),
                )
                .expect("release digest"),
                receipt_digest: Digest::raw_json(br#"{"receipt":"release"}"#),
            },
            reserved: Mutex::new(false),
            reserve_calls: Mutex::new(0),
            charge_calls: Mutex::new(0),
            release_calls: Mutex::new(0),
        }
    }

    pub(crate) fn reserve_calls(&self) -> usize {
        *self.reserve_calls.lock().expect("calls")
    }

    pub(crate) fn charge_calls(&self) -> usize {
        *self.charge_calls.lock().expect("calls")
    }

    pub(crate) fn release_calls(&self) -> usize {
        *self.release_calls.lock().expect("calls")
    }
}

impl BudgetLedger for TestLedger {
    fn reserve(
        &self,
        request: BudgetReserveRequest,
    ) -> PortFuture<Result<BudgetReservationReceipt, BudgetError>> {
        *self.reserve_calls.lock().expect("calls") += 1;
        *self.reserved.lock().expect("reserved") = true;
        assert_eq!(request.request_digest, self.reserve.request_digest);
        let receipt = self.reserve.clone();
        Box::pin(async move { Ok(receipt) })
    }

    fn reconcile(
        &self,
        scope_id: finstack_ai_kernel::BudgetScopeId,
        reservation_id: finstack_ai_kernel::BudgetReservationId,
    ) -> PortFuture<Result<BudgetReservationState, BudgetError>> {
        let reserved = *self.reserved.lock().expect("reserved");
        let receipt = self.reserve.clone();
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
        request: BudgetChargeRequest,
    ) -> PortFuture<Result<BudgetChargeReceipt, BudgetError>> {
        *self.charge_calls.lock().expect("calls") += 1;
        assert_eq!(request.effect_id, self.charge.effect_id);
        let receipt = self.charge.clone();
        Box::pin(async move { Ok(receipt) })
    }

    fn release(
        &self,
        request: BudgetReleaseRequest,
    ) -> PortFuture<Result<BudgetReleaseReceipt, BudgetError>> {
        *self.release_calls.lock().expect("calls") += 1;
        assert_eq!(request.terminal_run_id, self.release.terminal_run_id);
        let receipt = self.release.clone();
        Box::pin(async move { Ok(receipt) })
    }
}

pub(crate) fn request_env(now: i64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id::<RecordTag>(80), id::<RecordTag>(81)],
            vec![id(80), id(81)],
            vec![id(502)],
            vec![id::<InteractionTag>(501)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(180)],
            Vec::new(),
        )
        .expect("request ids"),
    }
}

pub(crate) fn resolve_env(now: i64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id::<RecordTag>(82), id::<RecordTag>(83)],
            vec![id(82), id(83)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(181)],
            Vec::new(),
        )
        .expect("resolve ids"),
    }
}

pub(crate) fn cancel_env(now: i64, record: u64, append_batch: u64, request: u64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id(record)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(append_batch)],
            vec![id(request)],
        )
        .expect("cancel ids"),
    }
}

pub(crate) fn typed_request(kind: InteractionKind) -> InteractionRequest {
    InteractionRequest::try_new(
        1,
        id::<InteractionTag>(501),
        id(502),
        kind,
        vec![ContentBlock::Text(
            TextBlock::try_new("resolve the outstanding interaction").expect("prompt"),
        )],
        RawJson::parse(
            r#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
        )
        .expect("schema"),
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").expect("component"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        None,
        false,
        Metadata::empty(),
    )
    .expect("request")
}

pub(crate) fn parent_locator() -> OperationLocator {
    OperationLocator {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
    }
}

pub(crate) fn parent_context() -> ChildRunContext {
    parent_context_effect(44)
}

pub(crate) fn parent_context_effect(effect: u64) -> ChildRunContext {
    ChildRunContext {
        parent: parent_locator(),
        parent_effect_id: id(effect),
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
    }
}

pub(crate) fn child_request(
    placement: ChildPlacement,
    session: u64,
    lane: u64,
    run: u64,
) -> ChildRunRequest {
    ChildRunRequest {
        agent: AgentRef {
            id: finstack_ai_kernel::AgentId::parse("finstack.agent.child").expect("agent"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
        },
        input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("work").expect("text"),
        )]),
        placement,
        locator: ChildRunLocator {
            operation: OperationLocator {
                tenant_scope: Arc::from("tenant-a"),
                session_id: id(session),
                lane_id: id(lane),
                run_id: id(run),
            },
            remote: None,
        },
        requested_deadline: None,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    }
}

pub(crate) fn budget_child_request() -> ChildRunRequest {
    let mut request = child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51);
    request.requested_budget = reserve_request().amount;
    request.request_digest = Digest::raw_json(br#"{"request":"budget-child"}"#);
    request
}

pub(crate) fn coordination_ids(batch: u64, record: u64) -> ChildCoordinationIds {
    ChildCoordinationIds {
        preparation_batch_id: id(batch),
        preparation_record_id: id(record),
        reservation_request_record_id: None,
        reservation_settlement: None,
    }
}

pub(crate) fn budget_ids() -> ChildCoordinationIds {
    ChildCoordinationIds {
        preparation_batch_id: id(401),
        preparation_record_id: id(402),
        reservation_request_record_id: Some(id(403)),
        reservation_settlement: Some(BudgetOperationIds {
            batch_id: id(404),
            record_id: id(405),
        }),
    }
}

pub(crate) fn child_accepted(run: u64, parent_effect: u64, parent: &RunAccepted) -> RunAccepted {
    RunAccepted::try_new(
        id(run),
        RunRelation::try_new(
            id(3),
            Some(id(3)),
            Some(id(parent_effect)),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("relation"),
        security(),
        None,
        RunLimits::empty(),
        propagation(),
        Digest::raw_json(br#"{"agent":"child"}"#),
        Some(parent),
    )
    .expect("child accepted")
}

pub(crate) fn reserve_request() -> BudgetReserveRequest {
    let amount = BudgetRequest {
        input_tokens: Some(1_000),
        output_tokens: Some(250),
        cost: None,
        extension_counters: BTreeMap::new(),
    };
    let request_digest =
        BudgetReserveRequest::compute_digest(id(342), id(343), id(51), &amount).expect("digest");
    BudgetReserveRequest {
        scope_id: id(342),
        reservation_id: id(343),
        run_id: id(51),
        amount,
        request_digest,
    }
}

pub(crate) fn charge_usage() -> Usage {
    Usage::try_new(Some(20), Some(10), Some(30), None, BTreeMap::new()).expect("usage")
}

pub(crate) fn charge_request() -> BudgetChargeRequest {
    let usage = charge_usage();
    BudgetChargeRequest {
        scope_id: id(342),
        reservation_id: id(343),
        effect_id: id(103),
        usage: usage.clone(),
        usage_digest: Digest::effect_output(&usage.canonical_bytes().expect("bytes")),
    }
}

pub(crate) fn release_request() -> BudgetReleaseRequest {
    BudgetReleaseRequest {
        scope_id: id(342),
        reservation_id: id(343),
        terminal_run_id: id(3),
        request_digest: BudgetReleaseRequest::compute_digest(id(342), id(343), id(3))
            .expect("digest"),
    }
}

pub(crate) fn middleware_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::MiddlewareOutcome,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"middleware"}"#),
    }
}

pub(crate) fn middleware_request(recovery: InvocationRecovery) -> EffectRequested {
    EffectRequested::try_new(
        id(10),
        EffectKind::Middleware,
        None,
        Some(ComponentInvocation {
            component: ComponentId::parse("finstack.middleware.compact").expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery,
        }),
        None,
        middleware_contract(),
        EffectInput::Middleware {
            stage: Arc::from("before_model"),
            input: RawJson::parse("{}").expect("input"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("requested")
}
