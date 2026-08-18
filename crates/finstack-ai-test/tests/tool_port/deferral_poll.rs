use finstack_ai_kernel::{ComponentId, ExternalHandleRef, ReconciliationPolicy};
use finstack_ai_runtime::{ToolDeferral, TOOL_DEFERRAL_EXPIRED};

fn due_polls(
    state: &finstack_ai_kernel::KernelState,
    now: finstack_ai_kernel::Timestamp,
) -> Vec<(finstack_ai_kernel::EffectId, finstack_ai_kernel::Timestamp)> {
    finstack_ai_runtime::__test_due_polls(state, now)
}

fn polling_deferral(
    handle: &str,
    next_poll_at: i64,
    expires_at: Option<i64>,
) -> ToolDeferral {
    ToolDeferral {
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tool.scripted").expect("component"),
            handle,
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::Poll,
        next_poll_at: Some(timestamp(next_poll_at)),
        expires_at: expires_at.map(timestamp),
    }
}

fn externally_waiting_deferral(handle: &str) -> ToolDeferral {
    let mut deferral = polling_deferral(handle, 2_000, None);
    deferral.next_poll_at = None;
    deferral
}

#[tokio::test]
async fn restore_rebuilds_poll_deadline_from_committed_deferral() {
    let next_poll_at = timestamp(2_500);
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", next_poll_at.as_unix_ms(), None),
        )))],
    };
    let (store, _, model, catalog, tools) =
        resume_ports(1, vec![plan], Vec::new(), spec);
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model,
        catalog,
        2_000,
        810,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    let committed = wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    let effect_id = committed
        .state()
        .active_tool_batch
        .as_ref()
        .and_then(|batch| batch.calls.first())
        .expect("deferred tool call")
        .assigned
        .effect_id;
    drop(owner);

    let recovered = CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover committed deferral");

    assert_eq!(
        due_polls(recovered.state(), timestamp(2_000)),
        vec![(effect_id, next_poll_at)]
    );
}

#[tokio::test]
async fn deferred_poll_reconciles_when_due_and_rearms_until_completion() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", 2_500, None),
        )))],
    };
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![plan],
        vec![
            ToolReconcileResult::StillRunning(polling_deferral("job-1", 2_600, None)),
            ToolReconcileResult::StillRunning(polling_deferral("job-1", 2_700, None)),
            ToolReconcileResult::Completed(tool_result(3)),
        ],
        spec,
    );
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        810,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    let recovered = wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    let effect_id = recovered
        .state()
        .active_tool_batch
        .as_ref()
        .and_then(|batch| batch.calls.first())
        .expect("deferred tool call")
        .assigned
        .effect_id;
    drop(owner);

    let owner = spawn_tool_owner(recovered, model.clone(), catalog.clone(), 2_499, 811)
        .await
        .expect("before due");
    assert_eq!(toolset.reconcile_count(), 0);
    drop(owner);

    let recovered = recover_session(&store).await;
    let owner = spawn_tool_owner(recovered, model.clone(), catalog.clone(), 2_500, 812)
        .await
        .expect("first due poll");
    wait_state(&store, |_| toolset.reconcile_count() == 1).await;
    drop(owner);

    let recovered = recover_session(&store).await;
    let owner = spawn_tool_owner(recovered, model.clone(), catalog.clone(), 2_600, 813)
        .await
        .expect("second due poll");
    wait_state(&store, |_| toolset.reconcile_count() == 2).await;
    drop(owner);

    let recovered = recover_session(&store).await;
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_700, 814)
        .await
        .expect("completion poll");
    wait_state(&store, |state| state.tool_settlements.contains_key(&effect_id)).await;
    assert_eq!(toolset.reconcile_count(), 3);
    owner.shutdown().await;
}

#[tokio::test]
async fn deferred_poll_rearms_live_wait_after_still_running() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", 2_000, None),
        )))],
    };
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![plan],
        vec![
            ToolReconcileResult::StillRunning(polling_deferral("job-1", 2_600, None)),
            ToolReconcileResult::Completed(tool_result(2)),
        ],
        spec,
    );
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        815,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_state(&store, |_| toolset.reconcile_count() == 1).await;
    assert!(
        tokio::time::timeout(StdDuration::from_millis(100), async {
            while toolset.reconcile_count() == 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err(),
        "the process-local wait must defer the second reconciliation"
    );
    drop(owner);

    let recovered = recover_session(&store).await;
    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_600, 816)
        .await
        .expect("later poll");
    let settled = wait_state(&store, |state| !state.tool_settlements.is_empty()).await;
    assert_eq!(toolset.reconcile_count(), 2);
    assert_eq!(settled.state().tool_settlements.len(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn deferred_poll_expiry_wakes_before_later_live_wait() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", 2_000, Some(2_500)),
        )))],
    };
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![plan],
        vec![ToolReconcileResult::StillRunning(polling_deferral(
            "job-1", 2_600, None,
        ))],
        spec,
    );
    let clock = ManualClock::new(timestamp(2_000));
    let mut owner = spawn_tool_owner_with_clock(
        CommitCoordinator::new(store.clone()),
        model,
        catalog,
        clock.clone(),
        817,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_state(&store, |_| toolset.reconcile_count() == 1).await;
    clock.set(timestamp(2_500)).expect("advance clock");
    tokio::time::sleep(StdDuration::from_millis(500)).await;

    let mut error_code = None;
    for _ in 0..100 {
        tokio::task::yield_now().await;
        let loaded = store
            .load(LoadRequest {
                session_id: id::<SessionTag>(1),
            })
            .await
            .expect("load");
        error_code = loaded
            .committed_batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .find_map(|record| match record.body() {
                RecordBody::EffectFailed(failure) => Some(failure.error().code.as_str().to_owned()),
                _ => None,
            });
        if error_code.is_some() {
            break;
        }
    }

    assert_eq!(toolset.reconcile_count(), 1);
    assert_eq!(error_code.as_deref(), Some(TOOL_DEFERRAL_EXPIRED));
    owner.shutdown().await;
}

