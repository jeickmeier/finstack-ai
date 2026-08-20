// ---- terminal folds ---------------------------------------------------

#[test]
fn fail_terminal_replaces_the_base_outcome_at_after_model() {
    let fold = StageFold {
        terminal: Some(StageTerminal::Fail(Box::new(
            ErrorDescriptor::new("boom", "fixture", ErrorCategory::Middleware, false)
                .expect("descriptor"),
        ))),
        ..StageFold::default()
    };
    let sources = test_sources();

    let outcome = apply_fold(
        &fold,
        StageCursor {
            cycle: 0,
            stage: Stage::AfterModel,
        },
        ReducerStageOutcome::Continue,
        &sources,
    )
    .expect("applied");

    assert!(matches!(outcome, ReducerStageOutcome::Fail(ref d) if d.code.as_str() == "boom"));
}

#[test]
fn a_non_terminal_fold_with_no_landing_is_rejected_not_dropped() {
    // FilterTools is port-legal at BeforeToolBatch and BeforeModel only, so
    // it can never reach `apply_fold` through the facade choke point — but
    // `run_stage_chain` is shared with the tool-batch hook, so the applier
    // must refuse a fold it cannot land rather than discard it.
    let fold = StageFold {
        retained_tools: Some(std::collections::BTreeSet::new()),
        ..StageFold::default()
    };
    let sources = test_sources();

    let error = apply_fold(
        &fold,
        StageCursor {
            cycle: 0,
            stage: Stage::AfterModel,
        },
        ReducerStageOutcome::Continue,
        &sources,
    )
    .expect_err("nothing at AfterModel can carry a tool filter");
    assert!(matches!(&error, RunHandleError::Middleware { code }
        if code.as_ref() == crate::middleware_driver::MIDDLEWARE_STAGE_UNLANDABLE));
}

#[test]
fn an_identity_fold_returns_the_base_outcome_untouched() {
    let sources = test_sources();
    let outcome = apply_fold(
        &StageFold::default(),
        StageCursor {
            cycle: 0,
            stage: Stage::AfterToolBatch,
        },
        ReducerStageOutcome::Continue,
        &sources,
    )
    .expect("identity");
    assert_eq!(outcome, ReducerStageOutcome::Continue);
}

// ---- BeforeToolBatch fold reduction ----------------------------------

#[test]
fn an_empty_tool_batch_fold_is_unchanged_not_an_empty_retain_set() {
    // `Unchanged` and `Retain({})` are not the same instruction: the first
    // plans the batch exactly as an un-middlewared run would, the second
    // denies every call. Collapsing them would turn a chain of pure
    // observers into a total tool ban.
    assert_eq!(
        tool_batch_policy(&StageFold::default()).expect("identity"),
        ToolBatchPolicy::Unchanged
    );
    assert_eq!(
        tool_batch_policy(&StageFold {
            retained_tools: Some(std::collections::BTreeSet::new()),
            ..StageFold::default()
        })
        .expect("empty retain"),
        ToolBatchPolicy::Retain(std::collections::BTreeSet::new())
    );
}

#[test]
fn a_tool_batch_fail_terminal_becomes_the_stage_failure() {
    let policy = tool_batch_policy(&StageFold {
        terminal: Some(StageTerminal::Fail(Box::new(
            ErrorDescriptor::new("boom", "fixture", ErrorCategory::Middleware, false)
                .expect("descriptor"),
        ))),
        // A terminal wins over a narrowing produced earlier in the chain.
        retained_tools: Some(std::collections::BTreeSet::new()),
        ..StageFold::default()
    })
    .expect("fail terminal");
    assert!(
        matches!(policy, ToolBatchPolicy::Fail(descriptor) if descriptor.code.as_str() == "boom")
    );
}

#[test]
fn a_tool_batch_fold_with_no_policy_expression_is_rejected_not_dropped() {
    // Neither case is reachable through the port — `validate_stage_outcome`
    // refuses AddContext at BeforeToolBatch and `StageFold::accumulate`
    // refuses Replace there — but the reducer must still refuse a fold it
    // cannot express rather than silently discard the contribution.
    for fold in [
        StageFold {
            context: vec![item("nowhere-to-land")],
            ..StageFold::default()
        },
        StageFold {
            replacement: Some(RawJson::parse(b"[]").expect("replacement")),
            ..StageFold::default()
        },
        StageFold {
            terminal: Some(StageTerminal::Retry(
                finstack_ai_kernel::RetryDirective::try_new(
                    finstack_ai_kernel::RetryClassification::Framework,
                    finstack_ai_kernel::Duration::from_millis(1),
                    "fixture-policy-v1",
                )
                .expect("directive"),
            )),
            ..StageFold::default()
        },
    ] {
        let error =
            tool_batch_policy(&fold).expect_err("a BeforeToolBatch fold has nowhere to land this");
        assert!(matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == crate::middleware_driver::MIDDLEWARE_STAGE_UNLANDABLE));
    }
}

