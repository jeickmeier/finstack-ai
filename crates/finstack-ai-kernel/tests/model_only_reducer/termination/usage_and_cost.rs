#[test]
fn missing_cost_usage_obeys_fail_closed_and_suspend_policies() {
    for (policy, expected_phase, expected_kind) in [
        (
            finstack_ai_kernel::UnknownUsagePolicy::FailClosed,
            RunPhase::Failed,
            "run_failed",
        ),
        (
            finstack_ai_kernel::UnknownUsagePolicy::SuspendForDecision,
            RunPhase::Suspended,
            "run_suspended",
        ),
        (
            finstack_ai_kernel::UnknownUsagePolicy::AllowWithinReservedMaximum,
            RunPhase::AfterModel,
            "effect_completed",
        ),
    ] {
        let mut limits = RunLimits::empty();
        limits.max_cost = Some(
            finstack_ai_kernel::CostLimit::try_new("USD", 1_000_000, "prices-v1", policy)
                .expect("cost limit"),
        );
        let mut harness = Harness::default();
        accept_with(&mut harness, limits, None);
        settle_before_run(&mut harness);
        prepare_context(&mut harness, 0, false);
        request_model(&mut harness, 0, false);
        let decision = harness.apply_input(
            if policy == finstack_ai_kernel::UnknownUsagePolicy::AllowWithinReservedMaximum {
                transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE])
            } else {
                transition_env(1_400, &[7], &[3], &[], &[], &[], &[])
            },
            completed_input(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "missing-cost",
                "hello",
            ),
        );
        assert_eq!(harness.kernel.state().phase, Some(expected_phase));
        assert_eq!(decision.records[0].body().kind_name(), expected_kind);
        assert!(decision.actions.is_empty());
        if policy == finstack_ai_kernel::UnknownUsagePolicy::AllowWithinReservedMaximum {
            assert!(harness.kernel.state().limit_usage.cost.is_none());
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the checked-overflow scenario requires two complete semantic model cycles"
)]
fn malformed_usage_and_checked_cost_overflow_fail_durably() {
    let cases = [
        (
            finstack_ai_kernel::Usage::try_new(
                None,
                None,
                None,
                Some(finstack_ai_kernel::CostAmount::try_new("EUR", 1, "prices-v1").expect("cost")),
                BTreeMap::new(),
            )
            .expect("usage"),
            "cost_policy_mismatch",
        ),
        (
            finstack_ai_kernel::Usage::try_new(
                None,
                None,
                None,
                Some(finstack_ai_kernel::CostAmount::try_new("USD", 0, "prices-v1").expect("cost")),
                BTreeMap::from([(
                    finstack_ai_kernel::LimitKey::parse("vendor.unregistered").expect("limit key"),
                    1,
                )]),
            )
            .expect("usage"),
            "unregistered_extension_counter",
        ),
    ];

    for (usage, expected_code) in cases {
        let mut limits = RunLimits::empty();
        limits.max_cost = Some(
            finstack_ai_kernel::CostLimit::try_new(
                "USD",
                u64::MAX,
                "prices-v1",
                finstack_ai_kernel::UnknownUsagePolicy::FailClosed,
            )
            .expect("cost limit"),
        );
        let mut harness = Harness::default();
        accept_with(&mut harness, limits, None);
        settle_before_run(&mut harness);
        prepare_context(&mut harness, 0, false);
        request_model(&mut harness, 0, false);
        let decision = harness.apply_input(
            transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
            completed_input_with_usage(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                expected_code,
                usage,
            ),
        );
        let RecordBody::RunFailed(failed) = decision.records[0].body() else {
            panic!("usage policy failure must be durable");
        };
        assert_eq!(failed.error.code.as_str(), expected_code);
        assert_eq!(failed.error.category, ErrorCategory::Limit);
        assert!(!failed.error.retryable);
    }

    let mut limits = RunLimits::empty();
    limits.max_cost = Some(
        finstack_ai_kernel::CostLimit::try_new(
            "USD",
            u64::MAX,
            "prices-v1",
            finstack_ai_kernel::UnknownUsagePolicy::FailClosed,
        )
        .expect("cost limit"),
    );
    let mut harness = Harness::default();
    accept_with(&mut harness, limits, None);
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    request_model(&mut harness, 0, false);
    let maximum_usage = finstack_ai_kernel::Usage::try_new(
        None,
        None,
        None,
        Some(finstack_ai_kernel::CostAmount::try_new("USD", u64::MAX, "prices-v1").expect("cost")),
        BTreeMap::new(),
    )
    .expect("usage");
    harness.apply_input(
        transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        completed_input_with_usage(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_400,
            "maximum-cost",
            maximum_usage,
        ),
    );
    settle_after_model(&mut harness, 0, false);
    harness.apply_input(
        transition_env(2_000, &[12], &[], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::ContinueModel { reason: None },
        ),
    );
    prepare_context(&mut harness, 1, true);
    request_model(&mut harness, 1, true);
    let one_more = finstack_ai_kernel::Usage::try_new(
        None,
        None,
        None,
        Some(finstack_ai_kernel::CostAmount::try_new("USD", 1, "prices-v1").expect("cost")),
        BTreeMap::new(),
    )
    .expect("usage");
    let overflow = harness.apply_input(
        transition_env(2_300, &[17], &[7], &[], &[], &[], &[]),
        completed_input_with_usage(
            TURN_TWO,
            MODEL_REQUEST_TWO,
            EFFECT_TWO,
            FINAL_MESSAGE_TWO,
            2_300,
            "cost-overflow",
            one_more,
        ),
    );
    assert_eq!(overflow.records.len(), 1);
    let RecordBody::RunFailed(failed) = overflow.records[0].body() else {
        panic!("cost overflow must be durable");
    };
    assert_eq!(failed.error.code.as_str(), "cost_overflow");
    assert!(
        !overflow
            .records
            .iter()
            .any(|record| matches!(record.body(), RecordBody::LimitReached(_)))
    );
}

