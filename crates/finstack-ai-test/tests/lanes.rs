//! PR-047 multi-lane session APIs, concurrency, and lineage fan-out.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetPropagation, CancellationInitiator, CancellationPropagation,
    ChildPlacement, ChildRunLocator, ContentBlock, DeadlinePropagation, Digest, Id, IdTag,
    KernelInput, LaneCreated, LaneMoved, LaneTag, Message, MessageRole, Metadata, OperationLocator,
    PrincipalPropagation, PrincipalRef, ProviderIds, RunAccepted, RunLimits, RunPhase,
    RunPropagationPolicy, RunRelation, RunRelationKind, RunSecurityContext, SessionTag, TextBlock,
    Timestamp, TransitionEnv,
};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, AuthorizationContext, ChildCoordinationIds, ChildRunContext,
    ChildRunCoordinator, ChildRunHandle, ChildRunRequest, ExternalIdentityKey, ExternalIdentityMap,
    JournalStore, LaneAppendIds, LaneCreateIds, MemoryExternalIdentityMap, PortFuture,
    SessionCreateIds, SessionError, SessionRuntime,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_test::{LegalRestore, classify_phase};
use proptest::test_runner::{Config as ProptestConfig, RngSeed};

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

fn memory_store() -> Arc<dyn JournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 256,
            records_per_session: 1024,
            snapshot_bytes: 64 * 1024,
        })
        .expect("store"),
    )
}

fn create_ids(session: u64, lane: u64, batch: u64) -> SessionCreateIds {
    SessionCreateIds {
        session_id: id(session),
        main_lane_id: id(lane),
        session_created_record_id: id(batch + 1),
        lane_created_record_id: id(batch + 2),
        batch_id: id(batch),
        now: timestamp(1),
    }
}

fn lane_ids(lane: u64, batch: u64, fork: bool) -> LaneCreateIds {
    LaneCreateIds {
        lane_id: id(lane),
        lane_created_record_id: id(batch + 1),
        lane_moved_record_id: fork.then_some(id(batch + 2)),
        batch_id: id(batch),
        now: timestamp(2),
    }
}

fn append_ids(batch: u64) -> LaneAppendIds {
    LaneAppendIds {
        entry_record_id: id(batch + 1),
        lane_moved_record_id: id(batch + 2),
        batch_id: id(batch),
    }
}

fn text_message(ordinal: u64, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        MessageRole::User,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(0),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn cancel_env(now: i64, record: u64, batch: u64, request: u64) -> TransitionEnv {
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
            vec![id(batch)],
            vec![id(request)],
        )
        .expect("cancel ids"),
    }
}

fn env(now: i64, record: u64, event: u64, batch: u64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id(record)],
            vec![id(event)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(batch)],
            Vec::new(),
        )
        .expect("ids"),
    }
}

fn security(decision: &str) -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
        "oidc",
        "high",
        "policy-v1",
        decision,
        None,
    )
    .expect("security")
}

fn propagation(cancellation: CancellationPropagation) -> RunPropagationPolicy {
    RunPropagationPolicy {
        cancellation,
        deadline: DeadlinePropagation::MinimumOfParentAndChild,
        budget: BudgetPropagation::SharedScope,
        principal: PrincipalPropagation::Inherit,
    }
}

fn root_acceptance(run: u64) -> RunAccepted {
    let run_id = id(run);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        security("decision-v1"),
        None,
        RunLimits::empty(),
        propagation(CancellationPropagation::Cascade),
        Digest::raw_json(br#"{"agent":"lanes"}"#),
        None,
    )
    .expect("acceptance")
}

