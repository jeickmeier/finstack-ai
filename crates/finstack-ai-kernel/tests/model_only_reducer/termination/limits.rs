#[test]
fn model_request_limit_allows_equality_and_records_first_crossing() {
    let mut below = Harness::default();
    let mut limits = RunLimits::empty();
    limits.max_model_requests = Some(2);
    accept_with(&mut below, limits, None);
    settle_before_run(&mut below);
    prepare_context(&mut below, 0, false);
    request_model(&mut below, 0, false);
    assert_eq!(below.kernel.state().limit_usage.model_requests, 1);

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
    assert_termination_golden("valid--pr011-limit.json", &crossed);
}

#[test]
fn scalar_stage_limits_cover_below_exact_and_above_boundaries() {
    let context_bytes = u64::try_from(
        serde_json_canonicalizer::to_vec(&context_messages())
            .expect("canonical context")
            .len(),
    )
    .expect("context length");
    for (maximum, crossing) in [(2, false), (1, false), (0, true)] {
        let mut limits = RunLimits::empty();
        limits.max_turns = Some(maximum);
        let mut harness = Harness::default();
        accept_with(&mut harness, limits, None);
        settle_before_run(&mut harness);
        let decision = harness.apply_input(
            if crossing {
                transition_env(1_200, &[3, 4], &[2, 3], &[], &[], &[], &[])
            } else {
                transition_env(1_200, &[3, 4], &[], &[], &[TURN_ONE], &[], &[])
            },
            stage_input(
                0,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from(context_messages()),
                },
            ),
        );
        if crossing {
            assert_limit_decision(&decision, &finstack_ai_kernel::LimitDimension::Turns);
        } else {
            assert_eq!(harness.kernel.state().limit_usage.turns, 1);
        }
    }

    for (maximum, crossing) in [
        (context_bytes + 1, false),
        (context_bytes, false),
        (context_bytes - 1, true),
    ] {
        let mut limits = RunLimits::empty();
        limits.max_context_bytes = Some(maximum);
        let mut harness = Harness::default();
        accept_with(&mut harness, limits, None);
        settle_before_run(&mut harness);
        let decision = harness.apply_input(
            if crossing {
                transition_env(1_200, &[3, 4], &[2, 3], &[], &[], &[], &[])
            } else {
                transition_env(1_200, &[3, 4], &[], &[], &[TURN_ONE], &[], &[])
            },
            stage_input(
                0,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from(context_messages()),
                },
            ),
        );
        if crossing {
            assert_limit_decision(&decision, &finstack_ai_kernel::LimitDimension::ContextBytes);
        } else {
            assert_eq!(
                harness.kernel.state().limit_usage.context_bytes,
                context_bytes
            );
        }
    }

    for (maximum_ms, crossing) in [(101, false), (100, false), (99, true)] {
        let mut limits = RunLimits::empty();
        limits.max_wall_time = Some(finstack_ai_kernel::Duration::from_millis(maximum_ms));
        let mut harness = Harness::default();
        accept_with(&mut harness, limits, None);
        let decision = harness.apply_input(
            if crossing {
                transition_env(1_100, &[2, 3], &[2, 3], &[], &[], &[], &[])
            } else {
                transition_env(1_100, &[2], &[], &[], &[], &[], &[])
            },
            stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
        );
        if crossing {
            assert_limit_decision(&decision, &finstack_ai_kernel::LimitDimension::WallTime);
        } else {
            assert_eq!(
                harness.kernel.state().limit_usage.wall_time,
                finstack_ai_kernel::Duration::from_millis(100)
            );
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one matrix applies identical below/exact/above proof to completion-owned dimensions"
)]
fn completion_limits_cover_below_exact_and_above_boundaries() {
    let output_bytes = u64::try_from(
        RawJson::parse(r#"{"text":"hello"}"#)
            .expect("output")
            .as_bytes()
            .len(),
    )
    .expect("output length");
    let extension_key = finstack_ai_kernel::LimitKey::parse("app.calls").expect("limit key");
    for boundary in 0..3 {
        let crossing = boundary == 2;
        let mut limits = RunLimits::empty();
        let observed = 10_u64;
        match boundary {
            0 => {
                limits.max_input_tokens = Some(observed + 1);
                limits.max_output_tokens = Some(observed + 1);
                limits.max_output_bytes = Some(output_bytes + 1);
                limits
                    .extension_counters
                    .insert(extension_key.clone(), observed + 1);
            }
            1 => {
                limits.max_input_tokens = Some(observed);
                limits.max_output_tokens = Some(observed);
                limits.max_output_bytes = Some(output_bytes);
                limits
                    .extension_counters
                    .insert(extension_key.clone(), observed);
            }
            _ => {
                limits.max_input_tokens = Some(observed - 1);
                limits.max_output_tokens = Some(observed);
                limits.max_output_bytes = Some(output_bytes);
                limits
                    .extension_counters
                    .insert(extension_key.clone(), observed);
            }
        }
        let usage = finstack_ai_kernel::Usage::try_new(
            Some(observed),
            Some(observed),
            Some(observed * 2),
            None,
            BTreeMap::from([(extension_key.clone(), observed)]),
        )
        .expect("usage");
        let mut harness = drive_to_awaiting_model_with_limits(limits);
        let decision = harness.apply_input(
            if crossing {
                transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[])
            } else {
                transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE])
            },
            completed_input_with_usage(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "completion-boundary",
                usage,
            ),
        );
        if crossing {
            assert_limit_decision(&decision, &finstack_ai_kernel::LimitDimension::InputTokens);
        } else {
            assert_eq!(harness.kernel.state().limit_usage.input_tokens, observed);
            assert_eq!(harness.kernel.state().limit_usage.output_tokens, observed);
            assert_eq!(
                harness.kernel.state().limit_usage.output_bytes,
                output_bytes
            );
            assert_eq!(
                harness.kernel.state().limit_usage.extension_counters[&extension_key],
                observed
            );
        }
    }

    for (maximum, crossing) in [(11, false), (10, false), (9, true)] {
        let mut limits = RunLimits::empty();
        limits.max_cost = Some(
            finstack_ai_kernel::CostLimit::try_new(
                "USD",
                maximum,
                "prices-v1",
                finstack_ai_kernel::UnknownUsagePolicy::FailClosed,
            )
            .expect("cost limit"),
        );
        let usage = finstack_ai_kernel::Usage::try_new(
            None,
            None,
            None,
            Some(finstack_ai_kernel::CostAmount::try_new("USD", 10, "prices-v1").expect("cost")),
            BTreeMap::new(),
        )
        .expect("usage");
        let mut harness = drive_to_awaiting_model_with_limits(limits);
        let decision = harness.apply_input(
            if crossing {
                transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[])
            } else {
                transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE])
            },
            completed_input_with_usage(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "cost-boundary",
                usage,
            ),
        );
        if crossing {
            assert_limit_decision(&decision, &finstack_ai_kernel::LimitDimension::Cost);
        } else {
            assert_eq!(
                harness
                    .kernel
                    .state()
                    .limit_usage
                    .cost
                    .as_ref()
                    .expect("cost")
                    .micros(),
                10
            );
        }
    }
}

