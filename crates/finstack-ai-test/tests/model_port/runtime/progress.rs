#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the proof keeps live progress, lag closure, finalization, and journal recovery contiguous"
)]
async fn runtime_publishes_validated_progress_before_terminal_settlement() {
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
                    text: Arc::from("early progress"),
                }))),
                ScriptedModelAction::Block(Arc::from("terminal-gate")),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(
                    "early progress",
                )))),
            ],
        }],
    ));
    let control = model.control();
    let model_port: Arc<dyn Model> = model.clone();
    let mut owner = RunTaskOwner::spawn_with_model(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 4,
            event_hub: EventHubConfig {
                source_capacity: 4,
                max_subscribers: 2,
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
        CounterRandom(AtomicU64::new(500)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    let mut subscription = handle
        .subscribe_events(EventSubscriptionConfig {
            queue_capacity: 2,
            filter: EventFilter {
                include_durable: false,
                include_transient: true,
                kinds: Arc::from([RunEventKind::ModelTextDelta]),
                max_sensitivity: Sensitivity::Confidential,
            },
            batching: EventBatchConfig {
                flush_count: 8,
                flush_bytes: 64 * 1_024,
                flush_interval: StdDuration::from_secs(1),
            },
            progress_coalescing: ProgressCoalescing::Disabled,
            lag_policy: EventLagPolicy::BlockBounded {
                timeout: StdDuration::from_millis(100),
            },
        })
        .await
        .expect("subscription");
    let mut stalled_durable = handle
        .subscribe_events(EventSubscriptionConfig {
            queue_capacity: 1,
            filter: EventFilter {
                include_durable: true,
                include_transient: false,
                kinds: Arc::from([]),
                max_sensitivity: Sensitivity::Confidential,
            },
            batching: EventBatchConfig {
                flush_count: 1,
                flush_bytes: 64 * 1_024,
                flush_interval: StdDuration::from_secs(1),
            },
            progress_coalescing: ProgressCoalescing::Enabled,
            lag_policy: EventLagPolicy::DropProgress {
                durable_timeout: StdDuration::from_millis(1),
            },
        })
        .await
        .expect("stalled durable subscription");

    drive_to_active_model_request(&handle).await;
    while control.entries("terminal-gate") == 0 {
        tokio::task::yield_now().await;
    }
    let batch = subscription.next_batch().await.expect("progress batch");
    assert_eq!(batch.events().len(), 1);
    let RunEventBody::ModelTextDelta(delta) = batch.events()[0].body() else {
        panic!("expected model text progress");
    };
    assert_eq!(delta.text(), "early progress");
    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover before terminal");
    assert!(recovered.state().pending_model_effect.is_some());
    assert!(recovered.state().model_settlements.is_empty());

    control.release("terminal-gate");
    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover after terminal");
        if recovered.state().model_settlements.len() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    handle
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    handle
        .submit(
            env(2_200, &[8, 9], &[3], &[], &[], &[], &[], 106),
            stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        )
        .await
        .expect("finalize");
    let recovered = CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover completed run");
    assert!(recovered.state().terminal.is_some());
    assert_eq!(
        stalled_durable.status().close_reason,
        Some(EventSubscriptionCloseReason::MissedDurable)
    );
    let delivered_before_lag = stalled_durable
        .next_batch()
        .await
        .expect("first durable batch");
    assert!(
        !matches!(
            delivered_before_lag
                .events()
                .last()
                .map(finstack_ai_kernel::RunEvent::kind),
            Some(RunEventKind::RunCompleted | RunEventKind::RunFailed | RunEventKind::RunCancelled)
        ),
        "the terminal event was not delivered to the disconnected subscriber"
    );
    assert!(stalled_durable.next_batch().await.is_none());
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}
