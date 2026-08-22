#[tokio::test]
async fn repeated_model_runs_settle_and_shutdown_without_stream_or_task_leaks() {
    for ordinal in 0..24_u64 {
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
            vec![chunks(1, "stress completion")],
        ));
        let model_port: Arc<dyn Model> = model.clone();
        let mut owner = RunTaskOwner::spawn_with_model(
            CommitCoordinator::new(store.clone()),
            RunTaskConfig {
                command_capacity: 2,
                event_hub: EventHubConfig {
                    source_capacity: 4,
                    max_subscribers: 2,
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
            CounterRandom(AtomicU64::new(10_000 + ordinal)),
        )
        .await
        .expect("owner");
        let handle = owner.handle();
        drive_to_active_model_request(&handle).await;
        tokio::time::timeout(StdDuration::from_secs(1), async {
            loop {
                let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
                    .await
                    .expect("recover");
                if recovered.state().model_settlements.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("model settlement timeout");

        let report = owner.shutdown().await;

        assert_eq!(report.outcome, ShutdownOutcome::Graceful);
        assert_eq!(report.aborted_tasks, 0);
        assert_eq!(handle.status(), RunStatus::Stopped);
        assert_eq!(model.request_count(), 1);
        assert_eq!(model.active_stream_count(), 0);
    }
}
