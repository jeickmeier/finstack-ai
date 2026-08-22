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
        if code.as_ref() == crate::ports::middleware::MIDDLEWARE_STAGE_UNLANDABLE));
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
            if code.as_ref() == crate::ports::middleware::MIDDLEWARE_STAGE_UNLANDABLE));
    }
}

#[test]
fn the_tool_batch_chain_is_skipped_entirely_when_no_component_is_registered() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    let driver = driver_for(
        "fixture.prepare-only",
        Stage::PrepareContext,
        StageOutcome::AddContext(Arc::from([item("never-runs")])),
    );
    let cursor = StageCursor {
        cycle: 0,
        stage: Stage::BeforeToolBatch,
    };

    let catalog = crate::ports::tool::ResolvedToolCatalog::try_new(
        [],
        &std::collections::BTreeMap::new(),
        &crate::ports::tool::JsonSchemaToolValidatorCompiler,
    )
    .expect("empty catalog");
    let sources = test_sources();

    for driver in [None, Some(&driver)] {
        let policy = block_on(run_tool_batch_chain(
            &mut coordinator,
            &catalog,
            driver,
            &sources,
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

// ---- Verification bounce lands end to end -----------------------------

fn verify_model_response(text: &str) -> crate::ports::model::ModelResponse {
    crate::ports::model::ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new(text).expect("text"))]),
        tool_calls: Arc::from([]),
        usage: finstack_ai_kernel::Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("completion-verify"),
        continuation_state: None,
    }
}

fn verify_model_completion(
    effect_id: finstack_ai_kernel::Id<finstack_ai_kernel::EffectTag>,
    text: &str,
) -> finstack_ai_kernel::EffectCompleted {
    let response = verify_model_response(text);
    let output = RawJson::parse(
        serde_json_canonicalizer::to_vec(&response)
            .expect("response json")
            .as_slice(),
    )
    .expect("canonical response");
    finstack_ai_kernel::EffectCompleted::try_new(
        effect_id,
        model_output_contract(),
        output,
        Some(finstack_ai_kernel::Usage::empty()),
        vec![],
        ProviderIds::empty(),
        Some("completion-verify"),
        None,
    )
    .expect("completion")
}

/// Drive an accepted coordinator all the way to `BeforeFinalize` with a
/// `Completed` terminal candidate over the assistant message `504`, mirroring
/// the kernel harness `drive_to_before_finalize_with_limits`
/// (`model_only_reducer/termination/verification_retry.rs`).
fn drive_to_before_finalize(coordinator: &mut CommitCoordinator) -> Message {
    drive_to_before_model(coordinator);
    let draft = request_draft(vec![user_message(4, "hi")], Vec::new());
    block_on(coordinator.submit(
        before_model_env(),
        KernelInput::StageSettled(model_request_settled(&draft)),
    ))
    .expect("model request settles");

    // `validate_assistant_semantics` requires `message.created_at() ==
    // env.now`, so this cannot reuse the fixture `assistant_message` helper,
    // which is pinned to `timestamp(900)`.
    let assistant = Message::try_new(
        id(504),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new("final answer").expect("text"),
        )],
        timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant message");
    block_on(coordinator.submit(
        env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[504], 105),
        KernelInput::ModelSettled(finstack_ai_kernel::ModelSettled {
            turn_id: id(101),
            model_request_id: id(102),
            outcome: finstack_ai_kernel::ModelSettlement::Completed {
                completion: verify_model_completion(id(103), "final answer"),
                assistant_message: assistant.clone(),
            },
        }),
    ))
    .expect("model settled");

    block_on(coordinator.submit(
        env(1_450, &[9], &[], &[], &[], &[], &[], 106),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::AfterModel,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    ))
    .expect("after model settles");

    assert!(
        matches!(
            coordinator.state().terminal_candidate,
            Some(finstack_ai_kernel::TerminalCandidate::Completed { .. })
        ),
        "the drive must land a Completed candidate before BeforeFinalize: {:?}",
        coordinator.state().terminal_candidate
    );
    assistant
}

/// End to end through the choke point: a `finstack.middleware.verify`
/// component bounces a `Completed` candidate at `BeforeFinalize` with a
/// framework-classified retry. The kernel side of this is pinned in
/// `crates/finstack-ai-kernel/tests/model_only_reducer/termination/verification_retry.rs`;
/// this test is the promotion claim that the same bounce lands through the
/// runtime's own choke point, `settle_facade_stage`, and that the run
/// actually re-enters `PreparingContext` on cycle `n + 1` once the retry
/// timer fires — with the bounced assistant message still in
/// `state.messages`, which is what Task 5's feedback design depends on.
#[test]
fn a_framework_verifier_bounce_at_before_finalize_lands_end_to_end() {
    let mut coordinator = accepted_coordinator_with_dispatcher(RunLimits {
        max_retries: Some(3),
        ..RunLimits::empty()
    });
    let bounced_assistant_message = drive_to_before_finalize(&mut coordinator);
    let sources = test_sources();

    let retry = finstack_ai_kernel::RetryDirective::try_new(
        finstack_ai_kernel::RetryClassification::Framework,
        finstack_ai_kernel::Duration::from_millis(1),
        "verify-policy-v1",
    )
    .expect("framework retry directive");
    let driver = driver_for(
        "finstack.middleware.verify",
        Stage::BeforeFinalize,
        StageOutcome::Retry(retry),
    );

    let outcome = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        env(1_500, &[13, 14], &[7], &[], &[], &[], &[], 108),
        StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::FinalizeAccepted,
        },
    ))
    .expect("a framework verifier bounce must commit over a Completed candidate");

    // ---- Step 1: the bounce lands as a retry, not a terminal -----------
    assert!(
        coordinator.state().terminal.is_none(),
        "a framework verifier bounce must not terminate the run: {:?}",
        coordinator.state().terminal
    );
    let pending = coordinator
        .state()
        .retry
        .pending
        .clone()
        .expect("the bounce must leave a pending retry");
    assert_eq!(
        pending.classification,
        finstack_ai_kernel::RetryClassification::Framework
    );

    let committed = outcome.committed.as_ref().expect("committed batch");
    let scheduled = committed
        .records
        .iter()
        .find_map(|record| match record.body() {
            finstack_ai_kernel::RecordBody::RetryScheduled(scheduled) => Some(scheduled),
            _ => None,
        })
        .expect("a RetryScheduled record must land in the journal");
    assert_eq!(
        scheduled.classification,
        finstack_ai_kernel::RetryClassification::Framework
    );
    assert_eq!(
        scheduled.prior_error.code.as_str(),
        "candidate_rejected",
        "a framework verifier bounce over a Completed candidate must carry the kernel's own error"
    );

    // ---- Step 2: settle the timer and re-enter PreparingContext --------
    let due_at = pending.due_at;
    block_on(coordinator.submit(
        env(1_501, &[15], &[], &[], &[], &[], &[], 109),
        KernelInput::TimerFired(finstack_ai_kernel::TimerFiredInput {
            effect_id: pending.timer_effect_id,
            due_at,
            fired_at: due_at,
        }),
    ))
    .expect("the retry timer fires");

    assert_eq!(
        coordinator.state().cycle,
        1,
        "the fired timer must open cycle n + 1"
    );
    assert_eq!(
        coordinator.state().phase,
        Some(RunPhase::PreparingContext),
        "the run must re-enter PreparingContext once the retry timer fires"
    );
    assert!(
        coordinator
            .state()
            .messages
            .iter()
            .any(|message| message.id() == bounced_assistant_message.id()),
        "the bounced assistant message must still be in state.messages on cycle n + 1"
    );
}
