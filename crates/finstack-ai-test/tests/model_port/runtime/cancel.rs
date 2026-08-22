#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the cancellation acceptance keeps exact committed identities and signalling visible"
)]
async fn runtime_routes_cancel_effect_to_only_the_active_model_task() {
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
            actions: vec![ScriptedModelAction::Block(Arc::from("runtime-slow"))],
        }],
    ));
    let control = model.control();
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
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        Arc::new(
            finstack_ai_runtime::ports::model::ReadyModel::prepare(model_port)
                .await
                .expect("model readiness"),
        ),
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(100)),
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
    handle
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
        .expect("model request");
    while control.entries("runtime-slow") == 0 {
        tokio::task::yield_now().await;
    }

    let cancel_env = TransitionEnv {
        now: timestamp(1_400),
        ids: AllocatedIds::try_new(
            vec![id(7)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(105)],
            vec![id::<CancellationRequestTag>(700)],
        )
        .expect("cancel ids"),
    };
    handle
        .submit(
            cancel_env,
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("test cancellation")),
            }),
        )
        .await
        .expect("cancel");
    while model.cancellation_acknowledgement_count() == 0 {
        tokio::task::yield_now().await;
    }
    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover cancellation reconciliation");
        if recovered.state().phase == Some(RunPhase::Cancelled) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.cancellation_acknowledgement_count(), 1);
    let report = owner.shutdown().await;
    assert_eq!(report.outcome, ShutdownOutcome::Graceful);
    assert_eq!(handle.status(), RunStatus::Stopped);
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
    let recovered = CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover cancelled run");
    assert!(recovered.state().messages.is_empty());
    assert!(recovered.state().completion_identities.is_empty());
}
