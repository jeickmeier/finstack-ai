// -- passthrough --------------------------------------------------------

#[test]
fn no_driver_plans_every_source_call_for_execution() {
    let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, None);
    assert_eq!(plans.len(), 3);
    assert!(
        plans
            .iter()
            .all(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_))),
        "with no chain every registered, schema-valid call must execute: {plans:?}"
    );
}

#[test]
fn a_chain_with_no_before_tool_batch_component_is_passthrough() {
    // The facade installs its chain unconditionally, so the driver is
    // almost always `Some`. Passthrough must key off `is_active`, and the
    // resulting plan must be byte-identical to the no-driver one.
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::AfterModel, retain(&[]), &calls);
    let (with_driver, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));
    let (without_driver, _c, _s) = prepare_with(&TOOL_NAMES, None);

    assert_eq!(
        with_driver, without_driver,
        "an inactive stage must not perturb the opened plan"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "no BeforeToolBatch component is registered, so none may run"
    );
}

#[test]
fn an_identity_fold_leaves_the_plan_unchanged() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, StageOutcome::Continue, &calls);
    let (with_driver, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));
    let (without_driver, _c, _s) = prepare_with(&TOOL_NAMES, None);

    assert_eq!(calls.load(Ordering::Relaxed), 1, "the component must run");
    assert_eq!(with_driver.len(), without_driver.len());
    for (durable, direct) in with_driver.iter().zip(&without_driver) {
        assert_eq!(durable.source_index, direct.source_index);
        assert_eq!(durable.group_index, direct.group_index);
        assert_eq!(durable.plan, direct.plan);
    }
}

// -- filtering ----------------------------------------------------------

#[test]
fn filtered_tool_call_becomes_a_synthetic_closure() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
    let (plans, _coordinator, _store) = prepare_with(&["alpha"], Some(&driver));

    assert_eq!(plans.len(), 1, "every source call must appear exactly once");
    assert!(
        matches!(plans[0].plan, ToolCallPlan::SyntheticClosure(_)),
        "a denied call must be a synthetic closure, got {:?}",
        plans[0].plan
    );
    assert_eq!(planned_call_ids(&plans), source_call_ids(1));
}

#[test]
fn fully_filtered_batch_still_commits_every_source_call() {
    // `prepare_tool_batch_if_ready` guards on the source `calls` being
    // empty, never on the plans, so denying every call still commits a
    // batch of all-SyntheticClosure plans rather than short-circuiting.
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
    let (plans, coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

    assert_eq!(plans.len(), 3);
    assert!(
        plans
            .iter()
            .all(|assigned| matches!(assigned.plan, ToolCallPlan::SyntheticClosure(_))),
        "every denied call must still be planned: {plans:?}"
    );
    assert_eq!(
        planned_call_ids(&plans),
        source_call_ids(3),
        "coverage is positional: same calls, same order, no duplicates"
    );
    assert!(
        coordinator.state().terminal.is_none(),
        "a fully filtered batch is not a run failure: {:?}",
        coordinator.state().terminal
    );
}

#[test]
fn partial_filter_denies_only_the_calls_outside_the_retained_set() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&["beta"]), &calls);
    let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

    let shapes = plans
        .iter()
        .map(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        shapes,
        vec![false, true, false],
        "only the retained tool may execute, and source order is preserved: {plans:?}"
    );
    assert_eq!(planned_call_ids(&plans), source_call_ids(3));
}