#[test]
fn completion_limit_crossing_identifies_each_non_cost_dimension() {
    let output_bytes = u64::try_from(
        RawJson::parse(r#"{"text":"hello"}"#)
            .expect("output")
            .as_bytes()
            .len(),
    )
    .expect("output length");
    let extension_key = finstack_ai_kernel::LimitKey::parse("app.calls").expect("limit key");
    let cases = [
        (
            {
                let mut limits = RunLimits::empty();
                limits.max_output_tokens = Some(9);
                limits
            },
            finstack_ai_kernel::Usage::try_new(None, Some(10), Some(10), None, BTreeMap::new())
                .expect("usage"),
            finstack_ai_kernel::LimitDimension::OutputTokens,
        ),
        (
            {
                let mut limits = RunLimits::empty();
                limits.max_output_bytes = Some(output_bytes - 1);
                limits
            },
            finstack_ai_kernel::Usage::empty(),
            finstack_ai_kernel::LimitDimension::OutputBytes,
        ),
        (
            {
                let mut limits = RunLimits::empty();
                limits.extension_counters.insert(extension_key.clone(), 9);
                limits
            },
            finstack_ai_kernel::Usage::try_new(
                None,
                None,
                None,
                None,
                BTreeMap::from([(extension_key, 10)]),
            )
            .expect("usage"),
            finstack_ai_kernel::LimitDimension::Extension {
                key: finstack_ai_kernel::LimitKey::parse("app.calls").expect("limit key"),
            },
        ),
    ];
    for (limits, usage, dimension) in cases {
        let mut harness = drive_to_awaiting_model_with_limits(limits);
        let decision = harness.apply_input(
            transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[]),
            completed_input_with_usage(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "dimension-crossing",
                usage,
            ),
        );
        assert_limit_decision(&decision, &dimension);
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "tool calls and admitted parallel width share one three-point plan-admission matrix"
)]
fn tool_plan_limits_cover_below_exact_and_above_boundaries() {
    use super::tool_batches::{
        BATCH, CALL_A, TOOL_EFFECT_A, call, execute, model_with_calls_and_limits,
        settle_after_model_for_tools, tool_env,
    };
    use finstack_ai_kernel::{ToolBatchContinuation, ToolExecutionMode, ToolFailurePolicy};
    const CALL_B: u64 = 302;
    const TOOL_EFFECT_B: u64 = 402;
    let calls = vec![call(CALL_A, "alpha"), call(CALL_B, "beta")];
    for (maximum, crossing) in [(3, false), (2, false), (1, true)] {
        let mut limits = RunLimits::empty();
        limits.max_tool_calls = Some(maximum);
        let mut harness = model_with_calls_and_limits(&calls, limits);
        settle_after_model_for_tools(&mut harness);
        let plans = Arc::from([
            execute(
                &calls[0],
                ToolExecutionMode::Parallel,
                ToolFailurePolicy::ReturnToModel,
            ),
            execute(
                &calls[1],
                ToolExecutionMode::Parallel,
                ToolFailurePolicy::ReturnToModel,
            ),
        ]);
        let decision = harness.apply_input(
            if crossing {
                transition_env(1_600, &[5_200, 5_201], &[5_200, 5_201], &[], &[], &[], &[])
            } else {
                tool_env(
                    1_600,
                    &[5_200, 5_201, 5_202, 5_203],
                    &[5_200, 5_201],
                    &[TOOL_EFFECT_A, TOOL_EFFECT_B],
                    &[],
                    &[BATCH],
                    &[],
                )
            },
            stage_input(
                0,
                Stage::BeforeToolBatch,
                ReducerStageOutcome::ToolBatchPrepared {
                    calls: plans,
                    continuation: ToolBatchContinuation::Finalize,
                },
            ),
        );
        if crossing {
            assert_limit_decision(&decision, &finstack_ai_kernel::LimitDimension::ToolCalls);
        } else {
            assert_eq!(harness.kernel.state().limit_usage.tool_calls, 2);
        }
    }

    for (maximum, crossing) in [(3, false), (2, false), (1, true)] {
        let mut limits = RunLimits::empty();
        limits.max_parallel_tools = Some(maximum);
        let mut harness = model_with_calls_and_limits(&calls, limits);
        settle_after_model_for_tools(&mut harness);
        let plans = Arc::from([
            execute(
                &calls[0],
                ToolExecutionMode::Parallel,
                ToolFailurePolicy::ReturnToModel,
            ),
            execute(
                &calls[1],
                ToolExecutionMode::Parallel,
                ToolFailurePolicy::ReturnToModel,
            ),
        ]);
        let decision = harness.apply_input(
            if crossing {
                transition_env(1_600, &[5_300, 5_301], &[5_300, 5_301], &[], &[], &[], &[])
            } else {
                tool_env(
                    1_600,
                    &[5_300, 5_301, 5_302, 5_303],
                    &[5_300, 5_301],
                    &[TOOL_EFFECT_A, TOOL_EFFECT_B],
                    &[],
                    &[BATCH],
                    &[],
                )
            },
            stage_input(
                0,
                Stage::BeforeToolBatch,
                ReducerStageOutcome::ToolBatchPrepared {
                    calls: plans,
                    continuation: ToolBatchContinuation::Finalize,
                },
            ),
        );
        if crossing {
            assert_limit_decision(
                &decision,
                &finstack_ai_kernel::LimitDimension::ParallelTools,
            );
        } else {
            assert_eq!(harness.kernel.state().limit_usage.max_parallel_tools, 2);
        }
    }
}

