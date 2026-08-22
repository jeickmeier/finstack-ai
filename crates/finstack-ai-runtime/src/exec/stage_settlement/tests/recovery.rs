// ---- recover then fold ------------------------------------------------

#[test]
fn missing_before_run_outcome_invokes_the_whole_chain_after_recover() {
    let store = Arc::new(MemoryStore::new());
    drop(accepted_on(Arc::clone(&store)));
    let mut recovered = recover(store);
    assert_eq!(recovered.state().phase, Some(RunPhase::BeforeRun));

    let calls = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver("fixture.before-run", Stage::BeforeRun, &calls);
    block_on(settle_facade_stage(
        &mut recovered,
        Some(&driver),
        &test_sources(),
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect("settle after recover");
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "a missing StageOutcomeRecorded must re-run the BeforeRun chain"
    );
}

#[test]
fn completed_middleware_effect_replays_without_a_second_invocation() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = accepted_on(Arc::clone(&store));
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver("fixture.before-run", Stage::BeforeRun, &calls);
    let cursor = StageCursor {
        cycle: 0,
        stage: Stage::BeforeRun,
    };
    let sources = test_sources();
    let settled = before_run_settled();
    let input = stage_input(
        coordinator.state(),
        cursor.stage,
        &settled.outcome,
        &test_profile(),
        coordinator.context_projection(),
    )
    .expect("before-run input");

    let fold = block_on(run_stage_chain(
        &mut coordinator,
        Some(&driver),
        &sources,
        cursor,
        input,
    ))
    .expect("commit middleware effect");
    assert!(fold.is_identity());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    drop(coordinator);

    let mut recovered = recover(store);
    let (requested, _) = recovered
        .replayed_completed_effects()
        .values()
        .next()
        .expect("completed middleware effect index");
    assert_eq!(
        recovered
            .replayed_extension_envelope(requested.effect_id())
            .and_then(RecordEnvelope::run_id),
        Some(id::<RunTag>(3))
    );
    assert_eq!(
        requested
            .pipeline()
            .map(finstack_ai_kernel::PipelinePosition::chain_digest),
        Some(driver.chain().digest())
    );
    block_on(settle_facade_stage(
        &mut recovered,
        Some(&driver),
        &test_sources(),
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        settled,
    ))
    .expect("settle from recorded middleware outcome");
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "a completed middleware effect must be replayed instead of invoked again"
    );
}

#[test]
fn rejected_middleware_outcome_commits_a_failed_effect_settlement() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    let driver = driver_for(
        "fixture.invalid-before-run",
        Stage::BeforeRun,
        StageOutcome::FilterTools(Arc::from([])),
    );

    let error = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &test_sources(),
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect_err("FilterTools is invalid at BeforeRun");
    assert!(matches!(
        error,
        RunHandleError::Middleware { ref code }
            if code.as_ref() == crate::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED
    ));
    assert!(
        coordinator.state().pending_extension_effect.is_none(),
        "validation failure must not strand a pending durable effect"
    );
    assert_eq!(coordinator.state().extension_settlements.len(), 1);
}

#[test]
fn recorded_before_run_is_not_invoked_when_prepare_context_settles() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = accepted_on(Arc::clone(&store));
    let before_run_calls = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver("fixture.before-run", Stage::BeforeRun, &before_run_calls);
    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &test_sources(),
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect("record BeforeRun");
    assert_eq!(before_run_calls.load(Ordering::Relaxed), 1);
    drop(coordinator);

    let mut recovered = recover(store);
    assert_eq!(recovered.state().phase, Some(RunPhase::PreparingContext));
    let driver = counting_driver("fixture.before-run", Stage::BeforeRun, &before_run_calls);
    block_on(settle_facade_stage(
        &mut recovered,
        Some(&driver),
        &test_sources(),
        &test_profile(),
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        prepare_context_settled(),
    ))
    .expect("settle PrepareContext");
    assert_eq!(
        before_run_calls.load(Ordering::Relaxed),
        1,
        "a recorded BeforeRun chain must not run again at PrepareContext"
    );
}

#[test]
fn missing_prepare_context_outcome_invokes_that_whole_chain_after_recover() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = accepted_on(Arc::clone(&store));
    drive_to_prepare_context(&mut coordinator);
    drop(coordinator);

    let mut recovered = recover(store);
    assert_eq!(recovered.state().phase, Some(RunPhase::PreparingContext));
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver("fixture.prepare-context", Stage::PrepareContext, &calls);
    block_on(settle_facade_stage(
        &mut recovered,
        Some(&driver),
        &test_sources(),
        &test_profile(),
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        prepare_context_settled(),
    ))
    .expect("settle after recover");
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "a missing PrepareContext StageOutcomeRecorded must re-run that chain"
    );
}

#[test]
fn recover_clears_the_chain_reinstall_then_stage_driver_invokes() {
    let store = Arc::new(MemoryStore::new());
    drop(accepted_on(Arc::clone(&store)));
    let mut recovered = recover(store);
    assert!(
        recovered.middleware_chain().is_none(),
        "recover must drop the installed chain"
    );
    assert!(
        super::stage_driver(&recovered, &CancellationSignal::new()).is_none(),
        "WorkflowSession respawn must reinstall before a stage driver exists"
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let middleware: Arc<dyn crate::middleware::Middleware> = Arc::new(Counting {
        descriptor: descriptor("fixture.before-run", Stage::BeforeRun),
        calls: Arc::clone(&calls),
    });
    recovered.install_middleware_chain(Arc::new(
        ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
            .expect("chain"),
    ));
    let driver = super::stage_driver(&recovered, &CancellationSignal::new())
        .expect("reinstall must produce a driver from the bound chain");
    block_on(settle_facade_stage(
        &mut recovered,
        Some(&driver),
        &test_sources(),
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect("settle after reinstall");
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "the chain bound at respawn must run, not a recover passthrough"
    );
}
