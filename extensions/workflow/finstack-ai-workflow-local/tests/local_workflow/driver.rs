#[tokio::test]
async fn adapter_matches_direct_owner_journal() {
    async fn run(through_adapter: bool) -> Vec<(String, Option<EffectId>)> {
        let store = memory_store();
        let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![completed_plan("hello")],
        ));
        let clock = ExternalClock::new(timestamp(2_000));
        let mut coordinator = CommitCoordinator::new(store.clone());
        let mut controller = coordinator.enable_manual_drive(1).expect("manual drive");
        let owner = spawn_model_owner(coordinator, Arc::clone(&model), clock.clone(), 500).await;
        let handle = owner.handle();
        let drive = tokio::spawn(async move { drive_to_active_model_request(&handle).await });
        let permit = tokio::time::timeout(StdDuration::from_secs(2), controller.next_effect())
            .await
            .expect("pause bound")
            .expect("paused execute");
        assert_eq!(permit.effect().action, ManualDriveAction::Execute);
        wait_state(&store, |state| state.pending_model_effect.is_some()).await;
        if through_adapter {
            drop(permit);
            drive.abort();
            let _ = drive.await;
            drop(owner);
            let mut driver = attach_driver(store.clone(), model, clock, 500).await;
            driver.ensure_owner().await.expect("spawn");
            wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
        } else {
            permit.continue_dispatch();
            drive.await.expect("drive");
            wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
            drop(owner);
        }
        journal_trace(&store).await
    }

    let direct = Box::pin(run(false)).await;
    let adapted = Box::pin(run(true)).await;
    let kinds = |rows: &[(String, Option<EffectId>)]| {
        rows.iter()
            .map(|(kind, effect)| (kind.clone(), *effect))
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&direct), kinds(&adapted), "A01 journal parity");
    assert!(
        direct.iter().any(|(kind, _)| kind == "effect_requested"),
        "model effect present"
    );
}

#[tokio::test]
async fn conflicting_checkpoint_sequence_is_ignored() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        501,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
    drop(owner);
    let driver = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 501).await;
    let mut hint = driver.persist_handoff().expect("handoff");
    let journal_seq = hint.last_applied_seq;
    hint.last_applied_seq = journal_seq.saturating_add(99);
    assert_eq!(
        finstack_ai_runtime::workflow::resolve_checkpoint_sequence(journal_seq, Some(hint.last_applied_seq)),
        journal_seq
    );
    let _ = (store, clock, model);
}

#[tokio::test]
async fn drive_timeout_is_configurable_and_defaults_to_two_seconds() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("done")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        900,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    drop(owner);
    let driver = attach_driver(store, model, clock, 900).await;
    assert_eq!(driver.drive_timeout(), StdDuration::from_secs(2));
    let session = driver
        .into_session()
        .with_drive_timeout(StdDuration::from_millis(250));
    assert_eq!(session.drive_timeout(), StdDuration::from_millis(250));
}