#[test]
fn source_order_survives_a_filter_that_denies_the_first_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&["gamma"]), &calls);
    let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

    let shapes = plans
        .iter()
        .map(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        shapes,
        vec![false, false, true],
        "denying the leading calls must not shift the surviving one forward: {plans:?}"
    );
    assert_eq!(
        plans
            .iter()
            .map(|assigned| assigned.source_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(planned_call_ids(&plans), source_call_ids(3));
}

// -- the coverage invariant itself --------------------------------------

#[test]
fn plan_coverage_rejects_a_short_reordered_or_duplicated_plan_array() {
    let alpha = tool_call(301, "alpha");
    let beta = tool_call(302, "beta");
    let plan = |call: &ToolCallBlock| {
        ToolCallPlan::SyntheticClosure(finstack_ai_kernel::SyntheticToolClosure {
            call: call.clone(),
            execution: ToolExecutionMode::Sequential,
            failure_policy: ToolFailurePolicy::ReturnToModel,
            error: ErrorDescriptor::new(
                "tool_policy_denied",
                "denied",
                ErrorCategory::Validation,
                false,
            )
            .expect("descriptor"),
        })
    };
    let source = vec![*alpha.tool_call_id(), *beta.tool_call_id()];

    assert_plan_coverage(&[plan(&alpha), plan(&beta)], &source).expect("exact cover");
    for (label, plans) in [
        ("short", vec![plan(&alpha)]),
        ("reordered", vec![plan(&beta), plan(&alpha)]),
        ("duplicated", vec![plan(&alpha), plan(&alpha)]),
        ("long", vec![plan(&alpha), plan(&beta), plan(&beta)]),
    ] {
        let Err(error) = assert_plan_coverage(&plans, &source) else {
            panic!("a {label} plan array must be rejected");
        };
        assert!(
            matches!(&error, RunHandleError::ToolSettlement { code }
                    if *code == TOOL_PLAN_COVERAGE_MISMATCH),
            "{label}: expected the reserved coverage code, got {error:?}"
        );
    }
}

// -- terminal folds and the deadline bypass -----------------------------

#[test]
fn a_middleware_failure_settles_the_stage_as_failed_instead_of_opening_a_batch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(
        Stage::BeforeToolBatch,
        StageOutcome::Fail(Box::new(
            ErrorDescriptor::new(
                "tool_batch_rejected",
                "fixture",
                ErrorCategory::Middleware,
                false,
            )
            .expect("descriptor"),
        )),
        &calls,
    );
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &TOOL_NAMES, None);
    let sources = sources_at(1_600);

    let opened = block_on(prepare_tool_batch_if_ready(
        &mut coordinator,
        &catalog(),
        &sources,
        Some(&driver),
    ))
    .expect("the failed stage still settles");

    assert!(!opened, "no batch may open when the chain fails the stage");
    assert!(
        store.opened_tool_batch().is_none(),
        "no ToolBatchOpened record may exist"
    );
    // `Fail` at a non-`BeforeFinalize` stage (`decide.rs:1165-1177`)
    // normalizes the stage as failed and drives the run to
    // `BeforeFinalize` carrying the component's own descriptor; the
    // facade's finalize settlement is what commits the terminal.
    assert_eq!(coordinator.state().phase, Some(RunPhase::BeforeFinalize));
    let Some(TerminalCandidate::Failed { error, .. }) =
        coordinator.state().terminal_candidate.as_ref()
    else {
        panic!(
            "the middleware Fail must become the terminal candidate: {:?}",
            coordinator.state().terminal_candidate
        );
    };
    assert_eq!(error.code.as_str(), "tool_batch_rejected");
}

#[test]
fn the_run_deadline_path_bypasses_the_chain_entirely() {
    // Fail closed: a run already out of budget must not spend more of it
    // on middleware.
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
    let store = Arc::new(MemoryStore::new());
    let mut coordinator =
        coordinator_at_before_tool_batch(&store, &TOOL_NAMES, Some(fixed_timestamp(1_550)));
    let sources = sources_at(1_600);

    let opened = block_on(prepare_tool_batch_if_ready(
        &mut coordinator,
        &catalog(),
        &sources,
        Some(&driver),
    ))
    .expect("the deadline path settles");

    assert!(!opened);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "the deadline path must not invoke any middleware component"
    );
    assert!(store.opened_tool_batch().is_none());
}
