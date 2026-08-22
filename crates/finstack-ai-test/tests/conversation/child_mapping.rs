#[tokio::test]
async fn compatible_lane_child_mapping_survives_recover_and_rejects_remap() {
    let store = store();
    let mut parent = CommitCoordinator::new(store.clone());
    bootstrap(&mut parent, 1, 2, 10).await;
    accept_parent(&mut parent).await;
    parent
        .commit_session_records(
            id(205),
            vec![session_draft(
                206,
                1,
                50,
                RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
            )],
        )
        .await
        .expect("research lane");
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker {
        starts: Mutex::new(0),
    }));
    let context = parent_context();
    let compatible = child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51);
    let ids = coordination_ids(201, 202);
    let first = children
        .start_or_attach(
            &mut parent,
            context.clone(),
            compatible.clone(),
            None,
            ids,
            timestamp(1_100),
        )
        .await
        .expect("prepare compatible");
    let accepted = child_accepted(51, 44, parent.state().accepted().expect("parent"));
    append_foreign_accept(&store, &parent, accepted, 50, 210, 211, 212).await;
    let mut recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover parent");
    assert_eq!(
        recovered
            .session()
            .child_mapping(id(3), id(44))
            .expect("mapping")
            .child
            .operation
            .run_id,
        id(51)
    );
    assert_eq!(
        recovered
            .session()
            .operations()
            .get(&id(51))
            .expect("child op")
            .relation
            .parent_effect_id(),
        Some(id(44))
    );
    let attached = children
        .start_or_attach(
            &mut recovered,
            context.clone(),
            compatible.clone(),
            None,
            ids,
            timestamp(1_100),
        )
        .await
        .expect("equal retry");
    assert_eq!(first, attached);
    let mut conflicting = compatible;
    conflicting.input = Arc::from([finstack_ai_kernel::ContentBlock::Text(
        finstack_ai_kernel::TextBlock::try_new("different work").expect("text"),
    )]);
    conflicting.request_digest = conflicting
        .canonical_digest()
        .expect("conflicting child request digest");
    assert!(matches!(
        children
            .start_or_attach(
                &mut recovered,
                context,
                conflicting,
                None,
                ids,
                timestamp(1_100)
            )
            .await,
        Err(CompositionError::Commit(
            CommitCoordinatorError::SidecarConflict
        ))
    ));
}

#[tokio::test]
async fn isolated_child_session_mapping_matches_recovered_relation() {
    let store = store();
    let mut parent = CommitCoordinator::new(store.clone());
    bootstrap(&mut parent, 1, 2, 10).await;
    accept_parent(&mut parent).await;
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker {
        starts: Mutex::new(0),
    }));
    let isolated = child_request(ChildPlacement::IsolatedChildSession, 80, 81, 82);
    children
        .start_or_attach(
            &mut parent,
            ChildRunContext {
                parent_effect_id: id(45),
                ..parent_context()
            },
            isolated,
            None,
            coordination_ids(221, 222),
            timestamp(1_300),
        )
        .await
        .expect("prepare isolated");
    let isolated_accepted =
        child_accepted(82, 45, parent.state().accepted().expect("parent"));
    let mut child_session = CommitCoordinator::new(store.clone());
    child_session
        .submit(
            env(3_000, 70, 71, 72),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(80),
                lane_id: id::<LaneTag>(81),
                accepted: isolated_accepted,
            }),
        )
        .await
        .expect("accept isolated");
    let parent_again = CommitCoordinator::recover(store.clone(), id(1))
        .await
        .expect("recover parent");
    let child_again = CommitCoordinator::recover(store, id(80))
        .await
        .expect("recover child");
    let isolated_map = parent_again
        .session()
        .child_mapping(id(3), id(45))
        .expect("isolated mapping");
    let child_relation = child_again
        .state()
        .accepted()
        .expect("child run")
        .relation();
    assert_eq!(isolated_map.child.operation.run_id, id(82));
    assert_eq!(child_relation.parent_run_id(), Some(id(3)));
    assert_eq!(child_relation.parent_effect_id(), Some(id(45)));
    assert_eq!(child_relation.root_run_id(), id(3));
    assert_eq!(child_relation.depth(), 1);
}
