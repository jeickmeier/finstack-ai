use super::*;
use finstack_ai_kernel::KernelInput;

#[tokio::test]
async fn resume_must_finish_original_run() {
    let gate = Arc::<str>::from("review-lane-resume");
    let mut plan = completed("resumed successfully");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![plan, completed("resumed successfully")],
    ));
    let control = model.control();
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");
    let run = lane.run(&agent, request("resume me")).expect("run");
    run.close_events();
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model gate");
    lane.suspend().await.expect("suspend");
    lane.resume(&agent).await.expect("resume");
    control.release(&gate);
    let result = tokio::time::timeout(Duration::from_secs(3), run.result()).await;
    let recovered = CommitCoordinator::recover_run(
        agent.journal_store(),
        run.locator().session_id,
        Some(run.locator().run_id),
    )
    .await
    .expect("inspect resumed state");
    assert!(
        matches!(result, Ok(Ok(_))),
        "resumed result={result:?}, phase={:?}, model requests={}",
        recovered.state().phase(),
        model.request_count()
    );
}

#[tokio::test]
async fn resolved_agent_preserves_inactive_capability_mask() {
    let model: Arc<dyn Model> =
        Arc::new(ScriptedModel::from_plans(profile(), vec![completed("ok")]));
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let calculator: Arc<dyn Toolset> = Arc::new(CalculatorToolset::try_new().expect("calculator"));
    let toolset = ComponentRef::new(
        ComponentId::parse("test.tools.calculator").expect("toolset"),
        Some(VERSION),
    );
    let with_research = Agent::builder(
        AgentId::parse("test.agent.mask-restore").expect("agent"),
        BundleId::parse("test.bundle.mask-restore").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.mask-restore").expect("model"),
                Some(VERSION),
            ),
            Arc::clone(&model),
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.mask-restore").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .capability_toolset(toolset.clone(), calculator)
    .capability(research_capability(toolset))
    .build()
    .await
    .expect("agent with research");
    let activated = [ActiveCapability {
        capability_id: CapabilityId::parse("test.capability.research").expect("id"),
        source: CapabilityActivationSource::Model,
    }];
    with_research
        .validate_restored_mask(&activated)
        .expect("journaled id in lock");
    let live = with_research.live_tool_specs(&activated);
    assert!(
        live.iter()
            .any(|tool| tool.model_name.as_ref() == "calculator")
    );
    let hidden = with_research.live_tool_specs(&[]);
    assert!(
        hidden
            .iter()
            .all(|tool| tool.model_name.as_ref() != "calculator")
    );

    let rebuilt = Agent::try_from_resolved(Arc::clone(with_research.resolved())).expect("rebuild");
    assert!(
        rebuilt
            .live_tool_specs(&[])
            .iter()
            .all(|tool| tool.model_name.as_ref() != "calculator"),
        "inactive capability leaked into resolved Agent"
    );
}

struct BlockingMiddleware {
    entered: Arc<AtomicBool>,
}
impl Middleware for BlockingMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("test.middleware.review-blocking")
                    .expect("component id"),
                version: VERSION,
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            stages: StageMask::from_stages([Stage::BeforeRun]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        }
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        self.entered.store(true, Ordering::Release);
        Box::pin(async move {
            ctx.run.cancellation.cancelled().await;
            Ok(StageOutcome::Continue)
        })
    }
}

async fn blocking_middleware_agent() -> (Agent, Arc<AtomicBool>) {
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![
            completed_with_id("first answer", "finalize-retry-completion-1"),
            completed_with_id("second answer", "finalize-retry-completion-2"),
        ],
    ));
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let entered = Arc::new(AtomicBool::new(false));
    let middleware: Arc<dyn Middleware> = Arc::new(BlockingMiddleware {
        entered: Arc::clone(&entered),
    });
    let agent = Agent::builder(
        AgentId::parse("test.agent.finalize-retry").expect("agent"),
        BundleId::parse("test.bundle.finalize-retry").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.finalize-retry").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.finalize-retry").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store) as Arc<dyn JournalStore>,
        ),
    )
    .middleware(middleware)
    .build()
    .await
    .expect("agent");
    (agent, entered)
}

