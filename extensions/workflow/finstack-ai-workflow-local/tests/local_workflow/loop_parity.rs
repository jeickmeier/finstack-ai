#[tokio::test]
async fn trusted_session_matches_direct_owner_journal() {
    async fn run(through_session: bool) -> Vec<(String, Option<EffectId>)> {
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
        if through_session {
            drop(permit);
            drive.abort();
            let _ = drive.await;
            drop(owner);
            let mut session = WorkflowSession::trusted(store.clone(), locator(), clock, 500)
                .await
                .expect("attach")
                .with_ports(model, locked_profile(), None);
            session.ensure_owner().await.expect("spawn");
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
    assert_eq!(direct, adapted, "A01 workflow session loop parity");
    assert!(
        direct.iter().any(|(kind, _)| kind == "effect_requested"),
        "model effect present"
    );
}
