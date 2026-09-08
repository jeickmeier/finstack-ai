// ---- passthrough invariant -------------------------------------------

#[test]
fn no_chain_installed_submits_the_facade_input_unchanged() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    let sources = test_sources();
    let facade_env = env(1_100, &[2], &[], &[], &[], &[], &[], 102);

    let outcome = block_on(settle_facade_stage(
        &mut coordinator,
        None,
        &sources,
        &test_profile(),
        facade_env,
        before_run_settled(),
    ))
    .expect("passthrough");

    assert_eq!(
        committed_record_ids(&outcome),
        vec![id(2)],
        "passthrough must commit the facade's own pre-minted record id"
    );
}

#[test]
fn inactive_stage_submits_the_facade_input_unchanged() {
    // The facade installs the chain unconditionally, so `middleware_chain()`
    // is always Some in a facade-started run. Passthrough must key off
    // `is_active(stage)`, not off the Option.
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    let sources = test_sources();
    let driver = driver_for(
        "fixture.prepare-only",
        Stage::PrepareContext,
        StageOutcome::AddContext(Arc::from([item("never-runs")])),
    );
    assert!(!driver.is_active(Stage::BeforeRun));

    let outcome = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect("passthrough");

    assert_eq!(
        committed_record_ids(&outcome),
        vec![id(2)],
        "an inactive stage must reuse the facade's env byte for byte"
    );
}

#[test]
fn identity_fold_submits_the_facade_input_unchanged() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    let sources = test_sources();
    let driver = driver_for("fixture.passive", Stage::BeforeRun, StageOutcome::Continue);
    assert!(driver.is_active(Stage::BeforeRun));

    let outcome = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect("identity fold");

    assert_eq!(
        committed_record_ids(&outcome),
        vec![id(2)],
        "an all-Continue chain must not perturb the facade's submission"
    );
}

/// Identity/empty-chain passthrough reuses the facade bag only when that bag
/// is the one `Kernel::decide` will consume. Once `now` is past the run
/// deadline, `decide_limit` intercepts and demands the mutually exclusive
/// `(2, 2, …)` bag. Without the shared fallback a short operational timeout
/// fails with `allocated_ids_exhausted` instead of committing
/// `deadline_exceeded`.
#[test]
fn passthrough_still_lands_when_the_deadline_has_already_elapsed() {
    let mut coordinator =
        accepted_coordinator_with_deadline(RunLimits::empty(), Some(timestamp(1_001)));
    let sources = test_sources();

    block_on(settle_facade_stage(
        &mut coordinator,
        None,
        &sources,
        &test_profile(),
        env(2_000, &[2], &[], &[], &[], &[], &[], 102),
        before_run_settled(),
    ))
    .expect("deadline crossing must still commit");

    let Some(finstack_ai_kernel::TerminalState::Failed(failed)) = coordinator.state().terminal()
    else {
        panic!(
            "expected deadline failure, got {:?}",
            coordinator.state().terminal()
        );
    };
    assert_eq!(failed.error.code.as_str(), "deadline_exceeded");
}

/// `BeforeModel` now folds, so its passthrough is no longer a stage-level
/// exclusion — it is the ordinary identity/inactive path, and it must still
/// reuse the facade's pre-minted ids byte for byte.
#[test]
fn before_model_passthrough_still_reuses_the_facade_ids() {
    for outcome in [
        // No component registered at BeforeModel at all.
        StageOutcome::AddContext(Arc::from([item("never-runs")])),
        // A component that runs and contributes nothing.
        StageOutcome::Continue,
    ] {
        let mut coordinator = accepted_coordinator(RunLimits::empty());
        drive_to_before_model(&mut coordinator);
        let sources = test_sources();
        let inactive = matches!(outcome, StageOutcome::AddContext(_));
        let driver = driver_for(
            "fixture.model",
            if inactive {
                Stage::PrepareContext
            } else {
                Stage::BeforeModel
            },
            outcome,
        );
        let draft = request_draft(vec![user_message(4, "hi")], Vec::new());

        let committed = block_on(settle_facade_stage(
            &mut coordinator,
            Some(&driver),
            &sources,
            &test_profile(),
            before_model_env(),
            model_request_settled(&draft),
        ))
        .expect("passthrough");

        assert_eq!(
            committed_record_ids(&committed),
            vec![id(5), id(6)],
            "a BeforeModel passthrough must reuse the facade's env byte for byte"
        );
        assert_eq!(
            committed_model_request(&coordinator),
            draft,
            "a passthrough must not perturb the facade's own model draft"
        );
    }
}

/// `Fail` is the *other* outcome the kernel admits at `BeforeModel`
/// (`decide.rs:1165-1177`), and it carries no draft. Folding it is
/// impossible, so it must land exactly as it would with no middleware
/// installed — otherwise registering a component at the stage, even one
/// that only observes, would stop a deliberate stage failure from
/// settling at all.
#[test]
fn a_fail_settled_at_before_model_passes_through_the_chain() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let driver = driver_for(
        "fixture.model",
        Stage::BeforeModel,
        StageOutcome::AddContext(Arc::from([item("must-not-run")])),
    );
    assert!(driver.is_active(Stage::BeforeModel));

    let committed = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        // `Fail` at a non-BeforeFinalize stage needs one record and
        // nothing else (`decide.rs:1165-1177`).
        env(1_300, &[5], &[], &[], &[], &[], &[], 104),
        StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeModel,
            },
            outcome: ReducerStageOutcome::Fail(
                ErrorDescriptor::new(
                    "fixture_before_model_failure",
                    "fixture",
                    ErrorCategory::Middleware,
                    false,
                )
                .expect("descriptor"),
            ),
        },
    ))
    .expect("a Fail at BeforeModel must still settle with a component registered");

    assert_eq!(
        committed_record_ids(&committed),
        vec![id(5)],
        "the failure must reuse the facade's own pre-minted record id"
    );
    assert!(
        coordinator.state().pending_model_effect().is_none(),
        "a failed BeforeModel must not have opened a model effect"
    );
}