#[tokio::test]
async fn deferred_poll_without_next_deadline_waits_externally() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", 2_000, None),
        )))],
    };
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![plan],
        vec![
            ToolReconcileResult::StillRunning(externally_waiting_deferral("job-1")),
            ToolReconcileResult::Completed(tool_result(2)),
        ],
        spec,
    );
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model,
        catalog,
        2_000,
        818,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_state(&store, |_| toolset.reconcile_count() == 1).await;
    assert!(
        tokio::time::timeout(StdDuration::from_millis(100), async {
            while toolset.reconcile_count() == 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err(),
        "an undated reconciliation must wait for an external completion"
    );
    drop(owner);
}

#[tokio::test]
async fn deferred_poll_uncertain_reconciliation_faults_with_tool_code() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", 2_000, None),
        )))],
    };
    let (store, _toolset, model, catalog, tools) =
        resume_ports(1, vec![plan], vec![ToolReconcileResult::Unknown], spec);
    let mut owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model,
        catalog,
        2_000,
        819,
    )
    .await
    .expect("owner");
    let mut status = owner.handle().observe_status();

    drive_to_tools(&owner.handle(), &store, tools).await;
    tokio::time::timeout(StdDuration::from_secs(2), status.changed())
        .await
        .expect("fault status")
        .expect("status sender");
    assert_eq!(
        *status.borrow(),
        RunStatus::Faulted {
            code: TOOL_RECONCILIATION_UNSUPPORTED,
        }
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn deferred_polls_keep_later_live_waits_per_effect() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let first = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", 2_000, None),
        )))],
    };
    let second = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![
            ScriptedToolAction::Block(Arc::from("second-deferral")),
            ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(polling_deferral(
                "job-2", 2_000, None,
            )))),
        ],
    };
    let (store, toolset, model, catalog, tools) = resume_ports(
        2,
        vec![first, second],
        vec![
            ToolReconcileResult::StillRunning(polling_deferral("job-1", 2_600, None)),
            ToolReconcileResult::Completed(tool_result(2)),
        ],
        spec,
    );
    let control = toolset.control();
    let mut owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model,
        catalog,
        2_000,
        817,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    wait_gate(&control, "second-deferral").await;
    let recovered = wait_state(&store, |_| toolset.reconcile_count() == 1).await;
    let second_effect_id = recovered
        .state()
        .active_tool_batch
        .as_ref()
        .and_then(|batch| batch.calls.get(1))
        .expect("second deferred tool call")
        .assigned
        .effect_id;

    control.release("second-deferral");
    wait_state(&store, |state| {
        state.tool_settlements.contains_key(&second_effect_id)
    })
    .await;
    assert_eq!(toolset.reconcile_count(), 2);
    owner.shutdown().await;
}

#[tokio::test]
async fn deferred_poll_capacity_one_drains_concurrent_terminal_schedules() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plans = ["job-1", "job-2", "job-3"]
        .into_iter()
        .map(|handle| ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
                polling_deferral(handle, 2_000, None),
            )))],
        })
        .collect();
    let (store, toolset, model, catalog, tools) = resume_ports(
        3,
        plans,
        vec![
            ToolReconcileResult::StillRunning(polling_deferral("job-1", 2_600, None)),
            ToolReconcileResult::Completed(tool_result(2)),
            ToolReconcileResult::Completed(tool_result(3)),
        ],
        spec,
    );
    let mut run_config = owner_run_config();
    run_config.command_capacity = 1;
    let mut owner = spawn_tool_owner_with_run_config(
        CommitCoordinator::new(store.clone()),
        model,
        catalog,
        FixedClock::new(timestamp(2_000)),
        822,
        run_config,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    let settled = wait_state(&store, |state| state.tool_settlements.len() == 2).await;

    assert_eq!(toolset.reconcile_count(), 3);
    assert_eq!(settled.state().phase, Some(RunPhase::AwaitingExternal));
    owner.shutdown().await;
}

#[tokio::test]
async fn deferred_poll_expiry_fails_without_reconciliation() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            polling_deferral("job-1", 2_500, Some(2_600)),
        )))],
    };
    let (store, toolset, model, catalog, tools) = resume_ports(
        1,
        vec![plan],
        vec![ToolReconcileResult::Completed(tool_result(3))],
        spec,
    );
    let owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model.clone(),
        catalog.clone(),
        2_000,
        820,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    let recovered = wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);

    let mut owner = spawn_tool_owner(recovered, model, catalog, 2_600, 821)
        .await
        .expect("expired deferral");
    let error_code = tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let loaded = store
                .load(LoadRequest {
                    session_id: id::<SessionTag>(1),
                })
                .await
                .expect("load");
            if let Some(code) = loaded
                .committed_batches
                .iter()
                .flat_map(|batch| batch.records.iter())
                .find_map(|record| match record.body() {
                    RecordBody::EffectFailed(failure) => Some(failure.error().code.as_str()),
                    _ => None,
                })
            {
                return code.to_owned();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("effect failure");
    assert_eq!(toolset.reconcile_count(), 0);
    assert_eq!(error_code, TOOL_DEFERRAL_EXPIRED);
    owner.shutdown().await;
}
