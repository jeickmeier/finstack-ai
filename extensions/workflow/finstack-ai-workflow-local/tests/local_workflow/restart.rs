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
    Box::pin(resumed.complete_external(command, timestamp(3_000)))
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
        deferral: finstack_ai_runtime::ToolDeferralSupport::Never,
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
    let ready_model = Arc::new(
        finstack_ai_runtime::ReadyModel::prepare(Arc::clone(&model))
            .await
            .expect("model readiness"),
    );
    let owner = RunTaskOwner::spawn_with_model_and_tools(
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
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        ready_model,
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

    let mut driver = WorkflowSession::trusted_seeded(store.clone(), locator(), clock.clone(), 700)
        .await
        .expect("attach")
        .with_ports(Arc::clone(&model), locked_profile(), Some(catalog));
    let WorkflowWait::Interaction { interaction_id, .. } =
        driver.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected interaction");
    };
    driver.abort_owner();
    let mut resumed = WorkflowSession::trusted_seeded(store.clone(), locator(), clock.clone(), 701)
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

fn cron_journal_file() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("dir");
    let path = dir.path().join("journal.sqlite");
    (dir, path)
}

fn open_sqlite_journal(path: &Path) -> Arc<SqliteJournalStore> {
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: path.to_path_buf(),
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 4,
                batches_per_session: 128,
                records_per_session: 512,
                snapshot_bytes: 4_096,
            },
            busy_timeout: StdDuration::from_secs(1),
        })
        .expect("sqlite journal"),
    )
}

async fn seed_accepted_run(
    store: Arc<SqliteJournalStore>,
    model: Arc<dyn Model>,
    clock: ExternalClock,
    seed: u64,
) {
    let owner = spawn_model_owner(CommitCoordinator::new(store.clone()), model, clock, seed).await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state_on(store as Arc<dyn JournalStore>, |state| {
        state.accepted.is_some()
    })
    .await;
    drop(owner);
}

#[tokio::test]
async fn cron_schedule_survives_worker_restart() {
    let (_dir, path) = cron_journal_file();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let first_next;
    {
        let store = open_sqlite_journal(&path);
        seed_accepted_run(store.clone(), Arc::clone(&model), clock.clone(), 810).await;
        let cron = Arc::new(SqliteCronStore::open(&path).expect("cron"));
        let driver = LocalWorkflowDriver::attach_seeded(store, locator(), clock.clone(), 810, cron)
            .await
            .expect("attach")
            .with_ports(Arc::clone(&model), locked_profile(), None);
        let scheduled = driver
            .schedule_cron("tick", IntervalSchedule::parse("every 10ms").expect("expr"))
            .expect("schedule");
        first_next = scheduled.next_fire_at;
        assert_eq!(scheduled.fire_count, 0);
        assert_eq!(driver.tenant_scope(), "tenant-a");
    }

    let store = open_sqlite_journal(&path);
    let cron = Arc::new(SqliteCronStore::open(&path).expect("reopen cron"));
    let resumed = LocalWorkflowDriver::attach_seeded(store, locator(), clock, 811, cron)
        .await
        .expect("resume")
        .with_ports(model, locked_profile(), None);
    let schedules = resumed.schedules().expect("schedules");
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].schedule_id.as_ref(), "tick");
    assert_eq!(schedules[0].next_fire_at, first_next);
    assert_eq!(schedules[0].fire_count, 0);
    assert!(resumed.catch_up_fires().is_empty());
}

#[tokio::test]
async fn cron_catch_up_fires_once_then_advances() {
    let (_dir, path) = cron_journal_file();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    {
        let store = open_sqlite_journal(&path);
        seed_accepted_run(store.clone(), Arc::clone(&model), clock.clone(), 820).await;
        let cron = Arc::new(SqliteCronStore::open(&path).expect("cron"));
        let driver = LocalWorkflowDriver::attach_seeded(store, locator(), clock.clone(), 820, cron)
            .await
            .expect("attach")
            .with_ports(Arc::clone(&model), locked_profile(), None);
        driver
            .schedule_cron("tick", IntervalSchedule::parse("every 10ms").expect("expr"))
            .expect("schedule");
    }

    clock.jump(25).expect("jump past several ticks");
    let now = clock.now().expect("now");
    let store = open_sqlite_journal(&path);
    let cron = Arc::new(SqliteCronStore::open(&path).expect("reopen cron"));
    let resumed = LocalWorkflowDriver::attach_seeded(store, locator(), clock.clone(), 821, cron)
        .await
        .expect("catch-up")
        .with_ports(Arc::clone(&model), locked_profile(), None);
    assert_eq!(resumed.catch_up_fires().len(), 1, "one catch-up fire");
    assert_eq!(resumed.catch_up_fires()[0].schedule_id.as_ref(), "tick");
    let after = &resumed.schedules().expect("schedules")[0];
    assert_eq!(after.fire_count, 1);
    assert!(after.next_fire_at > now, "next tick is in the future");
    assert_eq!(after.next_fire_at.as_unix_ms(), 2_030);
}

