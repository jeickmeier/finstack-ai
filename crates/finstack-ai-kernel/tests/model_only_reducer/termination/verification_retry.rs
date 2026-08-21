fn drive_to_before_finalize_with_limits(limits: RunLimits) -> Harness {
    let mut harness = Harness::default();
    accept_with(&mut harness, limits, None);
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    request_model(&mut harness, 0, false);
    complete_model(&mut harness, false, "completion-verify");
    settle_after_model(&mut harness, 0, false);
    harness
}

#[test]
fn framework_retry_admits_over_completed_candidate() {
    let mut limits = RunLimits::empty();
    limits.max_retries = Some(3);
    let mut harness = drive_to_before_finalize_with_limits(limits);
    assert!(matches!(
        harness.kernel.state().terminal_candidate,
        Some(TerminalCandidate::Completed { .. })
    ));

    let retry = finstack_ai_kernel::RetryDirective::try_new(
        finstack_ai_kernel::RetryClassification::Framework,
        finstack_ai_kernel::Duration::from_millis(250),
        "verify-policy-v1",
    )
    .expect("framework retry directive");
    let decision = harness.apply_input(
        transition_env(1_700, &[12, 13, 14], &[6], &[601], &[], &[], &[]),
        stage_input(0, Stage::BeforeFinalize, ReducerStageOutcome::Retry(retry)),
    );

    let RecordBody::RetryScheduled(scheduled) = decision.records[1].body() else {
        panic!("expected retry scheduled record");
    };
    assert_eq!(
        scheduled.classification,
        finstack_ai_kernel::RetryClassification::Framework
    );
    assert_eq!(scheduled.attempt, 1);
    assert_eq!(scheduled.prior_error.code.as_str(), "candidate_rejected");
    assert!(matches!(
        decision.records[2].body(),
        RecordBody::EffectRequested(_)
    ));
    assert_eq!(harness.kernel.state().retry.attempts, 1);

    let encoded = serde_json::to_value(scheduled).expect("serialize retry schedule");
    assert_eq!(encoded["classification"], "framework");
    let decoded: finstack_ai_kernel::RetryScheduled =
        serde_json::from_value(encoded.clone()).expect("deserialize framework retry schedule");
    assert_eq!(&decoded, scheduled);

    let mut unsupported = encoded;
    unsupported["classification"] = serde_json::json!("verification");
    let error = serde_json::from_value::<finstack_ai_kernel::RetryScheduled>(unsupported)
        .expect_err("post-v1 verification classification must fail closed");
    assert!(error.to_string().contains("unknown variant"));
}

#[test]
fn retry_guard_matrix_still_holds_and_admits_failed_candidates() {
    // A completed candidate only admits the framework family. The other
    // candidate-v1 families retain their existing prerequisites.
    let mut limits = RunLimits::empty();
    limits.max_retries = Some(3);
    let completed_harness = drive_to_before_finalize_with_limits(limits);
    for classification in [
        finstack_ai_kernel::RetryClassification::Model,
        finstack_ai_kernel::RetryClassification::Tool,
        finstack_ai_kernel::RetryClassification::Validation,
    ] {
        let retry = finstack_ai_kernel::RetryDirective::try_new(
            classification,
            finstack_ai_kernel::Duration::from_millis(250),
            "verify-policy-v1",
        )
        .expect("retry directive");
        let result = completed_harness.kernel.decide(
            &transition_env(1_700, &[12, 13, 14], &[6], &[601], &[], &[], &[]),
            stage_input(0, Stage::BeforeFinalize, ReducerStageOutcome::Retry(retry)),
        );
        assert!(matches!(
            result,
            Err(KernelError::InvalidPhaseInput {
                input: "stage_settled",
                ..
            })
        ));
    }

    // Framework retries over a failed candidate still reuse the candidate's
    // original retryable error exactly.
    let mut failed_limits = RunLimits::empty();
    failed_limits.max_retries = Some(3);
    let mut failed_harness = drive_to_awaiting_model_with_limits(failed_limits);
    let error = ErrorDescriptor::new(
        "provider_retry",
        "retryable provider failure",
        ErrorCategory::Model,
        true,
    )
    .expect("retryable error");
    let expected_error = error.clone();
    failed_harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
            model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
            outcome: ModelSettlement::Failed(
                EffectFailed::try_new(
                    id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
                    output_contract(),
                    error,
                    None,
                    Some("provider-retry"),
                )
                .expect("failed effect"),
            ),
        }),
    );
    assert!(matches!(
        failed_harness.kernel.state().terminal_candidate,
        Some(TerminalCandidate::Failed { .. })
    ));
    let framework_retry = finstack_ai_kernel::RetryDirective::try_new(
        finstack_ai_kernel::RetryClassification::Framework,
        finstack_ai_kernel::Duration::from_millis(250),
        "verify-policy-v1",
    )
    .expect("framework retry directive");
    let decision = failed_harness.apply_input(
        transition_env(1_500, &[8, 9, 10], &[4], &[701], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::Retry(framework_retry),
        ),
    );
    let RecordBody::RetryScheduled(scheduled) = decision.records[1].body() else {
        panic!("expected retry scheduled record");
    };
    assert_eq!(
        scheduled.classification,
        finstack_ai_kernel::RetryClassification::Framework
    );
    assert_eq!(scheduled.prior_error, expected_error);
    assert_eq!(failed_harness.kernel.state().retry.attempts, 1);
}
