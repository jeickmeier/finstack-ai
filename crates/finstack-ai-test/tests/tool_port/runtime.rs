#[tokio::test]
async fn runtime_publishes_tool_progress_before_tool_settlement() {
    let completed = ToolResult {
        output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("output"),
        is_error: false,
    };
    let plan = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![
            ScriptedToolAction::Block(Arc::from("progress-start")),
            ScriptedToolAction::Emit(Ok(ToolStreamItem::Progress(
                ToolProgress::try_new("halfway", Some(50)).expect("progress"),
            ))),
            ScriptedToolAction::Block(Arc::from("terminal-gate")),
            ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(completed))),
        ],
    };
    let (mut owner, store, toolset, handle) =
        setup(1, vec![plan], 1, 1, ToolExecutionMode::Parallel).await;
    let control = toolset.control();
    while control.entries("progress-start") == 0 {
        tokio::task::yield_now().await;
    }
    let mut subscription = handle
        .subscribe_events(EventSubscriptionConfig {
            queue_capacity: 2,
            filter: EventFilter {
                include_durable: false,
                include_transient: true,
                kinds: Arc::from([RunEventKind::ToolProgress]),
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

    control.release("progress-start");
    while control.entries("terminal-gate") == 0 {
        tokio::task::yield_now().await;
    }
    let batch = subscription.next_batch().await.expect("progress batch");
    assert_eq!(batch.events().len(), 1);
    let RunEventBody::ToolProgress(progress) = batch.events()[0].body() else {
        panic!("expected tool progress");
    };
    assert_eq!(
        progress,
        &ToolProgress::try_new("halfway", Some(50)).expect("expected progress")
    );
    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover before terminal");
    assert!(recovered.state().active_tool_batch.is_some());
    assert!(recovered.state().tool_settlements.is_empty());

    control.release("terminal-gate");
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn tool_progress_advances_the_transient_sequence_without_entering_the_journal() {
    let completed = ToolResult {
        output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("output"),
        is_error: false,
    };
    let with_progress = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![
            ScriptedToolAction::Emit(Ok(ToolStreamItem::Progress(
                ToolProgress::try_new("halfway", Some(50)).expect("progress"),
            ))),
            ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(completed))),
        ],
    };
    let (baseline, _, _) = Box::pin(next_model_sequence_for_plan(completed_tool(1))).await;
    let (with_progress, sensitivity, journal) =
        Box::pin(next_model_sequence_for_plan(with_progress)).await;
    assert_eq!(with_progress, baseline + 1);
    assert_eq!(sensitivity, Sensitivity::Confidential);
    let journal = serde_json::to_string(&journal).expect("journal JSON");
    assert!(!journal.contains("tool_progress"));
    assert!(!journal.contains("halfway"));
}

#[tokio::test]
async fn tool_reported_errors_remain_bounded_and_stably_classified() {
    let spec = tool_spec("reported-error");
    let tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]> = Arc::from([spec]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let resolved = catalog(toolset, 1)
        .by_name("reported-error")
        .expect("resolved")
        .clone();
    let reported = ToolError::try_new(
        "fixture_tool_reported",
        finstack_ai_kernel::ErrorCategory::Tool,
        true,
        "safe tool-reported failure",
        Metadata::empty(),
    )
    .expect("reported error");
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Err(reported)]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
            ToolDeferralSupport::Never,
        )
        .await
        .expect_err("tool-reported stream error");
    assert_eq!(error.code(), "fixture_tool_reported");
    assert!(error.retryable());

    let application_error = ToolResult {
        output: RawJson::parse(br#"{"error":"application"}"#).expect("output"),
        is_error: true,
    };
    let assembled = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Ok(ToolStreamItem::Completed(application_error))]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
            ToolDeferralSupport::Never,
        )
        .await
        .expect("bounded application errors bypass success schema validation");
    match assembled.terminal {
        ToolTerminal::Completed(result) => assert!(result.is_error),
        ToolTerminal::Deferred(_) => panic!("completed-only fixture deferred"),
    }

    let deadline = ToolError::try_new(
        finstack_ai_runtime::ports::tool::TOOL_DEADLINE_EXCEEDED,
        finstack_ai_kernel::ErrorCategory::Deadline,
        false,
        "tool result arrived after its committed deadline",
        Metadata::empty(),
    )
    .expect("stable deadline classification");
    assert_eq!(deadline.code(), "tool_deadline_exceeded");
    assert_eq!(
        deadline
            .to_descriptor()
            .expect("deadline descriptor")
            .category,
        finstack_ai_kernel::ErrorCategory::Deadline
    );
}