#[test]
fn the_tool_batch_chain_is_skipped_entirely_when_no_component_is_registered() {
    let coordinator = accepted_coordinator(RunLimits::empty());
    let driver = driver_for(
        "fixture.prepare-only",
        Stage::PrepareContext,
        StageOutcome::AddContext(Arc::from([item("never-runs")])),
    );
    let cursor = StageCursor {
        cycle: 0,
        stage: Stage::BeforeToolBatch,
    };

    let catalog = crate::ResolvedToolCatalog::try_new(
        [],
        &std::collections::BTreeMap::new(),
        &crate::JsonSchemaToolValidatorCompiler,
    )
    .expect("empty catalog");

    for driver in [None, Some(&driver)] {
        let policy = block_on(run_tool_batch_chain(
            &coordinator,
            &catalog,
            driver,
            cursor,
            &[],
        ))
        .expect("passthrough");
        assert_eq!(policy, ToolBatchPolicy::Unchanged);
    }
}

// ---- decide_limit interception ---------------------------------------

/// The stage table and `decide_limit` demand *mutually exclusive* id bags
/// once a limit is crossed, because `validate_allocated_ids`
/// (`allocated_ids.rs:97-107`) rejects both under- and over-allocation.
/// This pins that exclusivity directly against the kernel, so the two-shot
/// probe in `submit_folded` cannot be "simplified" into one lookup.
#[test]
fn the_stage_table_and_the_limit_requirement_are_mutually_exclusive() {
    let sources = test_sources();
    let state = finstack_ai_kernel::KernelState {
        session_id: Some(id::<SessionTag>(1)),
        lane_id: Some(id::<LaneTag>(2)),
        accepted: Some(acceptance(RunLimits {
            max_retries: Some(0),
            ..RunLimits::empty()
        })),
        accepted_at: Some(timestamp(1_000)),
        phase: Some(finstack_ai_kernel::RunPhase::BeforeFinalize),
        terminal_candidate: Some(finstack_ai_kernel::TerminalCandidate::Completed {
            cycle: 0,
            turn_id: id(20),
            model_request_id: id(21),
            effect_id: id(22),
            message_id: id(23),
            result_digest: Digest::raw_json(b"{}"),
        }),
        ..finstack_ai_kernel::KernelState::default()
    };
    let cursor = StageCursor {
        cycle: 0,
        stage: Stage::BeforeFinalize,
    };
    let outcome = ReducerStageOutcome::Retry(
        finstack_ai_kernel::RetryDirective::try_new(
            finstack_ai_kernel::RetryClassification::Framework,
            finstack_ai_kernel::Duration::from_millis(1),
            "fixture-policy-v1",
        )
        .expect("directive"),
    );
    let input = KernelInput::StageSettled(StageSettled {
        cursor,
        outcome: outcome.clone(),
    });

    let table = crate::settlement::stage_allocation(&state, cursor, &outcome, &sources)
        .expect("stage table allocation");
    assert_eq!(
        table.record_ids().len(),
        3,
        "Retry's stage tuple is (3,1,1)"
    );
    let rejected = finstack_ai_kernel::Kernel::try_restore(state.clone())
        .expect("restore")
        .decide(
            &TransitionEnv {
                now: timestamp(1_500),
                ids: table,
            },
            input.clone(),
        );
    assert!(
        rejected.is_err(),
        "the stage tuple must NOT satisfy decide_limit once max_retries is crossed: {rejected:?}"
    );

    let accepted = finstack_ai_kernel::Kernel::try_restore(state)
        .expect("restore")
        .decide(
            &TransitionEnv {
                now: timestamp(1_500),
                ids: limit_crossing_allocation(&sources).expect("limit allocation"),
            },
            input,
        );
    assert!(
        accepted.is_ok(),
        "the fixed limit-crossing bag must be the one that lands: {accepted:?}"
    );
}

/// End to end through the choke point: a `PrepareContext` fold submitted
/// against `max_turns = 0`. `decide_limit` increments `usage.turns` from
/// the submitted `ContextPrepared` and intercepts, so the stage table's
/// `(2, 0, 0, 1, 0, 0)` is rejected and the fallback bag has to carry the
/// settlement. Without the fallback this returns
/// `Coordinator(Decision { code: "unused_allocated_ids" })`.
#[test]
fn a_fold_that_crosses_a_limit_still_lands_through_the_choke_point() {
    let mut coordinator = accepted_coordinator(RunLimits {
        max_turns: Some(0),
        ..RunLimits::empty()
    });
    drive_to_prepare_context(&mut coordinator);
    let sources = test_sources();
    let driver = driver_for(
        "fixture.context",
        Stage::PrepareContext,
        StageOutcome::AddContext(Arc::from([item("pushes-context-over")])),
    );

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([user_message(4, "hi")]),
            },
        },
    ))
    .expect("the limit-crossing fold must still commit");

    assert!(
        matches!(
            coordinator.state().terminal,
            Some(finstack_ai_kernel::TerminalState::Failed(_))
        ),
        "decide_limit must have terminated the run, not been bypassed: {:?}",
        coordinator.state().terminal
    );
}
