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
    assert!(main
        .submit(
            env(1_200, 30, 31, 32),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id(2),
                accepted: root_acceptance(30),
            }),
        )
        .await
        .is_err());
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
