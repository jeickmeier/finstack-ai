#[tokio::test]
async fn failed_model_request_append_never_executes_the_model() {
    let store = Arc::new(FailFourthAppendStore::new());
    let model = Arc::new(ScriptedModel::from_inputs(
        profile(),
        vec![chunks(1, "must not execute")],
    ));
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
            approval_grant: ApprovalGrantMode::PerCall,
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(200)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    let error = handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: raw,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(5_000)),
                },
            ),
        )
        .await
        .expect_err("append failure");
    assert!(matches!(
        error,
        RunHandleError::Coordinator(CommitCoordinatorError::Store(StoreError::Unavailable {
            reason_code: "model_request_append_failed"
        }))
    ));
    assert_eq!(model.warmup_count(), 1);
    assert_eq!(model.request_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn malformed_stream_settles_as_failure_without_partial_durable_success() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("streamed"),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("different")))),
            ],
        }],
    ));
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
            approval_grant: ApprovalGrantMode::PerCall,
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(250)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;

    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().model_settlements.len() == 1 {
            assert!(recovered.state().pending_model_effect.is_none());
            assert!(recovered.state().messages.is_empty());
            assert!(recovered.state().completion_identities.is_empty());
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.active_stream_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn an_expired_committed_deadline_prevents_provider_execution() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let model = Arc::new(ScriptedModel::from_inputs(
        profile(),
        vec![chunks(1, "too late")],
    ));
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
            approval_grant: ApprovalGrantMode::PerCall,
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        model_port,
        locked_profile(),
        FixedClock::new(timestamp(6_000)),
        CounterRandom(AtomicU64::new(275)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;

    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().model_settlements.len() == 1 {
            assert!(recovered.state().pending_model_effect.is_none());
            assert!(recovered.state().messages.is_empty());
            assert!(recovered.state().completion_identities.is_empty());
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(model.request_count(), 0);
    assert_eq!(model.active_stream_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}
