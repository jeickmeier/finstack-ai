#[derive(Debug, PartialEq, Eq)]
struct RuntimeProjection {
    record_bodies: Vec<Vec<u8>>,
    assistant_message: Vec<u8>,
    state_hash: Digest,
    warmups: usize,
    requests: usize,
    status: RunStatus,
}

#[expect(
    clippy::too_many_lines,
    reason = "the acceptance helper keeps the complete commit-before-dispatch lifecycle visible"
)]
async fn run_runtime_chunks(count: usize, response_text: &str) -> RuntimeProjection {
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
        vec![chunks(count, response_text)],
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
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        Arc::new(
            finstack_ai_runtime::ReadyModel::prepare(model_port)
                .await
                .expect("model readiness"),
        ),
        locked_profile(),
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(1)),
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
    let request_draft = draft(Arc::from([message]));
    let raw =
        RawJson::parse(request_draft.canonical_bytes().expect("canonical")).expect("raw request");
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

    while model.request_count() == 0 {
        tokio::task::yield_now().await;
    }
    let (assistant_message, state_hash) = loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().messages.len() == 1 {
            assert!(recovered.state().pending_model_effect.is_none());
            let message = &recovered.state().messages[0];
            assert_eq!(
                message
                    .content()
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.text()),
                        _ => None,
                    })
                    .collect::<String>(),
                response_text
            );
            break (
                serde_json::to_vec(message).expect("message bytes"),
                recovered.state().state_hash().expect("state hash"),
            );
        }
        tokio::task::yield_now().await;
    };
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("loaded");
    let record_bodies = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(|record| serde_json::to_vec(record.body()).expect("record body"))
        .collect();
    let warmups = model.warmup_count();
    let requests = model.request_count();
    owner.shutdown().await;
    RuntimeProjection {
        record_bodies,
        assistant_message,
        state_hash,
        warmups,
        requests,
        status: handle.status(),
    }
}

#[tokio::test]
async fn runtime_warms_reuses_and_settles_only_after_committed_dispatch() {
    let projection = run_runtime_chunks(10, "runtime response").await;
    assert_eq!(projection.warmups, 1);
    assert_eq!(projection.requests, 1);
    assert_eq!(projection.status, RunStatus::Stopped);
}

#[tokio::test]
async fn durable_runtime_outcome_is_identical_for_all_required_chunk_counts() {
    let response = "x".repeat(10_000);
    let baseline = run_runtime_chunks(1, &response).await;
    for count in [10, 100, 1_000] {
        assert_eq!(run_runtime_chunks(count, &response).await, baseline);
    }
}