#[tokio::test]
async fn failed_tool_batch_append_never_executes_a_tool() {
    let tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]> = Arc::from([tool_spec("echo")]);
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(&tools),
        vec![completed_tool(0)],
    ));
    let catalog = catalog(Arc::clone(&toolset), 1);
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![model_plan(1, "echo")],
    ));
    let store = Arc::new(FailNthAppendStore::new(7));
    let mut owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
            approval_grant: ApprovalGrantMode::PerCall,
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        ToolTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            global_max_concurrency: 1,
            stream_limits: ToolStreamLimits::default(),
        },
        Arc::new(
            finstack_ai_runtime::ports::model::ReadyModel::prepare(model)
                .await
                .expect("model readiness"),
        ),
        locked_profile(),
        catalog,
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(500)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_after_model(&handle, &store, tools).await;
    let error = handle
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect_err("tool batch append must fail");
    assert!(matches!(
        error,
        RunHandleError::Coordinator(CommitCoordinatorError::Store(StoreError::Unavailable {
            reason_code: "tool_batch_append_failed"
        }))
    ));
    assert_eq!(toolset.call_count(), 0);
    let loaded = JournalStore::load(
        store.as_ref(),
        LoadRequest {
            session_id: id::<SessionTag>(1),
        },
    )
    .await
    .expect("load");
    assert!(!loaded.committed_batches.iter().any(|batch| {
        batch.records.iter().any(|record| {
            matches!(
                record.body(),
                RecordBody::EffectRequested(requested)
                    if requested.kind() == finstack_ai_kernel::EffectKind::Tool
            )
        })
    }));
    assert_eq!(handle.status(), RunStatus::Running);
    owner.shutdown().await;
}

#[tokio::test]
async fn global_and_per_tool_limits_execute_an_admitted_parallel_group_in_waves() {
    let plans = (0..4).map(|value| gated_tool("wave", value)).collect();
    let (mut owner, store, toolset, handle) =
        setup(4, plans, 2, 2, ToolExecutionMode::Parallel).await;
    while toolset.control().entries("wave") < 2 {
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.control().entries("wave"), 2);
    assert_eq!(toolset.max_active_call_count(), 2);
    toolset.control().release("wave");
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    assert_eq!(toolset.call_count(), 4);
    assert_eq!(toolset.max_active_call_count(), 2);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Running);
    owner.shutdown().await;
}

#[tokio::test]
async fn cancellation_cleans_running_and_queued_calls_without_starting_the_queue() {
    let plans = vec![
        gated_tool("cancel-running", 0),
        completed_tool(1),
        completed_tool(2),
    ];
    let (mut owner, store, toolset, handle) =
        setup(3, plans, 1, 3, ToolExecutionMode::Parallel).await;
    while toolset.control().entries("cancel-running") == 0 {
        tokio::task::yield_now().await;
    }
    handle
        .submit(
            cancellation_env(2_200, 800),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("tool cancellation fixture")),
            }),
        )
        .await
        .expect("cancel run");
    while toolset.active_call_count() != 0 {
        tokio::task::yield_now().await;
    }
    wait_for_phase(&store, RunPhase::Cancelled).await;
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.active_call_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn shutdown_cancels_streaming_tool_children_without_leaks() {
    let (mut owner, _, toolset, handle) = setup(
        1,
        vec![gated_tool("shutdown-stream", 0)],
        1,
        1,
        ToolExecutionMode::Parallel,
    )
    .await;
    while toolset.control().entries("shutdown-stream") == 0 {
        tokio::task::yield_now().await;
    }
    owner.shutdown().await;
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn dropping_the_owner_aborts_inner_tool_children_without_detaching_them() {
    let (owner, _, toolset, handle) = setup(
        1,
        vec![gated_tool("drop-stream", 0)],
        1,
        1,
        ToolExecutionMode::Parallel,
    )
    .await;
    while toolset.control().entries("drop-stream") == 0 {
        tokio::task::yield_now().await;
    }
    drop(owner);
    for _ in 0..64 {
        if toolset.active_call_count() == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Stopped);
}

async fn assert_kernel_exclusive_mode(execution: ToolExecutionMode) {
    let plans = (0..3)
        .map(|value| gated_tool(&format!("exclusive-{value}"), value))
        .collect();
    let (mut owner, store, toolset, _) = setup(3, plans, 3, 3, execution).await;
    while toolset.control().entries("exclusive-0") == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.control().entries("exclusive-1"), 0);
    assert_eq!(toolset.max_active_call_count(), 1);
    toolset.control().release("exclusive-0");
    while toolset.control().entries("exclusive-1") == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.control().entries("exclusive-2"), 0);
    toolset.control().release("exclusive-1");
    while toolset.control().entries("exclusive-2") == 0 {
        tokio::task::yield_now().await;
    }
    toolset.control().release("exclusive-2");
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    assert_eq!(toolset.max_active_call_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn sequential_and_barrier_modes_remain_exclusive_under_larger_executor_limits() {
    assert_kernel_exclusive_mode(ToolExecutionMode::Sequential).await;
    assert_kernel_exclusive_mode(ToolExecutionMode::Barrier).await;
}

#[tokio::test]
async fn fail_run_closes_undispatched_calls_and_matches_exact_kernel_id_requirements() {
    let (mut owner, store, toolset, handle) = setup_with_failure_policy(
        3,
        vec![failed_tool("fixture_fail_run")],
        3,
        3,
        ToolExecutionMode::Sequential,
        ToolFailurePolicy::FailRun,
    )
    .await;
    for _ in 0..128 {
        tokio::task::yield_now().await;
    }
    assert_eq!(handle.status(), RunStatus::Running);
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Running);
    let recovered = CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover failed run");
    assert_eq!(recovered.state().phase, Some(RunPhase::AfterToolBatch));
    assert!(matches!(
        recovered
            .state()
            .last_tool_batch
            .as_ref()
            .map(|batch| &batch.outcome),
        Some(finstack_ai_kernel::ToolBatchOutcome::Failed { error })
            if error.code.as_str() == "fixture_fail_run"
    ));
    let tool_results = recovered
        .state()
        .messages
        .iter()
        .flat_map(Message::content)
        .filter(|content| matches!(content, ContentBlock::ToolResult(_)))
        .count();
    assert_eq!(tool_results, 3);
    owner.shutdown().await;
}

