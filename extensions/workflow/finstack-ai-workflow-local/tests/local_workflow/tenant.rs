#[tokio::test]
async fn cross_tenant_locator_is_unknown() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        900,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    drop(owner);
    let driver = attach_driver(store, model, clock, 900).await;
    let foreign = OperationLocator::try_new("tenant-b", id(1), id(2), id(3)).expect("foreign");
    let command = ExternalEffectCompletionCommand::try_new(
        foreign,
        PrincipalRef::try_new("issuer", "subject", Some("tenant-b")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            id(103),
            "ext-x",
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "denied",
                    "denied",
                    finstack_ai_kernel::ErrorCategory::Validation,
                    false,
                )
                .expect("error"),
            },
        )
        .expect("completion"),
    )
    .expect("command");
    let error = Box::pin(driver.complete_external(command, timestamp(3_000)))
        .await
        .expect_err("cross-tenant");
    assert_eq!(error.code(), "unknown_locator");
}
