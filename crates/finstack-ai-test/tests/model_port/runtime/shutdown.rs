#[tokio::test]
async fn shutdown_aborts_an_uncooperative_model_only_after_its_grace_deadline() {
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
            actions: vec![ScriptedModelAction::BlockUninterruptibly(Arc::from(
                "forced-abort",
            ))],
        }],
    ));
    let control = model.control();
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(10),
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
        CounterRandom(AtomicU64::new(300)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;
    while control.entries("forced-abort") == 0 {
        tokio::task::yield_now().await;
    }

    let report = owner.shutdown().await;

    assert_eq!(handle.status(), RunStatus::Stopped);
    assert_eq!(report.outcome, ShutdownOutcome::Forced);
    assert!(report.aborted_tasks > 0);
    assert_eq!(model.cancellation_acknowledgement_count(), 0);
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
}