#[tokio::test]
async fn reverse_completion_commits_arrivals_but_finalizes_tool_messages_in_source_order() {
    let plans = (0..4)
        .map(|value| gated_tool(&format!("gate-{value}"), value))
        .collect();
    let (mut owner, store, toolset, _) = setup(4, plans, 4, 4, ToolExecutionMode::Parallel).await;
    for value in 0..4 {
        while toolset.control().entries(format!("gate-{value}")) == 0 {
            tokio::task::yield_now().await;
        }
    }
    for value in (0..4).rev() {
        toolset.control().release(format!("gate-{value}"));
        tokio::task::yield_now().await;
    }
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    let loaded = finstack_ai_runtime::ports::journal::JournalStore::load(
        store.as_ref(),
        finstack_ai_runtime::ports::journal::LoadRequest {
            session_id: id::<SessionTag>(1),
        },
    )
    .await
    .expect("load");
    let arrivals = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .filter_map(|record| match record.body() {
            RecordBody::EffectCompleted(completion)
                if completion.output_contract().kind == EffectOutputKind::ToolResult =>
            {
                serde_json::from_str::<ToolResultBlock>(completion.output().as_str())
                    .ok()
                    .map(|result| *result.tool_call_id())
            }
            _ => None,
        })
        .collect::<Vec<ToolCallId>>();
    assert_eq!(arrivals.len(), 4);
    assert_ne!(arrivals, {
        let mut sorted = arrivals.clone();
        sorted.sort();
        sorted
    });
    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover");
    let finalized = recovered
        .state()
        .messages
        .iter()
        .skip(1)
        .filter_map(|message| match message.content() {
            [ContentBlock::ToolResult(result)] => Some(*result.tool_call_id()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let source = recovered.state().messages[0]
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(*call.tool_call_id()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finalized, source);
    owner.shutdown().await;
}

#[tokio::test]
async fn native_panic_becomes_stable_call_failure_without_leaking_payload_or_killing_sibling() {
    let secret = "panic-payload-must-not-be-durable";
    let plans = vec![
        ScriptedToolPlan {
            panic_on_call: Some(Arc::from(secret)),
            actions: Vec::new(),
        },
        completed_tool(1),
    ];
    let (mut owner, store, toolset, handle) =
        setup(2, plans, 2, 2, ToolExecutionMode::Parallel).await;
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    let loaded = finstack_ai_runtime::ports::journal::JournalStore::load(
        store.as_ref(),
        finstack_ai_runtime::ports::journal::LoadRequest {
            session_id: id::<SessionTag>(1),
        },
    )
    .await
    .expect("load");
    let bytes = serde_json::to_vec(&loaded.committed_batches).expect("journal JSON");
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(text.contains("tool_panicked"));
    assert!(!text.contains(secret));
    assert_eq!(toolset.call_count(), 2);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Running);
    owner.shutdown().await;
}
