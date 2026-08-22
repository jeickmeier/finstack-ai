#[tokio::test]
async fn prefix_l1_through_l3() {
    let store = memory_store();
    let mut parent = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    parent
        .commit_session_records(
            id(205),
            vec![
                RecordDraft::try_new(
                    RECORD_FORMAT_VERSION,
                    RECORD_KIND_VERSION,
                    id(206),
                    id::<SessionTag>(1),
                    id::<LaneTag>(50),
                    None,
                    timestamp(1_050),
                    Vec::new(),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane"),
            ],
        )
        .await
        .expect("research");
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker));
    children
        .start_or_attach(
            &mut parent,
            parent_context(),
            child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51),
            None,
            coordination_ids(201, 202),
            timestamp(1_100),
        )
        .await
        .expect("prepare");
    drop(parent);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let mapping = recovered
        .session()
        .child_mapping(id(3), id(44))
        .expect("L1 mapping")
        .clone();
    assert_eq!(mapping.child.operation.run_id, id(51));
    assert_legal("L1", recovered.state().phase(), LegalRestore::Retryable);
    let attached = children
        .start_or_attach(
            &mut recovered,
            parent_context(),
            child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51),
            None,
            coordination_ids(201, 202),
            timestamp(1_100),
        )
        .await
        .expect("retry attaches");
    assert_eq!(attached.locator.operation.run_id, id(51));

    let accepted = child_accepted(51, 44, recovered.state().accepted().expect("parent"));
    store
        .append(
            AppendRequest::try_new(
                id(212),
                id::<SessionTag>(1),
                recovered.state().last_applied_sequence() + 1,
                vec![
                    RecordDraft::try_new(
                        RECORD_FORMAT_VERSION,
                        RECORD_KIND_VERSION,
                        id(210),
                        id::<SessionTag>(1),
                        id::<LaneTag>(50),
                        Some(id::<RunTag>(51)),
                        timestamp(1_200),
                        vec![id(211)],
                        RecordBody::RunAccepted(accepted),
                    )
                    .expect("child draft"),
                ],
            )
            .expect("append"),
        )
        .await
        .expect("accept child");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recovered
            .session()
            .child_mapping(id(3), id(44))
            .expect("L2")
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
    assert_legal("L2", recovered.state().phase(), LegalRestore::Retryable);

    let mut recovered = recovered;
    recovered
        .submit(
            cancel_env(2_000, 90, 190, 700),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("l3")),
            }),
        )
        .await
        .expect("parent cancel");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal("L3", recovered.state().phase(), LegalRestore::Cancelled);
}
