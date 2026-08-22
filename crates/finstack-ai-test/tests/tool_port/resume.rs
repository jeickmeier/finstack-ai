#[tokio::test]
async fn tool_resume_unstarted_crash_retries_same_identity() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![completed_tool(9)],
        vec![ToolReconcileResult::NotStarted],
        tool_spec("echo"),
    );
    let recovered =
        crash_before_tool_dispatch(store.clone(), model.clone(), catalog.clone(), tools).await;
    assert_eq!(toolset.call_count(), 0);
    let (effect_id, tool_call_id, frozen) = requested_execute(recovered.state());
    let tool_batch_id = recovered
        .state()
        .active_tool_batch()
        .expect("batch")
        .opened
        .tool_batch_id;
    assert_eq!(
        tool_resume_action(recovered.state(), effect_id),
        ToolResumeAction::Reconcile
    );
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_500, 701)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.tool_settlements().contains_key(&effect_id)
    })
    .await;
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.reconcile_count(), 1);
    assert_eq!(toolset.last_effect_id(), Some(effect_id));
    let retried = toolset.last_call().expect("retried call");
    assert_eq!(*retried.call.tool_call_id(), tool_call_id);
    assert_eq!(retried, frozen);
    let settled = recover_session(&store).await;
    assert!(settled.state().tool_settlements().contains_key(&effect_id));
    assert_eq!(
        settled
            .state()
            .active_tool_batch()
            .map(|batch| batch.opened.tool_batch_id),
        None
    );
    let _ = tool_batch_id;
    owner.shutdown().await;
}

#[tokio::test]
async fn tool_resume_in_flight_still_running_defers_without_second_call() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![gated_tool("in-flight", 1)],
        vec![ToolReconcileResult::StillRunning(scripted_tool_deferral(
            "job-1",
        ))],
        tool_spec("echo"),
    );
    let control = toolset.control();
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        710,
    )
    .await
    .expect("owner");
    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_gate(&control, "in-flight").await;
    assert_eq!(toolset.call_count(), 1);
    drop(owner);
    let recovered = recover_session(&store).await;
    let (effect_id, _, _) = requested_execute(recovered.state());
    assert_eq!(
        tool_resume_action(recovered.state(), effect_id),
        ToolResumeAction::Reconcile
    );
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_500, 711)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::AwaitingExternal)
    })
    .await;
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.reconcile_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn tool_resume_in_flight_unknown_retries_same_effect_id() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![gated_tool("in-flight-retry", 1), completed_tool(2)],
        vec![ToolReconcileResult::Unknown],
        tool_spec("echo"),
    );
    let control = toolset.control();
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        720,
    )
    .await
    .expect("owner");
    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_gate(&control, "in-flight-retry").await;
    let recovered = recover_session(&store).await;
    let (effect_id, tool_call_id, frozen) = requested_execute(recovered.state());
    drop(owner);
    let recovered = recover_session(&store).await;
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_500, 721)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.tool_settlements().contains_key(&effect_id)
    })
    .await;
    assert_eq!(toolset.call_count(), 2);
    assert_eq!(toolset.last_effect_id(), Some(effect_id));
    let retried = toolset.last_call().expect("retried call");
    assert_eq!(*retried.call.tool_call_id(), tool_call_id);
    assert_eq!(retried, frozen);
    owner.shutdown().await;
}

