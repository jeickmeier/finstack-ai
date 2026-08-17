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
