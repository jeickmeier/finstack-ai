use finstack_ai_kernel::{ComponentId, ExternalHandleRef, ReconciliationPolicy};
use finstack_ai_runtime::{ToolDeferral, TOOL_DEFERRAL_EXPIRED};

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
