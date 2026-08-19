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
