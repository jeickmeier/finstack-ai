#[tokio::test]
async fn model_resume_unstarted_crash_retries_same_identity() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("retried")])
            .with_reconcile_results(vec![ModelReconcileResult::NotStarted]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    assert_eq!(model.request_count(), 0);
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let pending = recovered
        .state()
        .pending_model_effect
        .clone()
        .expect("pending");
    let effect_id = pending.requested.effect_id();
    let request_id = pending.model_request_id;
    let input_digest = pending.requested.input_digest();
    let canonical = match pending.requested.input() {
        EffectInput::Model { request } => request.clone(),
        other => panic!("expected model input, got {other:?}"),
    };
    assert_eq!(
        input_digest,
        pending.requested.input().digest().expect("input digest")
    );
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 701)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.model_settlements.contains_key(&effect_id)
    })
    .await;
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.reconcile_count(), 1);
    let retried = model.last_request().expect("retried request");
    assert_eq!(retried.call.run.effect_id, effect_id);
    assert_eq!(retried.call.request_id, request_id);
    assert_eq!(
        retried
            .draft
            .canonical_bytes()
            .expect("canonical")
            .as_slice(),
        canonical.as_bytes()
    );
    let settled = recover_session(&store).await;
    assert_eq!(
        settled
            .state()
            .pending_model_effect
            .as_ref()
            .map(|pending| pending.requested.effect_id()),
        None
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_in_flight_still_running_defers_without_second_request() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(
            profile(),
            vec![ScriptedModelPlan {
                actions: vec![
                    ScriptedModelAction::Block(Arc::from("in-flight")),
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("late")))),
                ],
            }],
        )
        .with_reconcile_results(vec![ModelReconcileResult::StillRunning(
            scripted_deferral("job-1"),
        )]),
    );
    let control = model.control();
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        710,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    tokio::time::timeout(StdDuration::from_secs(1), async {
        while control.entries("in-flight") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("entered");
    assert_eq!(model.request_count(), 1);
    drop(owner);
    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 711)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.reconcile_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_in_flight_unknown_retries_same_effect_id() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(
            profile(),
            vec![
                ScriptedModelPlan {
                    actions: vec![
                        ScriptedModelAction::Block(Arc::from("in-flight-retry")),
                        ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(
                            "first",
                        )))),
                    ],
                },
                completed_plan("retried"),
            ],
        )
        .with_reconcile_results(vec![ModelReconcileResult::Unknown]),
    );
    let control = model.control();
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        720,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    tokio::time::timeout(StdDuration::from_secs(1), async {
        while control.entries("in-flight-retry") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("entered");
    let pending = recover_session(&store)
        .await
        .state()
        .pending_model_effect
        .clone()
        .expect("pending");
    let effect_id = pending.requested.effect_id();
    let request_id = pending.model_request_id;
    let input_digest = pending.requested.input_digest();
    let canonical = match pending.requested.input() {
        EffectInput::Model { request } => request.clone(),
        other => panic!("expected model input, got {other:?}"),
    };
    assert_eq!(
        input_digest,
        pending.requested.input().digest().expect("input digest")
    );
    drop(owner);
    let recovered = recover_session(&store).await;
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 721)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.model_settlements.contains_key(&effect_id)
    })
    .await;
    assert_eq!(model.request_count(), 2);
    let retried = model.last_request().expect("retried request");
    assert_eq!(retried.call.run.effect_id, effect_id);
    assert_eq!(retried.call.request_id, request_id);
    assert_eq!(
        retried
            .draft
            .canonical_bytes()
            .expect("canonical")
            .as_slice(),
        canonical.as_bytes()
    );
    assert!(
        recover_session(&store)
            .await
            .state()
            .model_settlements
            .contains_key(&effect_id)
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_completed_uncommitted_settles_without_request() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_reconcile_results(vec![ModelReconcileResult::Completed(completed("hello"))]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let effect_id = recovered
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending")
        .requested
        .effect_id();
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 731)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.model_settlements.contains_key(&effect_id)
    })
    .await;
    assert_eq!(model.request_count(), 0);
    assert_eq!(model.reconcile_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_deferred_same_handle_waits_and_completed_settles_externally() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![deferred_plan("job-1")]).with_reconcile_results(
            vec![
                ModelReconcileResult::StillRunning(scripted_deferral("job-1")),
                ModelReconcileResult::Completed(completed("external")),
            ],
        ),
    );
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        740,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);
    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let requests = model.request_count();
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 741)
        .await
        .expect("respawn wait");
    assert_eq!(model.request_count(), requests);
    assert_eq!(
        recover_session(&store).await.state().phase,
        Some(RunPhase::AwaitingExternal)
    );
    owner.shutdown().await;

    let recovered = recover_session(&store).await;
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_600, 742)
        .await
        .expect("respawn complete");
    wait_state(&store, |state| {
        state.phase != Some(RunPhase::AwaitingExternal) && state.pending_model_effect.is_none()
    })
    .await;
    assert_eq!(model.request_count(), requests);
    owner.shutdown().await;

    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::NoOutstanding
    );
    let reconciles = model.reconcile_count();
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_600, 743)
        .await
        .expect("equal completion idempotent");
    assert_eq!(model.request_count(), requests);
    assert_eq!(model.reconcile_count(), reconciles);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_non_resumable_suspends_without_request() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_reconcile_results(vec![ModelReconcileResult::Unknown]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::AtMostOnce).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::Reconcile
    );
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 751).await else {
        panic!("suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(model.request_count(), 0);
    assert_eq!(
        recover_session(&store).await.state().phase,
        Some(RunPhase::AwaitingModel)
    );

    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_idempotent_requests(false)
            .with_reconcile_results(vec![ModelReconcileResult::Unknown]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 752).await else {
        panic!("non-idempotent suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(model.request_count(), 0);

    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![completed_plan("unused")])
            .with_reconcile_results(vec![ModelReconcileResult::NonRepeatable]),
    );
    let recovered =
        crash_before_dispatch(store.clone(), model.clone(), RetrySafety::SafeToRetry).await;
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 753).await else {
        panic!("non-repeatable suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(model.request_count(), 0);
}

#[tokio::test]
async fn model_resume_settled_effect_never_requests_or_reconciles() {
    let store = memory_store();
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let mut owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        760,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.pending_model_effect.is_none() && !state.model_settlements.is_empty()
    })
    .await;
    let requests = model.request_count();
    owner.shutdown().await;
    let recovered = recover_session(&store).await;
    assert_eq!(
        model_resume_action(recovered.state()),
        ModelResumeAction::NoOutstanding
    );
    let mut owner = spawn_model_owner(recovered, model.clone(), 2_500, 761)
        .await
        .expect("respawn");
    assert_eq!(model.request_count(), requests);
    assert_eq!(model.reconcile_count(), 0);
    owner.shutdown().await;
}

#[tokio::test]
async fn model_resume_conflicting_deferred_handle_fails_closed() {
    let store = memory_store();
    let model = Arc::new(
        ScriptedModel::from_plans(profile(), vec![deferred_plan("job-1")]).with_reconcile_results(
            vec![ModelReconcileResult::StillRunning(scripted_deferral(
                "job-other",
            ))],
        ),
    );
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        2_000,
        770,
    )
    .await
    .expect("owner");
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);
    let recovered = recover_session(&store).await;
    let Err(error) = spawn_model_owner(recovered, model.clone(), 2_500, 771).await else {
        panic!("conflict");
    };
    assert_eq!(
        error,
        RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert!(journal_has_rejection(&store).await);
    assert_eq!(
        recover_session(&store).await.state().phase,
        Some(RunPhase::AwaitingExternal)
    );
    assert_eq!(model.request_count(), 1);
}
