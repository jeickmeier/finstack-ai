#[tokio::test]
async fn equal_duplicate_completion_is_idempotent() {
    let (_agent, parent, _store, effect_id) = deferred_tool_parent().await;
    let command = failed_command(
        &parent,
        effect_id,
        "ext-equal-1",
        security().principal().clone(),
    );
    let first = Box::pin(parent.complete_external(command.clone()))
        .await
        .expect("first completion");
    assert!(
        matches!(first, ExternalRouteOutcome::Committed(_)),
        "first completion must commit: {first:?}"
    );
    let second = Box::pin(parent.complete_external(command))
        .await
        .expect("equal duplicate");
    assert!(
        matches!(second, ExternalRouteOutcome::Idempotent { .. }),
        "equal duplicate must be idempotent: {second:?}"
    );
}

#[tokio::test]
async fn conflicting_duplicate_completion_fails_closed() {
    let (_agent, parent, _store, effect_id) = deferred_tool_parent().await;
    let first = failed_command(
        &parent,
        effect_id,
        "ext-conflict-1",
        security().principal().clone(),
    );
    let second = failed_command(
        &parent,
        effect_id,
        "ext-conflict-2",
        security().principal().clone(),
    );
    Box::pin(parent.complete_external(first))
        .await
        .expect("first completion");
    let outcome = Box::pin(parent.complete_external(second))
        .await
        .expect("conflicting completion routes");
    assert!(
        matches!(outcome, ExternalRouteOutcome::Rejected { .. }),
        "conflicting duplicate must fail closed: {outcome:?}"
    );
}

#[tokio::test]
async fn unauthorized_principal_is_rejected() {
    let (_agent, parent, _store, effect_id) = deferred_tool_parent().await;
    let stranger = PrincipalRef::try_new("preview-tests", "stranger", Some("tenant-preview"))
        .expect("stranger");
    let command = failed_command(&parent, effect_id, "ext-unauth-1", stranger);
    let error = Box::pin(parent.complete_external(command))
        .await
        .expect_err("unauthorized principal");
    assert_eq!(error.code(), finstack_ai::AGENT_RUN_RUNTIME_FAILURE);
}
