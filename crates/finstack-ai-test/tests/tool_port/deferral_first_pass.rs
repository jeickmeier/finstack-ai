#[tokio::test]
async fn deferred_tool_first_pass_preserves_requested_effect() {
    let mut spec = tool_spec("echo");
    spec.deferral = ToolDeferralSupport::Supported;
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
            scripted_tool_deferral("job-1"),
        )))],
    };
    let (store, _, model, catalog, tools) =
        resume_ports(1, vec![plan], Vec::new(), spec);
    let mut owner = spawn_tool_owner(
        CommitCoordinator::new(store.clone()),
        model,
        catalog,
        2_000,
        810,
    )
    .await
    .expect("owner");

    drive_to_tools(&owner.handle(), &store, tools).await;
    let mut status = owner.handle().observe_status();
    let recovered = tokio::select! {
        recovered = wait_state(&store, |state| {
            state.phase == Some(RunPhase::AwaitingExternal)
        }) => recovered,
        changed = status.changed() => {
            changed.expect("owner status");
            panic!("owner stopped before deferral committed: {:?}", *status.borrow());
        }
    };
    assert_eq!(recovered.state().phase, Some(RunPhase::AwaitingExternal));
    let batch = recovered
        .state()
        .active_tool_batch
        .as_ref()
        .expect("active tool batch");
    let call = batch.calls.first().expect("deferred tool call");
    let effect_id = call.assigned.effect_id;
    assert!(matches!(
        &call.status,
        ActiveToolCallStatus::Requested {
            deferred: Some(deferred),
            ..
        } if deferred.effect_id == effect_id
    ));
    assert!(!recovered.state().tool_settlements.contains_key(&effect_id));

    owner.shutdown().await;
}
