#[tokio::test]
async fn retry_scheduled_before_crash_fires_once_and_second_respawn_is_idempotent() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let owner = park_on_sleeping(&store, Arc::clone(&model), 2_000, 800, 10).await;
    let sleeping = recover_session(&store).await;
    let pending = sleeping
        .state()
        .retry()
        .pending
        .as_ref()
        .expect("pending timer")
        .clone();
    assert_eq!(pending.attempt, 1);
    drop(owner);

    let recovered = recover_session(&store).await;
    let replacement = spawn_model_owner(recovered, Arc::clone(&model), 20_000, 801)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::PreparingContext)
    })
    .await;
    let fired = recover_session(&store).await;
    assert_eq!(fired.state().retry().attempts, 1);
    assert!(fired.state().retry().pending.is_none());
    assert_eq!(fired.state().retry().timer_firings.len(), 1);
    let firing = fired
        .state()
        .retry()
        .timer_firings
        .get(&pending.timer_effect_id)
        .expect("same timer");
    assert_eq!(firing.due_at, pending.due_at);
    assert_eq!(replacement.handle().timer_diagnostics().already_due, 1);
    drop(replacement);

    let recovered = recover_session(&store).await;
    let mut second = spawn_model_owner(recovered, model, 30_000, 802)
        .await
        .expect("second respawn");
    tokio::task::yield_now().await;
    let again = recover_session(&store).await;
    assert_eq!(again.state().phase(), Some(RunPhase::PreparingContext));
    assert_eq!(again.state().retry().timer_firings.len(), 1);
    assert_eq!(again.state().retry().attempts, 1);
    second.shutdown().await;
}

#[tokio::test]
async fn cancel_while_sleeping_closes_without_firing_the_timer() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let mut owner = park_on_sleeping(&store, model, 2_000, 810, 10_000).await;
    owner
        .handle()
        .submit(
            cancel_env(2_400, 20, 205, 800),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("sleeping-cancel")),
            }),
        )
        .await
        .expect("cancel");
    wait_state(&store, |state| state.phase() == Some(RunPhase::Cancelled)).await;
    let cancelled = recover_session(&store).await;
    assert!(cancelled.state().retry().pending.is_none());
    assert!(cancelled.state().retry().timer_firings.is_empty());
    owner.shutdown().await;
}

#[tokio::test]
async fn cancel_while_deferred_model_does_not_issue_a_second_request() {
    let store = memory_store();
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![deferred_plan("job-1")],
    ));
    let mut owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        820,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::AwaitingExternal)
    })
    .await;
    assert_eq!(model.request_count(), 1);
    owner
        .handle()
        .submit(
            cancel_env(2_400, 20, 205, 800),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("deferred-cancel")),
            }),
        )
        .await
        .expect("cancel");
    wait_state(&store, |state| {
        matches!(state.phase(), Some(RunPhase::Cancelled | RunPhase::Suspended))
    })
    .await;
    assert_eq!(model.request_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn cancel_unknown_deferred_model_suspends_without_fabricating_success() {
    let store = memory_store();
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![deferred_plan("job-unknown")],
    ));
    let mut owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        821,
    )
    .await
    .expect("owner");
    drive_to_active_model_request_with(&owner.handle(), RetrySafety::Unknown).await;
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::AwaitingExternal)
    })
    .await;
    owner
        .handle()
        .submit(
            cancel_env(2_400, 20, 205, 800),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("uncertain-cancel")),
            }),
        )
        .await
        .expect("cancel");
    wait_state(&store, |state| state.phase() == Some(RunPhase::Suspended)).await;
    let suspended = recover_session(&store).await;
    assert!(suspended.state().terminal().is_none());
    assert!(suspended.state().messages().is_empty());
    assert_eq!(model.request_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn overdue_and_backward_clock_restart_are_bounded() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let owner = park_on_sleeping(&store, Arc::clone(&model), 2_000, 830, 10).await;
    drop(owner);

    let overdue = ExternalClock::new(timestamp(20_000));
    let recovered = recover_session(&store).await;
    let mut replacement = spawn_model_owner_with_clock(recovered, Arc::clone(&model), overdue, 831)
        .await
        .expect("overdue respawn");
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::PreparingContext)
    })
    .await;
    assert!(replacement.handle().timer_diagnostics().already_due >= 1);
    replacement.shutdown().await;

    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let owner = park_on_sleeping(&store, Arc::clone(&model), 2_000, 840, 10).await;
    drop(owner);
    let backward = ExternalClock::new(timestamp(0));
    let recovered = recover_session(&store).await;
    let mut replacement = spawn_model_owner_with_clock(recovered, model, backward, 841)
        .await
        .expect("backward respawn");
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::PreparingContext)
            || replacement
                .handle()
                .timer_diagnostics()
                .backward_clock_clamped
                >= 1
    })
    .await;
    assert!(
        replacement
            .handle()
            .timer_diagnostics()
            .backward_clock_clamped
            >= 1
            || recover_session(&store).await.state().phase() == Some(RunPhase::PreparingContext)
    );
    replacement.shutdown().await;
}