#[tokio::test]
async fn tool_resume_completed_uncommitted_settles_without_call() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![completed_tool(3)],
        vec![ToolReconcileResult::Completed(tool_result(3))],
        tool_spec("echo"),
    );
    let recovered =
        crash_before_tool_dispatch(store.clone(), model.clone(), catalog.clone(), tools).await;
    let (effect_id, _, _) = requested_execute(recovered.state());
    assert_eq!(
        tool_resume_action(recovered.state(), effect_id),
        ToolResumeAction::Reconcile
    );
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_500, 731)
        .await
        .expect("respawn");
    wait_state(&store, |state| {
        state.tool_settlements().contains_key(&effect_id)
    })
    .await;
    assert_eq!(toolset.call_count(), 0);
    assert_eq!(toolset.reconcile_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn tool_resume_deferred_same_handle_waits_and_completed_settles_externally() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![gated_tool("defer", 1)],
        vec![
            ToolReconcileResult::StillRunning(scripted_tool_deferral("job-1")),
            ToolReconcileResult::StillRunning(scripted_tool_deferral("job-1")),
            ToolReconcileResult::Completed(tool_result(4)),
        ],
        tool_spec("echo"),
    );
    let control = toolset.control();
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        740,
    )
    .await
    .expect("owner");
    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_gate(&control, "defer").await;
    drop(owner);
    let recovered = recover_session(&store).await;
    let (effect_id, _, _) = requested_execute(recovered.state());
    let mut owner = spawn_tool_owner(recovered, model.clone(), catalog.clone(), 2_500, 741)
        .await
        .expect("ensure deferred");
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::AwaitingExternal)
    })
    .await;
    let calls = toolset.call_count();
    owner.shutdown().await;

    let recovered = recover_session(&store).await;
    assert_eq!(
        tool_resume_action(recovered.state(), effect_id),
        ToolResumeAction::Reconcile
    );
    let mut owner = spawn_tool_owner(recovered, model.clone(), catalog.clone(), 2_500, 742)
        .await
        .expect("respawn wait");
    assert_eq!(toolset.call_count(), calls);
    assert_eq!(
        recover_session(&store).await.state().phase(),
        Some(RunPhase::AwaitingExternal)
    );
    owner.shutdown().await;

    let recovered = recover_session(&store).await;
    let mut owner = spawn_tool_owner(recovered, model.clone(), catalog.clone(), 2_600, 743)
        .await
        .expect("respawn complete");
    wait_state(&store, |state| {
        state.tool_settlements().contains_key(&effect_id)
    })
    .await;
    assert_eq!(toolset.call_count(), calls);
    owner.shutdown().await;

    let recovered = recover_session(&store).await;
    assert!(matches!(
        tool_resume_action(recovered.state(), effect_id),
        ToolResumeAction::UseRecorded | ToolResumeAction::NoOutstanding
    ));
    let reconciles = toolset.reconcile_count();
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_600, 744)
        .await
        .expect("equal completion idempotent");
    assert_eq!(toolset.call_count(), calls);
    assert_eq!(toolset.reconcile_count(), reconciles);
    owner.shutdown().await;
}

