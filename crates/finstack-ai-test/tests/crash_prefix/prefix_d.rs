#[tokio::test]
async fn prefix_d1_through_d6_and_post_horizon() {
    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut coordinator).await;
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(recovered.state().pending_model_effect.is_some());
    assert_legal("D1", recovered.state().phase, LegalRestore::Retryable);
    assert_legal("D2", recovered.state().phase, LegalRestore::Retryable);
    assert_legal("D3", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = drive_to_pending_model(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending")
        .clone();
    coordinator
        .submit(
            env(1_400, &[7], &[3], &[], &[], &[], &[], 105),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Deferred(EffectDeferred {
                    effect_id: pending.requested.effect_id(),
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.provider.demo").expect("component"),
                        "h1",
                        RawJson::parse("{}").expect("meta"),
                    )
                    .expect("handle"),
                    reconciliation: ReconciliationPolicy::CallbackOnly,
                    next_poll_at: None,
                    expires_at: None,
                    output_contract: output_contract(),
                }),
            }),
        )
        .await
        .expect("defer");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::AwaitingExternal));
    assert!(
        recovered
            .state()
            .pending_model_effect
            .as_ref()
            .expect("pending")
            .deferred
            .is_some()
    );
    assert_legal("D4", recovered.state().phase, LegalRestore::Suspended);

    let mut recovered = recovered;
    let failed = finstack_ai_kernel::ErrorDescriptor::new(
        "provider_failed",
        "provider failed",
        finstack_ai_kernel::ErrorCategory::Model,
        false,
    )
    .expect("error");
    let completion = ExternalEffectCompletion::try_new(
        id::<finstack_ai_kernel::EffectTag>(103),
        "ext-1",
        ExternalEffectOutcome::Failed {
            error: failed.clone(),
        },
    )
    .expect("completion");
    recovered
        .submit(
            env(1_500, &[8], &[4], &[], &[], &[], &[], 106),
            KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
                completion: completion.clone(),
                assistant_message: None,
            }),
        )
        .await
        .expect("complete");
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let again = recovered
        .submit(
            env(1_600, &[10], &[6], &[], &[], &[], &[], 107),
            KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
                completion,
                assistant_message: None,
            }),
        )
        .await
        .expect("equal replay");
    assert!(again.committed.is_none(), "D5 equal replay is idempotent");
    assert_legal(
        "D5",
        recovered.state().phase,
        classify_phase(recovered.state().phase.expect("phase")),
    );

    let conflicting = ExternalEffectCompletion::try_new(
        id::<finstack_ai_kernel::EffectTag>(103),
        "ext-2",
        ExternalEffectOutcome::Failed { error: failed },
    )
    .expect("conflict");
    assert!(
        recovered
            .submit(
                env(1_700, &[12], &[8], &[], &[], &[], &[], 108),
                KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
                    completion: conflicting,
                    assistant_message: None,
                }),
            )
            .await
            .is_err(),
        "D6 conflicting duplicate fails closed"
    );
    let unchanged = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        unchanged.state().completion_identities,
        recovered.state().completion_identities
    );

    let sink = Arc::new(RecordingSink::default());
    let gate = SecurityAuditGate::enable(Some(Arc::clone(&sink) as _), Duration::from_millis(100))
        .await
        .expect("gate");
    let router = ExternalCompletionRouter::new(Arc::clone(&store) as Arc<dyn JournalStore>, gate)
        .with_horizon(IdempotencyHorizon {
            expire_at: timestamp(1),
        });
    let late = ExternalEffectCompletionCommand::try_new(
        OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator"),
        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            id(999),
            "late-1",
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    finstack_ai_kernel::ErrorCategory::Model,
                    false,
                )
                .expect("error"),
            },
        )
        .expect("late"),
    )
    .expect("command");
    assert_eq!(
        router
            .route(late, timestamp(2_000))
            .await
            .expect_err("expired"),
        ExternalRouteError::IngressRejected
    );
    assert!(
        sink.events
            .lock()
            .expect("lock")
            .iter()
            .any(
                |event| event.category() == SecurityAuditCategory::UnknownLocator
                    && event.reason_code() == "expired_locator"
            ),
        "post-horizon is expired_locator"
    );
}
