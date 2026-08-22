#[tokio::test]
async fn stop_after_approval_request_waits_without_dispatch() {
    let (ports, owner) = park_on_approval(None).await;
    assert_eq!(ports.toolset.call_count(), 0);
    let recovered = crash_owner(owner, &ports.store).await;
    assert_eq!(recovered.state().phase(), Some(RunPhase::AwaitingInteraction));
    assert_eq!(
        interaction_resume_action(recovered.state(), timestamp(2_500)),
        InteractionResumeAction::WaitResolution
    );
    assert!(
        !record_kinds(&ports.store)
            .await
            .contains(&"effect_requested_tool")
    );
    let listed = router(ports.store.clone())
        .await
        .list(&locator(), &principal(), &authorization(), timestamp(2_500))
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind(), &InteractionKind::Approval);
}

#[tokio::test]
async fn grant_after_stop_dispatches_the_protected_tool_once() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    let recovered = crash_owner(owner, &ports.store).await;
    let _ = recovered;
    let outcome = router(ports.store.clone())
        .await
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("grant");
    assert!(matches!(outcome, ExternalRouteOutcome::Committed(_)));
    let recovered = recover_session(&ports.store).await;
    assert_eq!(recovered.state().phase(), Some(RunPhase::BeforeToolBatch));
    assert_eq!(
        recovered
            .state()
            .last_interaction_terminal()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Granted
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_700,
        800,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |_| ports.toolset.call_count() == 1).await;
    assert_eq!(ports.toolset.call_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn denied_approval_never_dispatches_the_protected_tool() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    drop(owner);
    router(ports.store.clone())
        .await
        .route(resolve_command(interaction_id, false), timestamp(2_600))
        .await
        .expect("deny");
    let recovered = recover_session(&ports.store).await;
    assert_eq!(
        recovered
            .state()
            .last_interaction_terminal()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Denied
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_700,
        801,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |state| {
        state.phase() == Some(RunPhase::AfterToolBatch)
            || state
                .active_tool_batch()
                .is_some_and(|batch| !batch.calls.is_empty())
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    assert!(
        !record_kinds(&ports.store)
            .await
            .contains(&"effect_requested_tool")
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn expired_resolution_never_dispatches_the_protected_tool() {
    let (ports, owner) = park_on_approval(Some(timestamp(3_000))).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    owner
        .handle()
        .submit(
            resolve_env(3_000),
            KernelInput::InteractionSettled(InteractionSettled::Expired(
                finstack_ai_kernel::InteractionExpired {
                    interaction_id,
                    expired_at: timestamp(3_000),
                },
            )),
        )
        .await
        .expect("live expire");
    drop(owner);
    let recovered = recover_session(&ports.store).await;
    assert_eq!(
        recovered
            .state()
            .last_interaction_terminal()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Expired
    );
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"interaction_expired")
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_700,
        802,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |state| {
        state.phase() == Some(RunPhase::AfterToolBatch) || state.last_interaction_terminal().is_some()
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    owner.shutdown().await;
}

#[tokio::test]
async fn expire_if_due_on_restore_never_dispatches() {
    let (ports, owner) = park_on_approval(Some(timestamp(3_000))).await;
    let recovered = crash_owner(owner, &ports.store).await;
    assert_eq!(
        interaction_resume_action(recovered.state(), timestamp(4_000)),
        InteractionResumeAction::ExpireIfDue
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        4_000,
        803,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |state| {
        state
            .last_interaction_terminal()
            .is_some_and(|terminal| terminal.outcome == InteractionTerminalOutcome::Expired)
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"interaction_expired")
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn duplicate_resolution_is_idempotent() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    drop(owner);
    let router = router(ports.store.clone()).await;
    let first = router
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("first");
    assert!(matches!(first, ExternalRouteOutcome::Committed(_)));
    let second = router
        .route(resolve_command(interaction_id, true), timestamp(2_700))
        .await
        .expect("duplicate");
    assert!(matches!(second, ExternalRouteOutcome::Idempotent { .. }));
    let kinds = record_kinds(&ports.store).await;
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "interaction_resolved")
            .count(),
        1
    );
}

#[tokio::test]
async fn conflicting_resolution_fails_closed_and_is_audited() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    drop(owner);
    let sink = RecordingSink::new();
    let router = audited_router(ports.store.clone(), Arc::clone(&sink)).await;
    router
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("first");
    let outcome = router
        .route(resolve_command(interaction_id, false), timestamp(2_700))
        .await
        .expect("conflict");
    assert!(matches!(
        outcome,
        ExternalRouteOutcome::Rejected {
            reason_code: "conflicting_or_invalid_resolution",
            ..
        }
    ));
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"external_command_rejected")
    );
}

#[tokio::test]
async fn cancelled_while_waiting_never_dispatches() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    owner
        .handle()
        .submit(
            resolve_env(2_600),
            KernelInput::InteractionSettled(InteractionSettled::Cancelled(
                InteractionCancelled::try_new(interaction_id, None, None, Some("operator-cancel"))
                    .expect("cancelled"),
            )),
        )
        .await
        .expect("cancel");
    wait_state(&ports.store, |state| {
        state
            .last_interaction_terminal()
            .is_some_and(|terminal| terminal.outcome == InteractionTerminalOutcome::Cancelled)
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"interaction_cancelled")
    );
    drop(owner);
}
