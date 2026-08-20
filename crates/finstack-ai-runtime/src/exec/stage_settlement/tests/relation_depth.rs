// ---- relation_depth on the stage-boundary context ---------------------

#[test]
fn the_stage_boundary_context_carries_the_accepted_runs_relation_depth() {
    // The accepted run is a `ChildAgent` at relation depth 1 (see
    // `child_acceptance`), not a root at depth 0, so a stub `0` would pass
    // this test only by coincidence — the assertion below pins the real
    // accepted value flowing through `stage_dispatch_seed` and
    // `invoke_stage_chain` into `RunCallContext::relation_depth`.
    let mut coordinator = accepted_child_coordinator(RunLimits::empty());
    let sources = test_sources();
    let captured: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let driver = capturing_depth_driver("fixture.depth", Stage::BeforeRun, &captured);
    assert!(driver.is_active(Stage::BeforeRun));

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect("settled");

    assert_eq!(
        *captured.lock().expect("lock"),
        Some(1),
        "the stage-boundary context must carry the accepted run's own relation depth"
    );
}
