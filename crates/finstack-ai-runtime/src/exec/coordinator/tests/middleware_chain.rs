#[test]
fn installed_chain_is_retrievable() {
    let mut coordinator = CommitCoordinator::new(Arc::new(FakeStore::new(FakeMode::Normal)));
    assert!(coordinator.middleware_chain().is_none());
    let chain = Arc::new(ResolvedMiddlewareChain::try_new(Vec::new()).expect("empty chain"));
    coordinator.install_middleware_chain(Arc::clone(&chain));
    assert!(coordinator.middleware_chain().is_some());
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[test]
fn stage_dispatch_seed_matches_pending_model_seed_security_fields() {
    let mut coordinator = CommitCoordinator::new(Arc::new(FakeStore::new(FakeMode::Normal)));
    block_on(drive_to_model_request(&mut coordinator));

    let model_seed = coordinator
        .pending_model_seed()
        .expect("pending model seed");
    let stage_seed = coordinator
        .stage_dispatch_seed()
        .expect("stage dispatch seed");

    assert_eq!(stage_seed.locator, model_seed.locator);
    assert_eq!(stage_seed.authorization, model_seed.authorization);
    assert_eq!(stage_seed.budget_scope_id, model_seed.budget_scope_id);
    assert_eq!(
        stage_seed.attempt, model_seed.attempt,
        "both derive attempt as state.retry.attempts + 1"
    );
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[test]
fn stage_dispatch_seed_is_none_before_a_run_is_accepted() {
    let coordinator = CommitCoordinator::new(Arc::new(FakeStore::new(FakeMode::Normal)));
    assert!(coordinator.stage_dispatch_seed().is_none());
}

/// `ModelDispatchSeed::relation_depth` must carry the accepted run's real
/// relation depth, the same value threaded through
/// `RunCallContext::relation_depth` on model dispatch. The accepted run
/// here is a `ChildAgent` at depth 1, not a root at depth 0, so a stub `0`
/// would fail this assertion instead of passing it by coincidence.
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[test]
fn pending_model_seed_carries_the_accepted_runs_relation_depth() {
    let mut coordinator = CommitCoordinator::new(Arc::new(FakeStore::new(FakeMode::Normal)));
    block_on(drive_child_to_model_request(&mut coordinator));

    let model_seed = coordinator
        .pending_model_seed()
        .expect("pending model seed");

    assert_eq!(
        model_seed.relation_depth, 1,
        "the model dispatch seed must carry the accepted child run's own relation depth"
    );
    assert_eq!(
        model_seed.relation_depth,
        coordinator.accepted_relation_depth(),
        "the seed's relation_depth must match the coordinator's own accepted depth lookup"
    );
}
