use super::*;
use std::collections::BTreeMap;

fn accept_with(harness: &mut Harness, limits: RunLimits, deadline: Option<Timestamp>) {
    harness.apply_input(
        transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
        KernelInput::AcceptRun(AcceptRun {
            session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
            lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
            accepted: root_acceptance_with(limits, deadline),
        }),
    );
}

fn completed_input_with_usage(
    turn_ordinal: u64,
    request_ordinal: u64,
    effect_ordinal: u64,
    message_ordinal: u64,
    now_ms: i64,
    completion_id: &str,
    usage: finstack_ai_kernel::Usage,
) -> KernelInput {
    KernelInput::ModelSettled(ModelSettled {
        turn_id: id::<finstack_ai_kernel::TurnTag>(turn_ordinal),
        model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(request_ordinal),
        outcome: ModelSettlement::Completed {
            completion: EffectCompleted::try_new(
                id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
                output_contract(),
                RawJson::parse(r#"{"text":"hello"}"#).expect("model output"),
                Some(usage),
                vec![],
                provider_ids(),
                Some(completion_id),
                None,
            )
            .expect("completed model effect"),
            assistant_message: assistant_message(message_ordinal, now_ms, "hello"),
        },
    })
}

#[test]
fn cancellation_without_outstanding_effects_reconciles_to_cancelled() {
    let mut harness = Harness::default();
    accept(&mut harness);

    let decision = harness.apply_input(
        cancellation_env(1_100, &[2], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    assert!(decision.actions.is_empty());
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Cancelling));
    assert_eq!(harness.kernel.state().state_version, 3);

    harness.apply_input(
        transition_env(1_200, &[3, 4], &[2], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Cancelled));
    assert!(matches!(
        harness.kernel.state().terminal,
        Some(TerminalState::Cancelled(_))
    ));
    assert!(matches!(
        harness.events.last().map(RunEvent::kind),
        Some(RunEventKind::RunCancelled)
    ));
}

#[test]
fn retry_records_timer_intent_and_starts_a_fresh_cycle_after_firing() {
    let mut harness = drive_to_awaiting_model();
    let error = ErrorDescriptor::new(
        "provider_retry",
        "retryable provider failure",
        ErrorCategory::Model,
        true,
    )
    .expect("retryable error");
    harness.apply_input(
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

    let retry = finstack_ai_kernel::RetryDirective::try_new(
        finstack_ai_kernel::RetryClassification::Model,
        finstack_ai_kernel::Duration::from_millis(250),
        "retry-v1",
    )
    .expect("retry directive");
    let decision = harness.apply_input(
        transition_env(1_500, &[8, 9, 10], &[4], &[701], &[], &[], &[]),
        stage_input(0, Stage::BeforeFinalize, ReducerStageOutcome::Retry(retry)),
    );
    assert_eq!(
        decision.actions,
        vec![PostCommitAction::ExecuteEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(701),
        }]
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Sleeping));
    assert_eq!(harness.kernel.state().retry.attempts, 1);

    harness.apply_input(
        transition_env(1_750, &[11], &[], &[], &[], &[], &[]),
        KernelInput::TimerFired(finstack_ai_kernel::TimerFiredInput {
            effect_id: id::<finstack_ai_kernel::EffectTag>(701),
            due_at: timestamp(1_750),
            fired_at: timestamp(1_750),
        }),
    );
    assert_eq!(harness.kernel.state().cycle, 1);
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::PreparingContext)
    );
    assert!(harness.kernel.state().retry.pending.is_none());

    let mut replayed = Kernel::default();
    let mut transient_sequence = 0;
    for batch in &harness.batches {
        let events = replayed
            .apply(batch, transient_sequence)
            .expect("retry history replay");
        transient_sequence += u64::try_from(events.len()).expect("event count");
    }
    assert_eq!(replayed.state(), harness.kernel.state());
    assert_eq!(
        replayed.state().state_hash().expect("replay hash"),
        harness.kernel.state().state_hash().expect("live hash")
    );
    let encoded = serde_json::to_value(harness.kernel.state()).expect("state v3 JSON");
    let decoded: finstack_ai_kernel::KernelState =
        serde_json::from_value(encoded).expect("state v3 round trip");
    assert_eq!(decoded, *harness.kernel.state());
}

#[test]
fn model_request_limit_allows_equality_and_records_first_crossing() {
    let mut allowed = Harness::default();
    let mut limits = RunLimits::empty();
    limits.max_model_requests = Some(1);
    accept_with(&mut allowed, limits, None);
    settle_before_run(&mut allowed);
    prepare_context(&mut allowed, 0, false);
    request_model(&mut allowed, 0, false);
    assert_eq!(allowed.kernel.state().limit_usage.model_requests, 1);
    assert_eq!(allowed.kernel.state().phase, Some(RunPhase::AwaitingModel));

    let mut crossed = Harness::default();
    let mut limits = RunLimits::empty();
    limits.max_model_requests = Some(0);
    accept_with(&mut crossed, limits, None);
    settle_before_run(&mut crossed);
    prepare_context(&mut crossed, 0, false);
    crossed.apply_input(
        transition_env(1_300, &[5, 6], &[2, 3], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeModel,
            ReducerStageOutcome::ModelRequestPrepared {
                request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
                component: None,
                output_contract: output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
        ),
    );
    assert_eq!(crossed.kernel.state().phase, Some(RunPhase::Failed));
    assert!(matches!(
        crossed.batches.last().unwrap().records[0].body(),
        RecordBody::LimitReached(_)
    ));
    assert_eq!(crossed.events.len(), 3);
}

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
            transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
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
    }
}

#[test]
fn active_model_cancellation_is_idempotent_and_uncertainty_suspends() {
    let mut harness = drive_to_awaiting_model();
    let cancel = KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
        initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
        reason: Some(Arc::from("shutdown")),
    });
    let decision = harness.apply_input(cancellation_env(1_350, &[7], &[], &[700]), cancel.clone());
    assert_eq!(
        decision.actions,
        vec![PostCommitAction::CancelEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
        }]
    );
    let duplicate = harness
        .kernel
        .decide(&empty_env(1_351), cancel)
        .expect("equal cancellation duplicate");
    assert!(duplicate.records.is_empty());

    harness.apply_input(
        transition_env(1_400, &[8, 9], &[3], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)]),
        }),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Suspended));
    assert_eq!(
        harness
            .kernel
            .state()
            .suspension
            .as_ref()
            .expect("suspension")
            .reason_code
            .as_str(),
        "cancellation_uncertain"
    );
}

#[test]
fn reconciled_model_cancellation_records_effect_closure_before_run_cancelled() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        cancellation_env(1_350, &[7], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    let decision = harness.apply_input(
        transition_env(1_400, &[8, 9, 10], &[3, 4], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert!(matches!(
        decision.records[0].body(),
        RecordBody::EffectCancelled(_)
    ));
    assert!(matches!(
        decision.records[1].body(),
        RecordBody::CancellationReconciled(_)
    ));
    assert!(matches!(
        decision.records[2].body(),
        RecordBody::RunCancelled(_)
    ));
    assert!(harness.kernel.state().pending_model_effect.is_none());
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Cancelled));
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