fn child_acceptance(
    run: u64,
    parent: &RunAccepted,
    parent_effect: u64,
    cancellation: CancellationPropagation,
    decision: &str,
) -> RunAccepted {
    RunAccepted::try_new(
        id(run),
        RunRelation::try_new(
            parent.run_id(),
            Some(parent.run_id()),
            Some(id(parent_effect)),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("relation"),
        security(decision),
        None,
        RunLimits::empty(),
        propagation(cancellation),
        Digest::raw_json(br#"{"agent":"child"}"#),
        Some(parent),
    )
    .expect("child accepted")
}

async fn open_session(store: Arc<dyn JournalStore>) -> Arc<SessionRuntime> {
    SessionRuntime::create(store, "tenant-a", create_ids(1, 2, 100))
        .await
        .expect("create")
}

struct RecordingInvoker;

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

fn parent_context() -> ChildRunContext {
    ChildRunContext {
        parent: OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(2),
            run_id: id(3),
        },
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
    }
}

fn child_request(placement: ChildPlacement, session: u64, lane: u64, run: u64) -> ChildRunRequest {
    ChildRunRequest {
        agent: finstack_ai_runtime::AgentRef {
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
        requested_budget: finstack_ai_kernel::BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    }
}

#[tokio::test]
async fn shared_prefix_fork_diverges_without_copying_entries() {
    let session = open_session(memory_store()).await;
    let a = session
        .append_message(id(2), &text_message(10, "A"), append_ids(110))
        .await
        .expect("A");
    let b = session
        .append_message(id(2), &text_message(11, "B"), append_ids(120))
        .await
        .expect("B");
    let c = session
        .append_message(id(2), &text_message(12, "C"), append_ids(130))
        .await
        .expect("C");
    session
        .create_lane("research", Some(b), lane_ids(50, 140, true))
        .await
        .expect("fork");
    let main = session.inspect(id(2)).await.expect("main");
    let research = session.inspect(id(50)).await.expect("research");
    assert_eq!(
        main.history
            .iter()
            .map(finstack_ai_kernel::ConversationEntry::id)
            .collect::<Vec<_>>(),
        vec![a, b, c]
    );
    assert_eq!(
        research
            .history
            .iter()
            .map(finstack_ai_kernel::ConversationEntry::id)
            .collect::<Vec<_>>(),
        vec![a, b]
    );
    let shared = session
        .projection()
        .expect("projection")
        .entries()
        .get(&b)
        .expect("B")
        .clone();
    let d = session
        .append_message(id(50), &text_message(13, "D"), append_ids(150))
        .await
        .expect("D");
    let after = session.projection().expect("after");
    assert_eq!(after.entries().get(&b).expect("B kept"), &shared);
    assert_eq!(after.lane_by_id(id(2)).expect("main").leaf_id, Some(c));
    assert_eq!(after.lane_by_id(id(50)).expect("research").leaf_id, Some(d));
    assert_eq!(after.entries().len(), 4);
}

#[tokio::test]
async fn busy_lane_rejects_a_second_owner_while_the_sibling_continues() {
    let session = open_session(memory_store()).await;
    session
        .create_lane("research", None, lane_ids(50, 200, false))
        .await
        .expect("lane");
    session.try_acquire_run(id(2), id(3)).expect("acquire main");
    let mut main = session
        .accept_run(id(2), root_acceptance(3), env(1_000, 10, 11, 12))
        .await
        .expect("accept main");
    assert_eq!(main.state().phase, Some(RunPhase::BeforeRun));
    session
        .try_acquire_run(id(50), id(4))
        .expect("acquire sibling");
    let sibling = session
        .accept_run(id(50), root_acceptance(4), env(1_100, 20, 21, 22))
        .await
        .expect("accept sibling");
    assert_eq!(sibling.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(
        session.try_acquire_run(id(2), id(30)),
        Err(SessionError::LaneBusy)
    );
    assert!(
        main.submit(
            env(1_200, 30, 31, 32),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id(2),
                accepted: root_acceptance(30),
            }),
        )
        .await
        .is_err()
    );
    assert_eq!(sibling.state().phase, Some(RunPhase::BeforeRun));
    assert!(sibling.state().cancellation.is_none());
    assert_eq!(
        session
            .projection()
            .expect("projection")
            .active_on_lane(id(50)),
        Some(id(4))
    );
}

#[tokio::test]
async fn interleaved_lane_records_restore_from_the_session_head() {
    let store = memory_store();
    let session = open_session(Arc::clone(&store)).await;
    session
        .create_lane("research", None, lane_ids(50, 300, false))
        .await
        .expect("lane");
    let main_a = session
        .append_message(id(2), &text_message(10, "main-a"), append_ids(310))
        .await
        .expect("main-a");
    let research_a = session
        .append_message(id(50), &text_message(11, "research-a"), append_ids(320))
        .await
        .expect("research-a");
    let main_b = session
        .append_message(id(2), &text_message(12, "main-b"), append_ids(330))
        .await
        .expect("main-b");
    let before = session.projection().expect("before");
    drop(session);
    let restored = SessionRuntime::open(store, id(1), "tenant-a")
        .await
        .expect("open");
    let after = restored.projection().expect("after");
    assert_eq!(after.entries(), before.entries());
    assert_eq!(after.lane_by_id(id(2)).expect("main").leaf_id, Some(main_b));
    assert_eq!(
        after.lane_by_id(id(50)).expect("research").leaf_id,
        Some(research_a)
    );
    let coordinator = restored
        .coordinator_for_run(None)
        .await
        .expect("structural");
    assert_eq!(
        coordinator.state().last_applied_sequence,
        restored
            .coordinator_for_run(None)
            .await
            .expect("again")
            .state()
            .last_applied_sequence
    );
    assert!(coordinator.state().last_applied_sequence >= 5);
    let _ = main_a;
}

#[tokio::test]
async fn sqlite_parallel_lane_appenders_preserve_single_writer_invariants() {
    let path = unique_sqlite_path();
    let store: Arc<dyn JournalStore> = Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path,
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 4,
                batches_per_session: 256,
                records_per_session: 1024,
                snapshot_bytes: 64 * 1024,
            },
            busy_timeout: Duration::from_secs(2),
        })
        .expect("sqlite"),
    );
    let session = open_session(Arc::clone(&store)).await;
    session
        .create_lane("research", None, lane_ids(50, 400, false))
        .await
        .expect("lane");
    let first = session.clone();
    let second = session.clone();
    let main_message = text_message(20, "main");
    let research_message = text_message(21, "research");
    let (left, right) = tokio::join!(
        first.append_message(id(2), &main_message, append_ids(410)),
        second.append_message(id(50), &research_message, append_ids(420)),
    );
    let left = left.expect("main append");
    let right = right.expect("research append");
    assert_ne!(left, right);
    session.try_acquire_run(id(2), id(3)).expect("first owner");
    assert_eq!(
        session.try_acquire_run(id(2), id(4)),
        Err(SessionError::LaneBusy)
    );
    let restored = SessionRuntime::open(store, id(1), "tenant-a")
        .await
        .expect("open");
    let projection = restored.projection().expect("projection");
    assert_eq!(
        projection.lane_by_id(id(2)).expect("main").leaf_id,
        Some(left)
    );
    assert_eq!(
        projection.lane_by_id(id(50)).expect("research").leaf_id,
        Some(right)
    );
    let coordinator = restored.coordinator_for_run(None).await.expect("head");
    assert!(coordinator.state().last_applied_sequence >= 4);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "A05 interleaves compatible, isolated, and detach children in one journal"
)]
async fn child_run_and_lane_stay_distinct_and_cancel_fans_out() {
    let store = memory_store();
    let session = open_session(Arc::clone(&store)).await;
    session
        .create_lane("research", None, lane_ids(50, 500, false))
        .await
        .expect("lane");
    session.try_acquire_run(id(2), id(3)).expect("parent guard");
    let mut parent = session
        .accept_run(id(2), root_acceptance(3), env(2_000, 40, 41, 42))
        .await
        .expect("parent");
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker));
    children
        .start_or_attach(
            &mut parent,
            parent_context(),
            child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51),
            None,
            ChildCoordinationIds {
                preparation_batch_id: id(510),
                preparation_record_id: id(511),
                reservation_request_record_id: None,
                reservation_settlement: None,
            },
            timestamp(2_100),
        )
        .await
        .expect("compatible");
    let parent_accepted = parent.state().accepted.clone().expect("parent accepted");
    session.try_acquire_run(id(50), id(51)).expect("child lane");
    let compatible = session
        .accept_run(
            id(50),
            child_acceptance(
                51,
                &parent_accepted,
                44,
                CancellationPropagation::Cascade,
                "decision-v1",
            ),
            env(2_200, 52, 53, 54),
        )
        .await
        .expect("accept compatible");
    assert_ne!(
        compatible
            .state()
            .accepted
            .as_ref()
            .expect("child")
            .run_id(),
        id(3)
    );
    assert_eq!(
        session
            .projection()
            .expect("map")
            .child_mapping(id(3), id(44))
            .expect("mapping")
            .child
            .operation
            .lane_id,
        id(50)
    );

    let isolated_session =
        SessionRuntime::create(Arc::clone(&store), "tenant-a", create_ids(80, 81, 600))
            .await
            .expect("isolated session");
    children
        .start_or_attach(
            &mut parent,
            ChildRunContext {
                parent_effect_id: id(45),
                ..parent_context()
            },
            child_request(ChildPlacement::IsolatedChildSession, 80, 81, 82),
            None,
            ChildCoordinationIds {
                preparation_batch_id: id(610),
                preparation_record_id: id(611),
                reservation_request_record_id: None,
                reservation_settlement: None,
            },
            timestamp(2_300),
        )
        .await
        .expect("isolated map");
    isolated_session
        .try_acquire_run(id(81), id(82))
        .expect("isolated guard");
    isolated_session
        .accept_run(
            id(81),
            child_acceptance(
                82,
                &parent_accepted,
                45,
                CancellationPropagation::Cascade,
                "decision-v1",
            ),
            env(2_400, 83, 84, 85),
        )
        .await
        .expect("accept isolated");

    let detach_session =
        SessionRuntime::create(Arc::clone(&store), "tenant-a", create_ids(90, 91, 700))
            .await
            .expect("detach session");
    children
        .start_or_attach(
            &mut parent,
            ChildRunContext {
                parent_effect_id: id(46),
                ..parent_context()
            },
            child_request(ChildPlacement::IsolatedChildSession, 90, 91, 92),
            None,
            ChildCoordinationIds {
                preparation_batch_id: id(710),
                preparation_record_id: id(711),
                reservation_request_record_id: None,
                reservation_settlement: None,
            },
            timestamp(2_500),
        )
        .await
        .expect("detach map");
    detach_session
        .try_acquire_run(id(91), id(92))
        .expect("detach guard");
    detach_session
        .accept_run(
            id(91),
            child_acceptance(
                92,
                &parent_accepted,
                46,
                CancellationPropagation::DetachOnlyIfPreauthorized,
                "detach:preauthorized",
            ),
            env(2_600, 93, 94, 95),
        )
        .await
        .expect("accept detach");

    let mut n = 3_000_u64;
    let mut next_env = || {
        n += 10;
        Ok(cancel_env(n.cast_signed(), n, n + 1, n + 2))
    };
    session
        .cancel_run(id(3), CancellationInitiator::RuntimeShutdown, &mut next_env)
        .await
        .expect("fan-out");

    let compatible_again = SessionRuntime::existing(&store, id(1))
        .expect("intern table")
        .expect("parent intern")
        .coordinator_for_run(Some(id(51)))
        .await
        .expect("compatible recover");
    let isolated_again = isolated_session
        .coordinator_for_run(Some(id(82)))
        .await
        .expect("isolated recover");
    let detach_again = detach_session
        .coordinator_for_run(Some(id(92)))
        .await
        .expect("detach recover");
    assert!(
        compatible_again.state().terminal.is_some()
            || compatible_again.state().cancellation.is_some()
    );
    assert!(
        isolated_again.state().terminal.is_some() || isolated_again.state().cancellation.is_some()
    );
    assert!(detach_again.state().terminal.is_none());
    assert!(detach_again.state().cancellation.is_none());
}

