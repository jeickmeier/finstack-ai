fn cancel_env(now: i64, record: u64, append_batch: u64, request: u64) -> TransitionEnv {
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
            vec![id(append_batch)],
            vec![id::<CancellationRequestTag>(request)],
        )
        .expect("cancel ids"),
    }
}

#[tokio::test]
async fn run_level_cancel_while_awaiting_interaction_closes_without_dispatching() {
    let (ports, mut owner) = park_on_approval(None).await;
    owner
        .handle()
        .submit(
            cancel_env(2_400, 90, 190, 700),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("approval-cancel")),
            }),
        )
        .await
        .expect("cancel");
    wait_state(&ports.store, |state| {
        state.phase() == Some(RunPhase::Cancelled)
    })
    .await;
    let cancelled = recover_session(&ports.store).await;
    assert_eq!(
        cancelled
            .state()
            .last_interaction_terminal()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Cancelled
    );
    assert_eq!(ports.toolset.call_count(), 0);
    owner.shutdown().await;
}

#[tokio::test]
async fn late_privileged_resolution_after_run_cancel_fails_closed() {
    let (ports, mut owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    owner
        .handle()
        .submit(
            cancel_env(2_400, 91, 191, 701),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("approval-cancel")),
            }),
        )
        .await
        .expect("cancel");
    wait_state(&ports.store, |state| {
        state.phase() == Some(RunPhase::Cancelled)
    })
    .await;
    owner.shutdown().await;
    let sink = RecordingSink::new();
    let router = audited_router(ports.store.clone(), Arc::clone(&sink)).await;
    let outcome = router
        .route(resolve_command(interaction_id, true), timestamp(2_700))
        .await
        .expect("late privileged");
    assert!(matches!(outcome, ExternalRouteOutcome::Rejected { .. }));
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"external_command_rejected")
    );
    assert_eq!(ports.toolset.call_count(), 0);
}