#[tokio::test]
async fn cron_second_attach_does_not_catch_up_again() {
    let (_dir, path) = cron_journal_file();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    {
        let store = open_sqlite_journal(&path);
        seed_accepted_run(store.clone(), Arc::clone(&model), clock.clone(), 830).await;
        let cron = Arc::new(SqliteCronStore::open(&path).expect("cron"));
        let driver = LocalWorkflowDriver::attach_seeded(store, locator(), clock.clone(), 830, cron)
            .await
            .expect("attach")
            .with_ports(Arc::clone(&model), locked_profile(), None);
        driver
            .schedule_cron("tick", IntervalSchedule::parse("every 10ms").expect("expr"))
            .expect("schedule");
    }
    clock.jump(25).expect("jump");
    {
        let store = open_sqlite_journal(&path);
        let cron = Arc::new(SqliteCronStore::open(&path).expect("reopen cron"));
        let first = LocalWorkflowDriver::attach_seeded(store, locator(), clock.clone(), 831, cron)
            .await
            .expect("first catch-up")
            .with_ports(Arc::clone(&model), locked_profile(), None);
        assert_eq!(first.catch_up_fires().len(), 1);
        assert_eq!(first.schedules().expect("schedules")[0].fire_count, 1);
    }

    let store = open_sqlite_journal(&path);
    let cron = Arc::new(SqliteCronStore::open(&path).expect("second reopen"));
    let second = LocalWorkflowDriver::attach_seeded(store, locator(), clock, 832, cron)
        .await
        .expect("second attach")
        .with_ports(model, locked_profile(), None);
    assert!(
        second.catch_up_fires().is_empty(),
        "no second catch-up without another clock jump"
    );
    assert_eq!(second.schedules().expect("schedules")[0].fire_count, 1);
}

#[tokio::test]
async fn cron_catch_up_is_tenant_scoped() {
    let (_dir, path) = cron_journal_file();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    {
        let store = open_sqlite_journal(&path);
        seed_accepted_run(store.clone(), Arc::clone(&model), clock.clone(), 840).await;
        let cron = Arc::new(SqliteCronStore::open(&path).expect("cron"));
        let driver = LocalWorkflowDriver::attach_seeded(
            store,
            locator(),
            clock.clone(),
            840,
            Arc::clone(&cron) as Arc<dyn CronScheduleStore>,
        )
        .await
        .expect("attach")
        .with_ports(Arc::clone(&model), locked_profile(), None);
        driver
            .schedule_cron("tick", IntervalSchedule::parse("every 10ms").expect("expr"))
            .expect("schedule");
        cron.upsert(&CronSchedule {
            tenant_scope: Arc::from("tenant-b"),
            schedule_id: Arc::from("tick"),
            expression: IntervalSchedule::parse("every 10ms").expect("expr"),
            origin: timestamp(2_000),
            next_fire_at: timestamp(2_010),
            last_fired_at: None,
            fire_count: 0,
        })
        .expect("foreign schedule");
        assert_eq!(driver.tenant_scope(), "tenant-a");
        assert_eq!(driver.schedules().expect("tenant-a").len(), 1);
    }

    clock.jump(25).expect("jump");
    let store = open_sqlite_journal(&path);
    let cron = Arc::new(SqliteCronStore::open(&path).expect("reopen cron"));
    let resumed = LocalWorkflowDriver::attach_seeded(
        store,
        locator(),
        clock,
        841,
        Arc::clone(&cron) as Arc<dyn CronScheduleStore>,
    )
    .await
    .expect("catch-up")
    .with_ports(model, locked_profile(), None);
    assert_eq!(resumed.catch_up_fires().len(), 1);
    assert_eq!(
        resumed.catch_up_fires()[0].tenant_scope.as_ref(),
        "tenant-a"
    );
    assert_eq!(resumed.schedules().expect("tenant-a")[0].fire_count, 1);
    let foreign = cron.load_tenant("tenant-b").expect("tenant-b");
    assert_eq!(foreign.len(), 1);
    assert_eq!(
        foreign[0].fire_count, 0,
        "foreign tenant is not catch-up fired"
    );
}