#[tokio::test]
async fn cancel_must_interrupt_cooperative_middleware() {
    let (agent, entered) = blocking_middleware_agent().await;
    let run = agent
        .start(request("cancel blocked middleware"))
        .expect("start");
    run.close_events();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !entered.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("middleware entered");
    let handle = run.runtime_handle().await.unwrap();
    let rejected = tokio::time::timeout(
        Duration::from_millis(500),
        handle.submit(
            super::super::prepare::NativeIds::cancellation_environment().unwrap(),
            KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
                initiator: finstack_ai_kernel::CancellationInitiator::Deadline,
                reason: None,
            }),
        ),
    )
    .await
    .expect("invalid cancellation must also be serviced");
    assert!(
        rejected.is_err(),
        "a deadline that has not elapsed cannot cancel"
    );
    assert!(handle.live_state().terminal.is_none());
    tokio::time::timeout(Duration::from_millis(500), run.cancel())
        .await
        .expect("bounded cancel")
        .expect("durable cancel");
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), run.result())
            .await
            .unwrap(),
        Err(AgentRunError::Cancelled)
    ));
    let recovered = CommitCoordinator::recover_run(
        agent.journal_store(),
        run.locator().session_id,
        Some(run.locator().run_id),
    )
    .await
    .unwrap();
    assert!(recovered.state().cancellation().is_some());
    assert!(recovered.state().pending_extension_effect().is_none());
}

#[tokio::test]
async fn child_accept_rejects_changed_request() {
    let gate = Arc::<str>::from("review-child-parent");
    let mut plan = completed("parent");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![plan, completed("child")],
    ));
    let control = model.control();
    let (agent, _store) = child_capable_agent(model).await;
    let parent = agent.start(request("parent")).expect("parent");
    parent.close_events();
    tokio::time::timeout(Duration::from_secs(2), async {
        while control.entries(&gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent running");
    let prepared = Box::pin(parent.prepare_child(
        &agent,
        request("approved child input"),
        ChildPlacement::IsolatedChildSession,
        None,
    ))
    .await
    .expect("prepare");
    let changed = parent
        .accept_child(&prepared, &agent, request("different uncommitted input"))
        .await;
    assert!(
        changed.is_err(),
        "accept_child ignored the frozen request digest"
    );
}

#[tokio::test]
async fn external_completion_routes_second_run_in_session() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("first"), deferred("second-job")],
    ));
    let (agent, store) = model_only_agent(model).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("lane");
    let first = lane.run(&agent, request("first")).expect("first");
    first.close_events();
    tokio::time::timeout(Duration::from_secs(2), first.result())
        .await
        .expect("first bounded")
        .expect("first complete");
    let parent = lane.run(&agent, request("second")).expect("second");
    parent.close_events();
    let effect_id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(commit) = CommitCoordinator::recover_run(
                agent.journal_store(),
                parent.locator().session_id,
                Some(parent.locator().run_id),
            )
            .await
                && let Some(pending) = commit.state().pending_model_effect()
                && let Some(deferred) = pending.deferred.as_ref()
            {
                return deferred.effect_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second deferred");
    let command = ExternalEffectCompletionCommand::try_new(
        parent.locator().clone(),
        security().principal().clone(),
        AuthorizationEvidence::try_new(
            security().authorization_policy_version(),
            security().authorization_decision_id(),
        )
        .expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            "ext-1",
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    finstack_ai_kernel::ErrorCategory::Model,
                    true,
                )
                .expect("error"),
            },
        )
        .expect("completion"),
    )
    .expect("command");
    let outcome = Box::pin(parent.complete_external(command))
        .await
        .expect("complete_external");
    assert!(
        matches!(
            outcome,
            ExternalRouteOutcome::Committed(_) | ExternalRouteOutcome::Rejected { .. }
        ),
        "external completion must route, not stay data-only"
    );
}

