#[test]
fn enumerated_prefix_matrix_lists_every_id() {
    assert_eq!(PREFIX_IDS.len(), 46);
    let mut unique = PREFIX_IDS.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), PREFIX_IDS.len());
}

#[test]
fn crash_prefix_catalog_covers_every_effect_kind_and_lane_operation() {
    let kinds = [
        (EffectKind::Model, "W2"),
        (EffectKind::Tool, "T1"),
        (EffectKind::Context, "X1"),
        (EffectKind::Middleware, "M1"),
        (EffectKind::Interaction, "I1"),
        (EffectKind::Timer, "R1"),
    ];
    for (kind, prefix) in kinds {
        assert!(
            PREFIX_IDS.contains(&prefix),
            "{kind:?} catalog prefix {prefix} is missing"
        );
    }
    for operation in ["P1", "P2", "P3", "P4"] {
        assert!(
            PREFIX_IDS.contains(&operation),
            "lane operation prefix {operation} is missing"
        );
    }
    for class in [
        LegalRestore::Retryable,
        LegalRestore::Suspended,
        LegalRestore::Cancelled,
        LegalRestore::Completed,
        LegalRestore::Failed,
    ] {
        let _ = class;
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "PR-064 catalog adds one recover prefix per EffectKind and lane operation"
)]
async fn prefix_effect_kind_and_lane_catalog() {
    let store = memory_store();
    let mut coordinator = drive_to_pending_model(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending")
        .clone();
    let tool_call = ToolCallBlock::try_new(id(301), "alpha", RawJson::parse("{}").expect("args"))
        .expect("tool call");
    let assistant = Message::try_new(
        id(617),
        MessageRole::Assistant,
        vec![
            ContentBlock::Text(TextBlock::try_new("calling").expect("text")),
            ContentBlock::ToolCall(tool_call.clone()),
        ],
        timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant");
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        output_contract(),
        RawJson::parse(r#"{"text":"calling"}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("cmpl-tool"),
        None,
    )
    .expect("completed");
    coordinator
        .submit(
            env_tools(
                1_400,
                &[607, 608],
                &[603, 604],
                &[],
                &[617],
                &[],
                &[301],
                605,
            ),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message: assistant,
                },
            }),
        )
        .await
        .expect("settle tools");
    coordinator
        .submit(
            env(1_500, &[609], &[], &[], &[], &[], &[], 606),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model tools");
    assert_eq!(coordinator.state().phase, Some(RunPhase::BeforeToolBatch));
    coordinator
        .submit(
            env_tools(
                1_600,
                &[1_000, 1_001, 1_002],
                &[1_000],
                &[401],
                &[],
                &[400],
                &[],
                700,
            ),
            stage(
                Stage::BeforeToolBatch,
                ReducerStageOutcome::ToolBatchPrepared {
                    calls: Arc::from([ToolCallPlan::Execute(ValidatedToolCall {
                        call: tool_call,
                        tool_id: ToolId::parse("finstack.tools.fixture").expect("tool"),
                        component: None,
                        output_contract: EffectOutputContract {
                            kind: EffectOutputKind::ToolResult,
                            schema_version: 1,
                            schema_digest: Digest::raw_json(br#"{"type":"tool_result"}"#),
                        },
                        retry_safety: RetrySafety::IdempotentWithKey,
                        deadline: None,
                        execution: ToolExecutionMode::Sequential,
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                    })]),
                    continuation: ToolBatchContinuation::Finalize,
                },
            ),
        )
        .await
        .expect("tool batch");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::AwaitingTools));
    assert!(
        recovered
            .state()
            .active_tool_batch
            .as_ref()
            .is_some_and(|batch| batch.calls.iter().any(|call| call
                .assigned
                .plan
                .call()
                .tool_name()
                == "alpha")),
        "T1 journal must retain the tool effect"
    );
    assert_legal("T1", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            env(1_500, &[9], &[], &[], &[], &[], &[], 106),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    coordinator
        .submit(
            env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 107),
            stage(
                Stage::BeforeFinalize,
                ReducerStageOutcome::Fail(
                    ErrorDescriptor::new(
                        "model_failed",
                        "model failed",
                        ErrorCategory::Model,
                        false,
                    )
                    .expect("error"),
                ),
            ),
        )
        .await
        .expect("fail");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal("K1", recovered.state().phase, LegalRestore::Failed);

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
            env(1_400, &[607], &[603], &[], &[], &[], &[], 605),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Failed(
                    EffectFailed::try_new(
                        pending.requested.effect_id(),
                        output_contract(),
                        ErrorDescriptor::new(
                            "provider_retry",
                            "retryable provider failure",
                            ErrorCategory::Model,
                            true,
                        )
                        .expect("error"),
                        None,
                        Some("provider-retry"),
                    )
                    .expect("failed"),
                ),
            }),
        )
        .await
        .expect("settle failed");
    coordinator
        .submit(
            env(1_700, &[11, 12, 13], &[6], &[501], &[], &[], &[], 108),
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
        .expect("retry");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::Sleeping));
    assert!(
        recovered
            .state()
            .retry
            .pending
            .as_ref()
            .is_some_and(|pending| pending.timer_effect_id == id(501)),
        "R1 journal must retain the timer effect"
    );
    assert_legal("R1", recovered.state().phase, LegalRestore::Suspended);

    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recovered.state().phase,
        Some(RunPhase::PreparingContext),
        "X1 is the PrepareContext crash window for EffectKind::Context"
    );
    assert_legal("X1", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recovered.state().phase,
        Some(RunPhase::BeforeRun),
        "M1 is the BeforeRun middleware crash window for EffectKind::Middleware"
    );
    assert_legal("M1", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let session = SessionRuntime::create(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        "tenant-a",
        SessionCreateIds {
            session_id: id(1),
            main_lane_id: id(2),
            session_created_record_id: id(101),
            lane_created_record_id: id(102),
            batch_id: id(100),
            now: timestamp(1),
        },
    )
    .await
    .expect("session");
    session
        .create_lane(
            "research",
            None,
            LaneCreateIds {
                lane_id: id(50),
                lane_created_record_id: id(201),
                lane_moved_record_id: None,
                batch_id: id(200),
                now: timestamp(2),
            },
        )
        .await
        .expect("P1 create");
    session.try_acquire_run(id(2), id(3)).expect("P2 acquire");
    assert_eq!(
        session.try_acquire_run(id(2), id(30)),
        Err(SessionError::LaneBusy),
        "P2 reject-while-busy"
    );
    session
        .try_acquire_run(id(50), id(4))
        .expect("P3 sibling acquire");
    let main = session
        .accept_run(id(2), acceptance(3), simple_env(1_000, 10, 11, 12))
        .await
        .expect("accept main");
    let sibling = session
        .accept_run(id(50), acceptance(4), simple_env(1_100, 20, 21, 22))
        .await
        .expect("accept sibling");
    assert_eq!(main.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(sibling.state().phase, Some(RunPhase::BeforeRun));
    assert!(
        sibling.state().cancellation.is_none(),
        "P2 does not cancel sibling"
    );
    drop(main);
    drop(sibling);
    drop(session);
    let restored = SessionRuntime::open(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id(1),
        "tenant-a",
    )
    .await
    .expect("P4 restore");
    assert!(
        restored
            .inspect(id(50))
            .await
            .expect("research")
            .history
            .is_empty()
            || restored
                .projection()
                .expect("projection")
                .lane_by_id(id(50))
                .is_some(),
        "P1 created lane survives restore"
    );
    let main = restored
        .coordinator_for_run(Some(id(3)))
        .await
        .expect("main restore");
    let sibling = restored
        .coordinator_for_run(Some(id(4)))
        .await
        .expect("sibling restore");
    assert_legal("P1", main.state().phase, LegalRestore::Retryable);
    assert_legal("P2", main.state().phase, LegalRestore::Retryable);
    assert_legal("P3", sibling.state().phase, LegalRestore::Retryable);
    assert_legal("P4", sibling.state().phase, LegalRestore::Retryable);
}
