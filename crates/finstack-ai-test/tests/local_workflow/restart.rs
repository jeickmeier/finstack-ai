#[tokio::test]
async fn timer_survives_worker_restart() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        800,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::BeforeFinalize)
    })
    .await;
    owner
        .handle()
        .submit(
            env(2_300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                ReducerStageOutcome::Retry(
                    RetryDirective::try_new(
                        RetryClassification::Model,
                        KernelDuration::from_millis(10),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    wait_state(&store, |state| state.phase == Some(RunPhase::Sleeping)).await;
    drop(owner);

    let mut driver = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 800).await;
    let WorkflowWait::Timer { effect_id, due_at } = driver.drive_until_wait().await.expect("timer")
    else {
        panic!("expected timer wait");
    };
    let first = (effect_id, due_at);
    driver.abort_owner();
    clock.jump(20).expect("jump");
    let mut resumed = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 801).await;
    let again = resumed.drive_until_wait().await.expect("resume");
    match again {
        WorkflowWait::Timer { effect_id, due_at } => {
            assert_eq!((effect_id, due_at), first, "same timer after restart");
        }
        WorkflowWait::Terminal { .. } => {}
        other => panic!("unexpected wait {other:?}"),
    }
    let second = attach_driver(store, model, clock, 802).await;
    let _ = second;
}

#[tokio::test]
async fn deferred_survives_worker_restart() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
                ModelDeferral {
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.model.scripted").expect("component"),
                        "job-1",
                        RawJson::parse(b"{}").expect("metadata"),
                    )
                    .expect("handle"),
                    reconciliation: ReconciliationPolicy::CallbackOnly,
                    next_poll_at: None,
                    expires_at: None,
                },
            )))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        740,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);

    let mut driver = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 740).await;
    let WorkflowWait::DeferredEffect { effect_id, .. } =
        driver.drive_until_wait().await.expect("deferred")
    else {
        panic!("expected deferred");
    };
    driver.abort_owner();
    let mut resumed = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 741).await;
    let WorkflowWait::DeferredEffect {
        effect_id: again, ..
    } = resumed.drive_until_wait().await.expect("resume deferred")
    else {
        panic!("same deferred after restart");
    };
    assert_eq!(again, effect_id);
    let command = ExternalEffectCompletionCommand::try_new(
        locator(),
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            "ext-1",
            ExternalEffectOutcome::Completed {
                output: RawJson::parse(r#"{"ok":true}"#).expect("output"),
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("completion"),
    )
    .expect("command");
    resumed
        .complete_external(command, timestamp(3_000))
        .await
        .expect("complete");
    let _ = resumed.drive_until_wait().await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "A04 keeps catalog, park, abort, resume, and resolve contiguous"
)]
async fn interaction_survives_worker_restart() {
    let tools: Arc<[ToolSpec]> = Arc::from([ToolSpec {
        id: finstack_ai_kernel::ToolId::parse("finstack.tools.echo").expect("tool id"),
        model_name: Arc::from("echo"),
        title: Arc::from("echo"),
        description: Arc::from("echo"),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"value":{"type":"integer"}},"required":["value"],"type":"object"}"#,
        )
        .expect("input"),
        output_schema: None,
        execution: finstack_ai_kernel::ToolExecutionMode::Parallel,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
    }]);
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(&tools),
        vec![ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(
                ToolResult {
                    output: RawJson::parse(r#"{"ok":true,"value":1}"#).expect("out"),
                    is_error: false,
                },
            )))],
        }],
    ));
    let toolset_port: Arc<dyn Toolset> = toolset;
    let policies = tools
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                ToolExecutionPolicy {
                    failure_policy: ToolFailurePolicy::ReturnToModel,
                    approval: ToolPolicyDecision::RequireApproval,
                    max_concurrency: 1,
                },
            )
        })
        .collect();
    let catalog = Arc::new(
        ResolvedToolCatalog::try_new(
            [ToolsetRegistration {
                toolset: toolset_port,
                policies,
                components: BTreeMap::new(),
            }],
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .expect("catalog"),
    );
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    name: Some(Arc::from("echo")),
                    arguments_delta: Arc::from(arguments.as_str()),
                    provider_call_id: None,
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([]),
                    tool_calls: Arc::from([ModelToolCall {
                        name: Arc::from("echo"),
                        arguments,
                provider_call_id: None,
            }]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::empty(),
                    completion_id: Arc::from("completion-tools"),
                    continuation_state: None,
                }))),
            ],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_500));
    let owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        ToolTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        Arc::clone(&model),
        locked_profile(),
        Arc::clone(&catalog),
        clock.clone(),
        CounterRandom(AtomicU64::new(700)),
    )
    .await
    .expect("owner");
    drive_to_after_model(&owner.handle(), &store, Arc::clone(&tools)).await;
    owner
        .handle()
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingInteraction)
    })
    .await;
    drop(owner);

    let mut driver = WorkflowSession::trusted(store.clone(), locator(), clock.clone(), 700)
        .await
        .expect("attach")
        .with_ports(Arc::clone(&model), locked_profile(), Some(catalog));
    let WorkflowWait::Interaction { interaction_id, .. } =
        driver.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected interaction");
    };
    driver.abort_owner();
    let mut resumed = WorkflowSession::trusted(store.clone(), locator(), clock.clone(), 701)
        .await
        .expect("resume")
        .with_ports(model, locked_profile(), None);
    let WorkflowWait::Interaction {
        interaction_id: again,
        ..
    } = resumed.drive_until_wait().await.expect("same interaction")
    else {
        panic!("same interaction after restart");
    };
    assert_eq!(again, interaction_id);
    let command = InteractionResolutionCommand::try_new(
        locator(),
        InteractionResolution::try_new(
            interaction_id,
            "resolution-1",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
            RawJson::parse(r#"{"approved":true}"#).expect("response"),
            None::<&str>,
        )
        .expect("resolution"),
    )
    .expect("command");
    resumed
        .resolve_interaction(command, timestamp(3_000))
        .await
        .expect("resolve");
}