#[tokio::test]
async fn repeated_resume_rebinds_events_and_preserves_single_input() {
    let gate = Arc::<str>::from("repeated-resume");
    let mut plan = completed("resumed final");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![plan.clone(), plan.clone(), plan],
    ));
    let control = model.control();
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let session = Session::create(store, "tenant-preview").await.unwrap();
    let lane = session.lane("main").await.unwrap();
    let run = lane.run(&agent, request("one input")).unwrap();
    let events_run = run.clone();
    let events = tokio::spawn(async move {
        let mut count = 0;
        while events_run.next_event_batch().await.unwrap().is_some() {
            count += 1;
        }
        count
    });
    for entered in 1..=2 {
        tokio::time::timeout(Duration::from_secs(2), async {
            while control.entries(&gate) < entered {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let old = run.runtime_handle().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), lane.suspend())
            .await
            .expect("bounded park")
            .unwrap();
        assert!(
            !events.is_finished(),
            "park must not close the original event consumer"
        );
        assert!(run.inner.result.lock().unwrap().is_none());
        if entered == 1 {
            let (different, _) = blocking_middleware_agent().await;
            assert!(
                lane.resume(&different).await.is_err(),
                "resume must preserve the accepted agent lock"
            );
        }
        lane.resume(&agent).await.unwrap();
        assert!(matches!(
            old.status(),
            finstack_ai_runtime::run::RunStatus::Stopped
        ));
    }
    control.release(&gate);
    tokio::time::timeout(Duration::from_secs(2), run.result())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), events)
            .await
            .unwrap()
            .unwrap()
            > 0
    );
    assert_eq!(model.request_count(), 3);
    let replay = CommitCoordinator::recover_run(
        agent.journal_store(),
        run.locator().session_id,
        Some(run.locator().run_id),
    )
    .await
    .unwrap();
    assert!(matches!(
        replay.state().terminal(),
        Some(TerminalState::Completed(_))
    ));
    // The lane becomes available again after the same SDK run finishes.
    let next_model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("next")],
    ));
    let (next_agent, _) = model_only_agent(next_model).await;
    let next = lane.run(&next_agent, request("next input")).unwrap();
    next.close_events();
    tokio::time::timeout(Duration::from_secs(2), next.result())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn deadline_interrupts_pending_middleware() {
    let (agent, entered) = blocking_middleware_agent().await;
    let mut input = request("deadline in middleware");
    input.timeout = Duration::from_millis(100);
    let run = agent.start(input).unwrap();
    run.close_events();
    let result = tokio::time::timeout(Duration::from_secs(2), run.result())
        .await
        .unwrap();
    assert!(entered.load(Ordering::Acquire));
    assert!(
        matches!(result, Err(AgentRunError::Timeout { .. })),
        "{result:?}"
    );
}

struct BlockingContext {
    entered: Arc<AtomicBool>,
    dropped_after_signal: Arc<AtomicBool>,
}

struct ContextDrop {
    signal: finstack_ai_runtime::ports::model::CancellationSignal,
    observed: Arc<AtomicBool>,
}
impl Drop for ContextDrop {
    fn drop(&mut self) {
        self.observed
            .store(self.signal.is_cancelled(), Ordering::Release);
    }
}

impl finstack_ai_runtime::ports::context::ContextProvider for BlockingContext {
    fn descriptor(&self) -> finstack_ai_runtime::ports::context::ContextProviderDescriptor {
        finstack_ai_runtime::ports::context::ContextProviderDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("test.context.blocking").unwrap(),
                version: VERSION,
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        }
    }
    fn collect(
        &self,
        ctx: finstack_ai_runtime::ports::context::ContextCallContext,
        _: finstack_ai_runtime::ports::context::ContextRequest,
    ) -> PortFuture<
        Result<
            finstack_ai_runtime::ports::context::ContextContribution,
            finstack_ai_runtime::ports::context::ContextError,
        >,
    > {
        let entered = Arc::clone(&self.entered);
        let guard = ContextDrop {
            signal: ctx.run.cancellation,
            observed: Arc::clone(&self.dropped_after_signal),
        };
        Box::pin(async move {
            let _guard = guard;
            entered.store(true, Ordering::Release);
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn cancellation_drops_noncooperative_context_after_signalling() {
    let entered = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("unused")],
    ));
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4096,
        })
        .unwrap(),
    );
    let component =
        |name: &str| ComponentRef::new(ComponentId::parse(name).unwrap(), Some(VERSION));
    let agent = Agent::builder(
        AgentId::parse("test.agent.context-cancel").unwrap(),
        BundleId::parse("test.bundle.context-cancel").unwrap(),
        (component("test.model.context-cancel"), model),
        (component("test.store.context-cancel"), store),
    )
    .context_provider(Arc::new(BlockingContext {
        entered: Arc::clone(&entered),
        dropped_after_signal: Arc::clone(&dropped),
    }))
    .build()
    .await
    .unwrap();
    let run = agent.start(request("cancel context")).unwrap();
    run.close_events();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !entered.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), run.cancel())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(run.result().await, Err(AgentRunError::Cancelled)));
    assert!(
        dropped.load(Ordering::Acquire),
        "context must be signalled before dropping its future"
    );
    let recovered = CommitCoordinator::recover_run(
        agent.journal_store(),
        run.locator().session_id,
        Some(run.locator().run_id),
    )
    .await
    .unwrap();
    assert!(recovered.state().cancellation().is_some());
    assert!(recovered.state().pending_extension_effect().is_none());
}

#[tokio::test]
async fn failed_idle_resume_does_not_reserve_the_lane() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("available")],
    ));
    let (agent, store) = model_only_agent(model).await;
    let session = Session::create(store, "tenant-preview").await.unwrap();
    let lane = session.lane("main").await.unwrap();
    assert!(lane.resume(&agent).await.is_err());
    let run = lane.run(&agent, request("still available")).unwrap();
    run.close_events();
    tokio::time::timeout(Duration::from_secs(2), run.result())
        .await
        .unwrap()
        .unwrap();
}
