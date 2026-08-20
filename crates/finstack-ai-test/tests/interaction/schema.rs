async fn park_with_schema(
    schema: RawJson,
) -> (ApprovalPorts, RunTaskOwner, InteractionId) {
    let ports = approval_ports();
    let owner = spawn_owner(
        CommitCoordinator::new(ports.store.clone()),
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_500,
        720,
    )
    .await
    .expect("owner");
    drive_to_after_model(
        &owner.handle(),
        &ports.store,
        Arc::clone(&ports.tools),
        None,
    )
    .await;
    owner
        .handle()
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: request_with_schema(schema),
            }),
        )
        .await
        .expect("request");
    wait_phase(&ports.store, RunPhase::AwaitingInteraction).await;
    (ports, owner, id::<InteractionTag>(501))
}

#[tokio::test]
async fn router_rejects_numeric_answer_for_free_text_schema() {
    let (ports, owner, interaction_id) = park_with_schema(free_text_schema()).await;
    drop(owner);
    let error = router(ports.store.clone())
        .await
        .route(
            resolve_command_with_response(
                interaction_id,
                RawJson::parse(r#"{"answer":1}"#).expect("response"),
            ),
            timestamp(2_600),
        )
        .await
        .expect_err("numeric free-text answer must not commit");
    assert_eq!(error, finstack_ai_runtime::ExternalRouteError::InvalidNormalizedCommand);
    let kinds = record_kinds(&ports.store).await;
    assert!(
        !kinds.contains(&"interaction_resolved"),
        "invalid resolve must not commit: {kinds:?}"
    );
}

#[tokio::test]
async fn router_accepts_string_answer_for_free_text_schema() {
    let (ports, owner, interaction_id) = park_with_schema(free_text_schema()).await;
    drop(owner);
    router(ports.store.clone())
        .await
        .route(
            resolve_command_with_response(
                interaction_id,
                RawJson::parse(r#"{"answer":"250k USD"}"#).expect("response"),
            ),
            timestamp(2_600),
        )
        .await
        .expect("string free-text answer");
    let kinds = record_kinds(&ports.store).await;
    assert!(kinds.contains(&"interaction_resolved"), "{kinds:?}");
}
