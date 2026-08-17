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
fn recovery_restores_last_model_continuation_from_committed_output() {
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let dispatcher = Arc::new(RecordingDispatcher::default());
    let mut coordinator =
        CommitCoordinator::with_test_dispatcher(store.clone(), dispatcher);
    block_on(drive_to_model_request(&mut coordinator));
    let continuation =
        RawJson::parse(r#"{"provider":"openai.responses","replay_items":[],"version":1}"#)
            .expect("continuation");
    let response = crate::ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )]),
        tool_calls: Arc::from([]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("response-1"),
        continuation_state: Some(continuation.clone()),
    };
    let output = RawJson::parse(
        serde_json_canonicalizer::to_vec(&response)
            .expect("response JSON")
            .as_slice(),
    )
    .expect("canonical response");
    let completion = EffectCompleted::try_new(
        id(103),
        output_contract(),
        output,
        Some(Usage::empty()),
        vec![],
        ProviderIds::empty(),
        Some("response-1"),
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
    block_on(coordinator.submit(
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
    assert_eq!(
        coordinator.last_model_continuation(),
        Some(&continuation),
        "committed completion updates live continuation"
    );

    let recovered =
        block_on(CommitCoordinator::recover(store, id::<SessionTag>(1))).expect("recover");
    assert_eq!(recovered.last_model_continuation(), Some(&continuation));
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
