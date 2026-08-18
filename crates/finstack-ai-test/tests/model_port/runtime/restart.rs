#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the restart acceptance keeps the pre-crash and recovered owner states visible"
)]
async fn persisted_retry_timer_resumes_once_after_runtime_restart() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 32,
            records_per_session: 64,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let retryable = ModelError::try_new(
        "temporary_model_failure",
        ErrorCategory::Model,
        true,
        "temporary model failure",
        Metadata::empty(),
    )
    .expect("retryable error");
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable))],
        }],
    ));
    let model_port: Arc<dyn Model> = model;
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
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
        CounterRandom(AtomicU64::new(400)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_active_model_request(&handle).await;
    let settled = tokio::time::timeout(StdDuration::from_secs(1), async {
        loop {
            let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
                .await
                .expect("recover failure");
            if recovered.state().phase == Some(RunPhase::BeforeFinalize) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if settled.is_err() {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover diagnostic");
        panic!(
            "model failure did not settle; status={:?}; phase={:?}; settlements={}",
            handle.status(),
            recovered.state().phase,
            recovered.state().model_settlements.len()
        );
    }
    handle
        .submit(
            env(2_300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                ReducerStageOutcome::Retry(
                    RetryDirective::try_new(
                        RetryClassification::Model,
                        KernelDuration::from_millis(10_000),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    let sleeping = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover sleeping");
    assert_eq!(sleeping.state().phase, Some(RunPhase::Sleeping));
    assert_eq!(sleeping.state().retry.attempts, 1);
    assert_eq!(
        sleeping
            .state()
            .retry
            .pending
            .as_ref()
            .expect("timer")
            .attempt,
        1
    );
    tokio::time::timeout(StdDuration::from_secs(1), owner.shutdown())
        .await
        .expect("first owner shutdown");

    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover runtime");
    let replacement: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(profile(), Vec::new()));
    let mut replacement_owner = RunTaskOwner::spawn_with_model(
        recovered,
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(250),
        },
        ModelTaskConfig {
            job_capacity: 1,
            result_capacity: 1,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        replacement,
        locked_profile(),
        FixedClock::new(timestamp(20_000)),
        CounterRandom(AtomicU64::new(500)),
    )
    .await
    .expect("replacement owner");
    tokio::time::timeout(StdDuration::from_secs(1), async {
        loop {
            let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
                .await
                .expect("recover fired timer");
            if recovered.state().phase == Some(RunPhase::PreparingContext) {
                assert_eq!(recovered.state().retry.attempts, 1);
                assert!(recovered.state().retry.pending.is_none());
                assert_eq!(recovered.state().retry.timer_firings.len(), 1);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timer did not fire; status={:?}",
            replacement_owner.handle().status()
        )
    });
    assert_eq!(
        replacement_owner.handle().timer_diagnostics().already_due,
        1
    );
    tokio::time::timeout(StdDuration::from_secs(1), replacement_owner.shutdown())
        .await
        .expect("replacement owner shutdown");
}