#[test]
fn identity_map_equal_bind_is_idempotent() {
    let map = MemoryExternalIdentityMap::new();
    let key = ExternalIdentityKey::try_new("slack", "acct", "t1").expect("key");
    map.bind(key.clone(), id(1), id(2)).expect("bind");
    map.bind(key.clone(), id(1), id(2)).expect("equal");
    assert_eq!(map.resolve(&key), Some((id(1), id(2))));
}

const PROPERTY_CASES: u32 = 256;
const PROPERTY_SEED: u64 = 0x5eed_0130_0000_0001;

fn lane_property_config() -> ProptestConfig {
    ProptestConfig {
        cases: PROPERTY_CASES,
        rng_seed: RngSeed::Fixed(PROPERTY_SEED),
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::SourceParallel("proptest-regressions"),
        )),
        max_shrink_iters: 10_000,
        ..ProptestConfig::default()
    }
}

#[test]
fn lane_created_and_moved_fail_closed_on_unknown_state_bearing_fields() {
    assert!(
        serde_json::from_str::<LaneCreated>(r#"{"name":"research","owner":"evil"}"#).is_err(),
        "LaneCreated must deny unknown fields"
    );
    assert!(
        serde_json::from_str::<LaneMoved>(
            r#"{"leaf_id":"01234567-89ab-7cde-89ab-0123456789ab","forked_from":"x"}"#
        )
        .is_err(),
        "LaneMoved must deny unknown fields"
    );
}

#[test]
fn lane_invariants_hold_for_fixed_seed_sibling_sequences() {
    let mut runner = proptest::test_runner::TestRunner::new(lane_property_config());
    runner
        .run(&(1_usize..=3), |siblings| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let store = memory_store();
                let session = open_session(Arc::clone(&store)).await;
                let mut lane_ids_created = vec![id::<LaneTag>(2)];
                for offset in 0..siblings {
                    let lane = 50 + u64::try_from(offset).expect("lane");
                    session
                        .create_lane(
                            format!("lane-{offset}"),
                            None,
                            lane_ids(lane, 200 + lane * 10, false),
                        )
                        .await
                        .expect("create");
                    lane_ids_created.push(id(lane));
                }
                let mut owners = Vec::new();
                for (index, lane) in lane_ids_created.iter().copied().enumerate() {
                    let run = 3 + u64::try_from(index).expect("run");
                    session.try_acquire_run(lane, id(run)).expect("one owner");
                    assert_eq!(
                        session.try_acquire_run(lane, id(run + 30)),
                        Err(SessionError::LaneBusy),
                        "busy lane rejects a second owner"
                    );
                    let coordinator = session
                        .accept_run(
                            lane,
                            root_acceptance(run),
                            env(
                                1_000 + i64::try_from(index).expect("now"),
                                10 + run,
                                11 + run,
                                12 + run,
                            ),
                        )
                        .await
                        .expect("accept");
                    assert_eq!(coordinator.state().phase, Some(RunPhase::BeforeRun));
                    owners.push(coordinator);
                }
                for owner in &owners {
                    assert!(owner.state().cancellation.is_none());
                }
                drop(owners);
                drop(session);
                let restored = SessionRuntime::open(store, id(1), "tenant-a")
                    .await
                    .expect("restore");
                for (index, lane) in lane_ids_created.iter().copied().enumerate() {
                    let run = 3 + u64::try_from(index).expect("run");
                    let coordinator = restored
                        .coordinator_for_run(Some(id(run)))
                        .await
                        .expect("restored run");
                    let class = classify_phase(coordinator.state().phase.expect("phase"));
                    assert_eq!(class, LegalRestore::Retryable);
                    let _ = lane;
                }
                Ok(())
            })
        })
        .expect("lane properties");
}

fn unique_sqlite_path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pr047-a04-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("journal.sqlite")
}
