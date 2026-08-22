#[tokio::test]
async fn prefix_b1_through_b4() {
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
        .expect("lane");
    let reserve = reserve_request();
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker));
    let failed = children
        .start_or_attach(
            &mut parent,
            parent_context_effect(344),
            budget_child_request(),
            Some(reserve.clone()),
            budget_ids(),
            timestamp(1_100),
        )
        .await;
    assert!(
        matches!(
            failed,
            Err(CompositionError::ServiceMissing {
                service: "budget_ledger"
            })
        ),
        "B1 does not silently grant"
    );
    drop(parent);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let replay = recovered
        .state()
        .budget_reservations()
        .get(&reserve.reservation_id)
        .expect("B1 requested");
    assert!(replay.settlement.is_none());
    assert_legal("B1", recovered.state().phase(), LegalRestore::Retryable);

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
        .expect("lane");
    let ledger = Arc::new(TestLedger::new());
    let children =
        ChildRunCoordinator::new(Arc::new(RecordingInvoker)).with_budget_ledger(ledger.clone());
    children
        .start_or_attach(
            &mut parent,
            parent_context_effect(344),
            budget_child_request(),
            Some(reserve.clone()),
            budget_ids(),
            timestamp(1_100),
        )
        .await
        .expect("B2 settle");
    drop(parent);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(
        recovered
            .state()
            .budget_reservations()
            .get(&reserve.reservation_id)
            .expect("settled")
            .settlement
            .is_some()
    );
    children
        .start_or_attach(
            &mut recovered,
            parent_context_effect(344),
            budget_child_request(),
            Some(reserve.clone()),
            budget_ids(),
            timestamp(1_100),
        )
        .await
        .expect("equal reserve");
    assert_eq!(ledger.reserve_calls(), 1, "B2 no double-allocate");
    assert_legal("B2", recovered.state().phase(), LegalRestore::Retryable);

    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut recovered).await;
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    settle_model_completed(&mut recovered).await;
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let budget = BudgetCoordinator::new(ledger.clone());
    let charge = charge_request();
    budget
        .charge_committed(
            &mut recovered,
            &parent_locator(),
            charge.clone(),
            BudgetOperationIds {
                batch_id: id(406),
                record_id: id(407),
            },
            timestamp(1_450),
        )
        .await
        .expect("charge");
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    budget
        .charge_committed(
            &mut recovered,
            &parent_locator(),
            charge,
            BudgetOperationIds {
                batch_id: id(406),
                record_id: id(407),
            },
            timestamp(1_450),
        )
        .await
        .expect("equal charge");
    assert_eq!(ledger.charge_calls(), 1, "B4 no double-charge");
    assert_legal("B4", recovered.state().phase(), LegalRestore::Retryable);

    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    recovered
        .submit(
            env(1_500, &[9], &[], &[], &[], &[], &[], 108),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    recovered
        .submit(
            env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 109),
            stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        )
        .await
        .expect("terminal");
    budget
        .release_committed(
            &mut recovered,
            &parent_locator(),
            release_request(),
            BudgetOperationIds {
                batch_id: id(408),
                record_id: id(409),
            },
            timestamp(1_700),
        )
        .await
        .expect("release");
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    budget
        .release_committed(
            &mut recovered,
            &parent_locator(),
            release_request(),
            BudgetOperationIds {
                batch_id: id(408),
                record_id: id(409),
            },
            timestamp(1_700),
        )
        .await
        .expect("equal release");
    assert_eq!(ledger.release_calls(), 1, "B3 release idempotent");
    assert_legal("B3", recovered.state().phase(), LegalRestore::Completed);
}
