#[tokio::test]
async fn prefix_i1_through_i4() {
    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase(), Some(RunPhase::AwaitingInteraction));
    assert_legal("I1", recovered.state().phase(), LegalRestore::Suspended);

    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let mut coordinator = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            resolve_env(2_300),
            KernelInput::InteractionSettled(InteractionSettled::Resolved(
                InteractionResolution::try_new(
                    id::<InteractionTag>(501),
                    "resolution-1",
                    PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                        .expect("principal"),
                    AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
                    RawJson::parse(r#"{"approved":true}"#).expect("response"),
                    None::<&str>,
                )
                .expect("resolution"),
            )),
        )
        .await
        .expect("resolve");
    let identities = coordinator.state().resolution_identities().clone();
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().resolution_identities(), &identities);
    assert_legal("I2", recovered.state().phase(), LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let mut coordinator = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            resolve_env(2_400),
            KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
                interaction_id: id::<InteractionTag>(501),
                expired_at: timestamp(2_400),
            })),
        )
        .await
        .expect("expire");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(recovered.state().last_interaction_terminal().is_some());
    assert_legal("I3", recovered.state().phase(), LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let mut coordinator = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            cancel_env(2_500, 90, 190, 700),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("i4")),
            }),
        )
        .await
        .expect("cancel");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal("I4", recovered.state().phase(), LegalRestore::Cancelled);
}