#[test]
fn checked_extension_counter_overflow_fails_without_limit_reached() {
    let key = finstack_ai_kernel::LimitKey::parse("app.calls").expect("limit key");
    let mut limits = RunLimits::empty();
    limits.extension_counters.insert(key.clone(), u64::MAX);
    let mut harness = drive_to_awaiting_model_with_limits(limits);
    let maximum = finstack_ai_kernel::Usage::try_new(
        None,
        None,
        None,
        None,
        BTreeMap::from([(key.clone(), u64::MAX)]),
    )
    .expect("usage");
    harness.apply_input(
        transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        completed_input_with_usage(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_400,
            "maximum-counter",
            maximum,
        ),
    );
    settle_after_model(&mut harness, 0, false);
    harness.apply_input(
        transition_env(2_000, &[12], &[], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::ContinueModel { reason: None },
        ),
    );
    prepare_context(&mut harness, 1, true);
    request_model(&mut harness, 1, true);
    let one_more =
        finstack_ai_kernel::Usage::try_new(None, None, None, None, BTreeMap::from([(key, 1)]))
            .expect("usage");
    let overflow = harness.apply_input(
        transition_env(2_300, &[17], &[7], &[], &[], &[], &[]),
        completed_input_with_usage(
            TURN_TWO,
            MODEL_REQUEST_TWO,
            EFFECT_TWO,
            FINAL_MESSAGE_TWO,
            2_300,
            "counter-overflow",
            one_more,
        ),
    );
    let RecordBody::RunFailed(failed) = overflow.records[0].body() else {
        panic!("counter overflow must be durable");
    };
    assert_eq!(failed.error.code.as_str(), "counter_overflow");
    assert_eq!(failed.error.category, ErrorCategory::Limit);
    assert!(!failed.error.retryable);
    assert!(
        !overflow
            .records
            .iter()
            .any(|record| matches!(record.body(), RecordBody::LimitReached(_)))
    );
}