#[test]
fn retry_limit_covers_below_exact_and_above_boundaries() {
    for (maximum, crossing) in [(2, false), (1, false), (0, true)] {
        let mut limits = RunLimits::empty();
        limits.max_retries = Some(maximum);
        let mut harness = drive_to_awaiting_model_with_limits(limits);
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
            if crossing {
                transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[])
            } else {
                transition_env(1_500, &[8, 9, 10], &[4], &[701], &[], &[], &[])
            },
            stage_input(0, Stage::BeforeFinalize, ReducerStageOutcome::Retry(retry)),
        );
        if crossing {
            assert_limit_decision(&decision, &finstack_ai_kernel::LimitDimension::Retries);
        } else {
            assert_eq!(harness.kernel.state().retry.attempts, 1);
            assert_eq!(harness.kernel.state().limit_usage.retries, 1);
        }
    }
}

#[test]
fn security_validation_precedes_limit_and_hard_deadline_precedes_explicit_cancel() {
    let mut harness = Harness::default();
    accept_with(&mut harness, RunLimits::empty(), Some(timestamp(1_500)));
    let unauthorized = harness
        .kernel
        .decide(
            &empty_env(1_500),
            KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
                initiator: finstack_ai_kernel::CancellationInitiator::Principal {
                    principal: PrincipalRef::try_new("issuer-a", "intruder", Some("tenant-a"))
                        .expect("principal"),
                    authorization: finstack_ai_kernel::AuthorizationEvidence::try_new(
                        "policy-v1",
                        "decision-v1",
                    )
                    .expect("authorization"),
                },
                reason: None,
            }),
        )
        .expect_err("unauthorized cancellation must fail before deadline evaluation");
    assert!(matches!(
        unauthorized,
        KernelError::InvalidInputPayload {
            field: "initiator",
            reason_code: "unauthorized"
        }
    ));

    let decision = harness.apply_input(
        transition_env(1_500, &[2, 3], &[2, 3], &[], &[], &[], &[]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    assert!(matches!(
        decision.records[0].body(),
        RecordBody::LimitReached(finstack_ai_kernel::LimitReached {
            dimension: finstack_ai_kernel::LimitDimension::WallTime,
            ..
        })
    ));
    let RecordBody::RunFailed(failed) = decision.records[1].body() else {
        panic!("deadline must terminate before explicit cancellation");
    };
    assert_eq!(failed.error.code.as_str(), "deadline_exceeded");
    assert_eq!(failed.error.category, ErrorCategory::Deadline);
    assert!(decision.actions.is_empty());
}