#[tokio::test]
async fn tool_resume_non_resumable_suspends_without_call() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![completed_tool(1)],
        vec![ToolReconcileResult::Unknown],
        at_most_once_spec(),
    );
    let recovered =
        crash_before_tool_dispatch(store.clone(), model.clone(), catalog.clone(), tools).await;
    let (effect_id, _, _) = requested_execute(recovered.state());
    assert_eq!(
        tool_resume_action(recovered.state(), effect_id),
        ToolResumeAction::Reconcile
    );
    let Err(error) = spawn_tool_owner(recovered, model, catalog, 2_500, 751).await else {
        panic!("suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Tool {
            code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(toolset.call_count(), 0);
    assert_eq!(
        recover_session(&store).await.state().phase(),
        Some(RunPhase::AwaitingTools)
    );

    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![completed_tool(1)],
        vec![ToolReconcileResult::Unknown],
        non_idempotent_spec(),
    );
    let recovered =
        crash_before_tool_dispatch(store.clone(), model.clone(), catalog.clone(), tools).await;
    let Err(error) = spawn_tool_owner(recovered, model, catalog, 2_500, 752).await else {
        panic!("non-idempotent suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Tool {
            code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(toolset.call_count(), 0);

    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![completed_tool(1)],
        vec![ToolReconcileResult::NonRepeatable],
        tool_spec("echo"),
    );
    let recovered =
        crash_before_tool_dispatch(store.clone(), model.clone(), catalog.clone(), tools).await;
    let Err(error) = spawn_tool_owner(recovered, model, catalog, 2_500, 753).await else {
        panic!("non-repeatable suspend");
    };
    assert_eq!(
        error,
        RunHandleError::Tool {
            code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert_eq!(toolset.call_count(), 0);
}

#[tokio::test]
async fn tool_resume_settled_effect_never_calls_or_reconciles() {
    let (store, toolset, model, catalog, tools) =
        resume_ports(1, vec![completed_tool(5)], Vec::new(), tool_spec("echo"));
    let mut owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        760,
    )
    .await
    .expect("owner");
    drive_to_tools(&owner.handle(), &store, tools).await;
    let settled = wait_state(&store, |state| !state.tool_settlements().is_empty()).await;
    let effect_id = *settled.state().tool_settlements().keys().next().expect("id");
    let calls = toolset.call_count();
    owner.shutdown().await;
    let recovered = recover_session(&store).await;
    assert!(matches!(
        tool_resume_action(recovered.state(), effect_id),
        ToolResumeAction::UseRecorded | ToolResumeAction::NoOutstanding
    ));
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_500, 761)
        .await
        .expect("respawn");
    assert_eq!(toolset.call_count(), calls);
    assert_eq!(toolset.reconcile_count(), 0);
    owner.shutdown().await;
}

#[tokio::test]
async fn tool_resume_completed_subset_retries_outstanding_in_source_order() {
    let mut spec = tool_spec("echo");
    spec.execution = ToolExecutionMode::Sequential;
    let (store, toolset, model, catalog, tools) = resume_ports(
        2,
        vec![
            completed_tool(0),
            gated_tool("subset", 1),
            completed_tool(1),
        ],
        vec![ToolReconcileResult::Unknown],
        spec,
    );
    let control = toolset.control();
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        770,
    )
    .await
    .expect("owner");
    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_gate(&control, "subset").await;
    drop(owner);
    let recovered = recover_session(&store).await;
    let batch = recovered
        .state()
        .active_tool_batch()
        .expect("batch")
        .clone();
    assert_eq!(batch.calls.len(), 2);
    let first = batch.calls[0].assigned.effect_id;
    let second = batch.calls[1].assigned.effect_id;
    assert!(matches!(
        batch.calls[0].status,
        ActiveToolCallStatus::Settled { .. }
    ));
    assert!(matches!(
        batch.calls[1].status,
        ActiveToolCallStatus::Requested { deferred: None, .. }
    ));
    assert_eq!(
        tool_resume_action(recovered.state(), first),
        ToolResumeAction::UseRecorded
    );
    assert_eq!(
        tool_resume_action(recovered.state(), second),
        ToolResumeAction::Reconcile
    );
    let calls = toolset.call_count();
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_500, 771)
        .await
        .expect("respawn");
    let settled = wait_state(&store, |state| {
        state.tool_settlements().contains_key(&first) && state.tool_settlements().contains_key(&second)
    })
    .await;
    assert_eq!(toolset.call_count(), calls + 1);
    assert_eq!(toolset.last_effect_id(), Some(second));
    assert_eq!(
        tool_result_call_ids(settled.state()),
        source_tool_call_ids(settled.state())
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn tool_resume_conflicting_deferred_handle_fails_closed() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![gated_tool("conflict", 1)],
        vec![
            ToolReconcileResult::StillRunning(scripted_tool_deferral("job-1")),
            ToolReconcileResult::StillRunning(scripted_tool_deferral("job-other")),
        ],
        tool_spec("echo"),
    );
    let control = toolset.control();
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        780,
    )
    .await
    .expect("owner");
    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_gate(&control, "conflict").await;
    drop(owner);
    let recovered = recover_session(&store).await;
    let mut owner = spawn_tool_owner(recovered, model.clone(), catalog.clone(), 2_500, 781)
        .await
        .expect("ensure deferred");
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::AwaitingExternal)
    })
    .await;
    owner.shutdown().await;
    let recovered = recover_session(&store).await;
    let Err(error) = spawn_tool_owner(recovered, model, catalog, 2_500, 782).await else {
        panic!("conflict");
    };
    assert_eq!(
        error,
        RunHandleError::Tool {
            code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
        }
    );
    assert!(journal_has_rejection(&store).await);
    assert_eq!(
        recover_session(&store).await.state().phase(),
        Some(RunPhase::AwaitingExternal)
    );
    assert_eq!(toolset.call_count(), 1);
}

#[tokio::test]
async fn cancel_while_deferred_tool_does_not_issue_a_second_request() {
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![gated_tool("defer-cancel", 1)],
        vec![ToolReconcileResult::StillRunning(scripted_tool_deferral(
            "job-cancel",
        ))],
        tool_spec("echo"),
    );
    let control = toolset.control();
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        800,
    )
    .await
    .expect("owner");
    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_gate(&control, "defer-cancel").await;
    drop(owner);
    let recovered = recover_session(&store).await;
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_500, 801)
        .await
        .expect("deferred");
    wait_state(&store, |state| {
        state.phase() == Some(RunPhase::AwaitingExternal)
    })
    .await;
    owner
        .handle()
        .submit(
            cancellation_env(2_600, 800),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("deferred-tool-cancel")),
            }),
        )
        .await
        .expect("cancel");
    wait_state(&store, |state| {
        matches!(state.phase(), Some(RunPhase::Cancelled | RunPhase::Suspended))
    })
    .await;
    assert_eq!(toolset.call_count(), 1);
    owner.shutdown().await;
}
