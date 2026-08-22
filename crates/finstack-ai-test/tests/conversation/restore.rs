#[tokio::test]
async fn drop_snapshot_leaves_the_conversation_tree() {
    let store = store();
    let mut coordinator =
        CommitCoordinator::new(store.clone()).with_snapshot_schedule(SnapshotSchedule {
            every_n_records: 1,
            write_timeout: Duration::from_millis(50),
        });
    bootstrap(&mut coordinator, 1, 2, 10).await;
    accept_parent(&mut coordinator).await;
    let before = coordinator.session().clone();
    store.discard_snapshot(id(1)).expect("discard");
    let loaded = store
        .load(LoadRequest { session_id: id(1) })
        .await
        .expect("load");
    assert!(loaded.snapshot.is_none());
    assert!(loaded.accelerated.is_none());
    let recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover");
    assert_eq!(recovered.session().entries(), before.entries());
    assert_eq!(
        recovered.session().main_lane().expect("main").1.leaf_id,
        before.main_lane().expect("main").1.leaf_id
    );
    assert_eq!(
        recovered.session().operations().keys().collect::<Vec<_>>(),
        before.operations().keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn main_lane_restores_leaf_and_rejects_a_second_root() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store.clone());
    bootstrap(&mut coordinator, 1, 2, 10).await;
    accept_parent(&mut coordinator).await;
    let leaf = coordinator
        .session()
        .main_lane()
        .expect("main")
        .1
        .leaf_id
        .expect("leaf");
    let recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover");
    assert_eq!(
        recovered.session().main_lane().expect("main").1.leaf_id,
        Some(leaf)
    );
    assert_eq!(
        recovered.state().accepted().expect("run").run_id(),
        id(3)
    );
    assert_eq!(recovered.state().phase(), Some(RunPhase::BeforeRun));
    assert_eq!(recovered.session().active_on_lane(id(2)), Some(id(3)));
    let mut recovered = recovered;
    assert!(matches!(
        recovered
            .submit(
                env(2_000, 20, 21, 22),
                KernelInput::AcceptRun(AcceptRun {
                    session_id: id::<SessionTag>(1),
                    lane_id: id::<LaneTag>(2),
                    accepted: acceptance(30),
                }),
            )
            .await,
        Err(CommitCoordinatorError::Decision { .. })
    ));
}

#[tokio::test]
async fn pre046_journals_without_session_created_still_recover() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store.clone());
    accept_parent(&mut coordinator).await;
    let recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover");
    assert!(recovered.session().main_lane().is_none());
    assert_eq!(
        recovered.state().accepted().expect("run").run_id(),
        id(3)
    );
}
