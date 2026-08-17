#[tokio::test]
async fn prefix_n1_through_n4() {
    let store: Arc<dyn JournalStore> = memory_store();
    let session = SessionRuntime::create(
        Arc::clone(&store),
        "tenant-a",
        SessionCreateIds {
            session_id: id(1),
            main_lane_id: id(2),
            session_created_record_id: id(101),
            lane_created_record_id: id(102),
            batch_id: id(100),
            now: timestamp(1),
        },
    )
    .await
    .expect("create");
    session
        .create_lane(
            "research",
            None,
            LaneCreateIds {
                lane_id: id(50),
                lane_created_record_id: id(201),
                lane_moved_record_id: None,
                batch_id: id(200),
                now: timestamp(2),
            },
        )
        .await
        .expect("lane");
    let leaf = session
        .append_message(
            id(2),
            &Message::try_new(
                id(10),
                MessageRole::User,
                vec![ContentBlock::Text(TextBlock::try_new("A").expect("text"))],
                timestamp(0),
                None,
                ProviderIds::empty(),
                Metadata::empty(),
            )
            .expect("message"),
            LaneAppendIds {
                entry_record_id: id(211),
                lane_moved_record_id: id(212),
                batch_id: id(210),
            },
        )
        .await
        .expect("append");
    drop(session);
    let restored = SessionRuntime::open(Arc::clone(&store), id(1), "tenant-a")
        .await
        .expect("open");
    assert_eq!(
        restored.inspect(id(2)).await.expect("main").name.as_ref(),
        "main"
    );
    assert_eq!(
        restored
            .inspect(id(50))
            .await
            .expect("research")
            .name
            .as_ref(),
        "research"
    );
    assert_eq!(
        restored
            .inspect(id(2))
            .await
            .expect("main")
            .history
            .last()
            .map(finstack_ai_kernel::ConversationEntry::id),
        Some(leaf)
    );
    let _ = ("N1", "N2");

    restored
        .try_acquire_run(id(2), id(3))
        .expect("acquire main");
    let main = restored
        .accept_run(id(2), acceptance(3), simple_env(1_000, 10, 11, 12))
        .await
        .expect("accept main");
    restored
        .try_acquire_run(id(50), id(4))
        .expect("acquire sibling");
    let sibling = restored
        .accept_run(id(50), acceptance(4), simple_env(1_100, 20, 21, 22))
        .await
        .expect("accept sibling");
    assert_eq!(main.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(sibling.state().phase, Some(RunPhase::BeforeRun));
    drop(main);
    drop(sibling);
    drop(restored);
    let restored = SessionRuntime::open(Arc::clone(&store), id(1), "tenant-a")
        .await
        .expect("open siblings");
    let main = restored
        .coordinator_for_run(Some(id(3)))
        .await
        .expect("main run");
    let sibling = restored
        .coordinator_for_run(Some(id(4)))
        .await
        .expect("sibling run");
    assert_eq!(main.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(sibling.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(
        restored.try_acquire_run(id(2), id(30)),
        Err(SessionError::LaneBusy),
        "N3 busy-lane still holds"
    );

    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker));
    let mut parent = restored
        .coordinator_for_run(Some(id(3)))
        .await
        .expect("parent");
    children
        .start_or_attach(
            &mut parent,
            parent_context(),
            child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51),
            None,
            coordination_ids(510, 511),
            timestamp(2_100),
        )
        .await
        .expect("first child");
    let mut n = 3_000_u64;
    let mut next_env = || {
        n += 10;
        Ok(cancel_env(n.cast_signed(), n, n + 1, n + 2))
    };
    restored
        .cancel_run(id(3), CancellationInitiator::RuntimeShutdown, &mut next_env)
        .await
        .expect("first cancel");
    drop(parent);
    drop(restored);
    let restored = SessionRuntime::open(store, id(1), "tenant-a")
        .await
        .expect("open after cancel");
    let mut n = 4_000_u64;
    let mut next_env = || {
        n += 10;
        Ok(cancel_env(n.cast_signed(), n, n + 1, n + 2))
    };
    restored
        .cancel_run(id(3), CancellationInitiator::RuntimeShutdown, &mut next_env)
        .await
        .expect("N4 second cancel idempotent");
}
