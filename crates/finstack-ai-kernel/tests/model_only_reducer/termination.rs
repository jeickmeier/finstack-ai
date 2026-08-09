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

fn drive_to_awaiting_model_with_limits(limits: RunLimits) -> Harness {
    let mut harness = Harness::default();
    accept_with(&mut harness, limits, None);
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    request_model(&mut harness, 0, false);
    harness
}

fn assert_limit_decision(decision: &Decision, expected: &finstack_ai_kernel::LimitDimension) {
    let RecordBody::LimitReached(reached) = decision.records[0].body() else {
        panic!("expected limit record");
    };
    assert_eq!(&reached.dimension, expected);
    let RecordBody::RunFailed(failed) = decision.records[1].body() else {
        panic!("expected terminal limit failure");
    };
    assert_eq!(failed.error.code.as_str(), "limit_reached");
    assert_eq!(failed.error.category, ErrorCategory::Limit);
    assert!(!failed.error.retryable);
    assert!(decision.actions.is_empty());
}

fn assert_termination_golden(file: &str, harness: &Harness) {
    let source = match file {
        "valid--pr011-limit.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-limit.json"
        ),
        "valid--pr011-retry.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-retry.json"
        ),
        "valid--pr011-model-cancel.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-model-cancel.json"
        ),
        "valid--pr011-tool-cancel.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-tool-cancel.json"
        ),
        "valid--pr011-uncertain.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-uncertain.json"
        ),
        _ => panic!("unknown termination golden: {file}"),
    };
    let expected: Value = serde_json::from_str(source).expect("parse termination golden");
    let state = harness.kernel.state();
    let records = harness
        .batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(|record| record.body().kind_name())
        .collect::<Vec<_>>();
    let tool_call_ids = state
        .messages
        .iter()
        .filter(|message| message.role() == MessageRole::Tool)
        .map(|message| match &message.content()[0] {
            ContentBlock::ToolResult(result) => {
                u64::from_be_bytes(
                    result.tool_call_id().as_bytes()[8..]
                        .try_into()
                        .expect("ordinal bytes"),
                ) & 0x3fff_ffff_ffff_ffff
            }
            _ => panic!("tool message must contain result"),
        })
        .collect::<Vec<_>>();
    let terminal = state.terminal.as_ref().map(|terminal| match terminal {
        TerminalState::Completed(_) => "completed",
        TerminalState::Failed(_) => "failed",
        TerminalState::Cancelled(_) => "cancelled",
    });
    let mut replayed = Kernel::default();
    let mut transient = 0_u64;
    for batch in &harness.batches {
        let events = replayed.apply(batch, transient).expect("golden replay");
        transient += u64::try_from(events.len()).expect("event count");
    }
    let actual = json!({
        "format_version": 1,
        "phase": state.phase,
        "cycle": state.cycle,
        "state_version": state.state_version,
        "records": records,
        "retry_attempts": state.retry.attempts,
        "has_pending_retry": state.retry.pending.is_some(),
        "last_limit": state.last_limit.as_ref().map(|limit| &limit.dimension),
        "cancellation_outstanding": state.cancellation.as_ref().map_or(0, |value| value.outstanding_effects.len()),
        "cancellation_uncertain": state.cancellation.as_ref().map_or(0, |value| value.uncertain_effects.len()),
        "tool_call_ids": tool_call_ids,
        "active_tool_batch": state.active_tool_batch.is_some(),
        "terminal": terminal,
        "replay_hash_equal": replayed.state().state_hash().expect("replay hash") == state.state_hash().expect("live hash"),
    });
    assert_eq!(actual, expected, "{file}");
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
    assert_termination_golden("valid--pr011-retry.json", &harness);
}

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
    assert_termination_golden("valid--pr011-uncertain.json", &harness);
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
    assert_termination_golden("valid--pr011-model-cancel.json", &harness);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the end-to-end chunked cancellation proof keeps exact records and source order visible"
)]
fn tool_cancellation_chunks_close_every_call_in_source_order() {
    use super::tool_batches::{
        BATCH, CALL_A, TOOL_EFFECT_A, call, execute, model_with_calls,
        settle_after_model_for_tools, tool_env,
    };
    use finstack_ai_kernel::{ToolBatchContinuation, ToolExecutionMode, ToolFailurePolicy};

    const CALL_B: u64 = 302;
    const CALL_C: u64 = 303;
    const TOOL_EFFECT_B: u64 = 402;
    const TOOL_EFFECT_C: u64 = 403;
    let calls = vec![
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
    ];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[5_000, 5_001, 5_002, 5_003],
            &[5_000, 5_001],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([
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
                    execute(
                        &calls[2],
                        ToolExecutionMode::Sequential,
                        ToolFailurePolicy::ReturnToModel,
                    ),
                ]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    let cancel = harness.apply_input(
        cancellation_env(1_650, &[5_010], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    assert_eq!(
        cancel.actions,
        vec![
            PostCommitAction::CancelEffect {
                effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
            },
            PostCommitAction::CancelEffect {
                effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_B),
            },
        ]
    );

    let later_first = harness.apply_input(
        transition_env(1_700, &[5_020, 5_021], &[5_020], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_B)]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(later_first.records.len(), 2);
    assert!(
        harness
            .kernel
            .state()
            .messages
            .iter()
            .all(|message| message.role() != MessageRole::Tool)
    );

    let closed = harness.apply_input(
        transition_env(
            1_750,
            &[5_030, 5_031, 5_032, 5_033, 5_034, 5_035, 5_036],
            &[5_030, 5_031, 5_032, 5_033, 5_034, 5_035, 5_036, 5_037],
            &[],
            &[],
            &[],
            &[5_100, 5_101, 5_102],
        ),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A)]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(closed.records.len(), 7);
    let tool_results = harness
        .kernel
        .state()
        .messages
        .iter()
        .filter(|message| message.role() == MessageRole::Tool)
        .map(|message| match &message.content()[0] {
            ContentBlock::ToolResult(result) => *result.tool_call_id(),
            _ => panic!("tool message must contain a tool result"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_results,
        vec![
            id::<finstack_ai_kernel::ToolCallTag>(CALL_A),
            id::<finstack_ai_kernel::ToolCallTag>(CALL_B),
            id::<finstack_ai_kernel::ToolCallTag>(CALL_C),
        ]
    );
    assert!(
        closed
            .records
            .iter()
            .filter_map(|record| {
                let RecordBody::ToolCallSettled(settled) = record.body() else {
                    return None;
                };
                Some(
                    settled.synthetic
                        && settled.error.as_ref().is_some_and(|error| {
                            error.code.as_str() == "cancelled"
                                && error.category == ErrorCategory::Cancellation
                                && !error.retryable
                        }),
                )
            })
            .all(|valid| valid)
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Cancelled));
    assert!(harness.kernel.state().active_tool_batch.is_none());
    let replayed = replay(&harness.batches);
    assert_eq!(replayed.state(), harness.kernel.state());
    assert_termination_golden("valid--pr011-tool-cancel.json", &harness);
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

#[test]
fn parent_cancellation_obeys_persisted_child_propagation_authorization() {
    let parent = root_acceptance();
    let parent_run_id = parent.run_id();
    for (policy, decision_id, accepted) in [
        (CancellationPropagation::Cascade, "child-decision", true),
        (
            CancellationPropagation::DetachOnlyIfPreauthorized,
            "child-decision",
            true,
        ),
        (
            CancellationPropagation::DetachOnlyIfPreauthorized,
            "detach:child-decision",
            false,
        ),
    ] {
        let child_run_id = id::<finstack_ai_kernel::RunTag>(30);
        let relation = RunRelation::try_new(
            parent.relation().root_run_id(),
            Some(parent_run_id),
            Some(id::<finstack_ai_kernel::EffectTag>(31)),
            finstack_ai_kernel::RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("child relation");
        let security = RunSecurityContext::try_new(
            "tenant-a",
            parent.security().principal().clone(),
            "oidc",
            "high",
            "policy-v1",
            decision_id,
            None,
        )
        .expect("child security");
        let child = RunAccepted::try_new(
            child_run_id,
            relation,
            security,
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: policy,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"agent":"child"}"#),
            Some(&parent),
        )
        .expect("child acceptance");
        let mut harness = Harness::default();
        harness.apply_input(
            transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
                lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
                accepted: child,
            }),
        );
        let result = harness.kernel.decide(
            &cancellation_env(1_100, &[2], &[], &[700]),
            KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
                initiator: finstack_ai_kernel::CancellationInitiator::ParentRun { parent_run_id },
                reason: Some(Arc::from("parent_cancelled")),
            }),
        );
        if accepted {
            assert!(matches!(
                result.expect("authorized propagation").records.as_slice(),
                [record] if matches!(record.body(), RecordBody::CancellationRequested(_))
            ));
        } else {
            assert!(matches!(
                result.expect_err("preauthorized detach must reject parent cancellation"),
                KernelError::InvalidInputPayload {
                    field: "initiator",
                    reason_code: "unauthorized"
                }
            ));
        }
    }
}

#[test]
fn deferred_model_cancellation_preserves_effect_identity_through_reconciliation() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_350, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "deferred-model-cancel",
        ),
    );
    let decision = harness.apply_input(
        cancellation_env(1_400, &[8], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    assert_eq!(
        decision.actions,
        vec![PostCommitAction::CancelEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
        }]
    );
    let reconciled = harness.apply_input(
        transition_env(1_450, &[9, 10, 11], &[4, 5], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)]),
            uncertain_effects: Arc::from([]),
        }),
    );
    let RecordBody::EffectCancelled(cancelled) = reconciled.records[0].body() else {
        panic!("deferred model must close with EffectCancelled");
    };
    assert_eq!(
        cancelled.effect_id(),
        id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Cancelled));
}

#[test]
fn deferred_tool_cancellation_closes_original_effect_and_source_call() {
    use super::tool_batches::{
        BATCH, CALL_A, TOOL_EFFECT_A, call, execute, model_with_calls,
        settle_after_model_for_tools, tool_contract, tool_env,
    };
    use finstack_ai_kernel::{
        ToolBatchContinuation, ToolBatchSettled, ToolBatchTag, ToolExecutionMode,
        ToolFailurePolicy, ToolSettlement,
    };
    let calls = vec![call(CALL_A, "alpha")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[6_000, 6_001, 6_002],
            &[6_000],
            &[TOOL_EFFECT_A],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &calls[0],
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    harness.apply_input(
        transition_env(1_650, &[6_010], &[6_010], &[], &[], &[], &[]),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Deferred(EffectDeferred {
                effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
                handle: ExternalHandleRef::try_new(
                    ComponentId::parse("finstack.tool.fixture").expect("component"),
                    "external-cancel",
                    RawJson::parse("{}").expect("metadata"),
                )
                .expect("handle"),
                reconciliation: ReconciliationPolicy::CallbackOrPoll,
                next_poll_at: None,
                expires_at: None,
                output_contract: tool_contract(),
            }),
        }),
    );
    let cancelled = harness.apply_input(
        cancellation_env(1_700, &[6_020], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    assert_eq!(
        cancelled.actions,
        vec![PostCommitAction::CancelEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
        }]
    );
    harness.apply_input(
        transition_env(
            1_750,
            &[6_030, 6_031, 6_032, 6_033, 6_034],
            &[6_030, 6_031, 6_032, 6_033],
            &[],
            &[],
            &[],
            &[6_100],
        ),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A)]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Cancelled));
    assert!(harness.kernel.state().active_tool_batch.is_none());
    let tool_message = harness
        .kernel
        .state()
        .messages
        .last()
        .expect("cancelled tool result");
    let ContentBlock::ToolResult(result) = &tool_message.content()[0] else {
        panic!("tool result");
    };
    assert!(result.is_error());
    assert_eq!(result.tool_call_id(), calls[0].tool_call_id());
}

#[test]
fn terminal_race_permutations_follow_committed_journal_precedence() {
    let completed_first = drive_to_completed();
    let late_cancel = completed_first.kernel.decide(
        &empty_env(2_000),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    assert!(matches!(
        late_cancel,
        Err(KernelError::TerminalStateImmutable)
    ));

    let mut cancellation_first = drive_to_awaiting_model();
    cancellation_first.apply_input(
        cancellation_env(1_350, &[7], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    cancellation_first.apply_input(
        transition_env(1_400, &[8, 9], &[3], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(
        cancellation_first.kernel.state().phase,
        Some(RunPhase::Cancelled)
    );
    assert!(
        cancellation_first
            .kernel
            .state()
            .messages
            .iter()
            .all(|message| message.role() != MessageRole::Assistant)
    );

    let mut uncertain = drive_to_awaiting_model();
    uncertain.apply_input(
        cancellation_env(1_350, &[7], &[], &[701]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    uncertain.apply_input(
        transition_env(1_400, &[8, 9], &[3], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(701),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)]),
        }),
    );
    assert_eq!(uncertain.kernel.state().phase, Some(RunPhase::Suspended));
    assert!(uncertain.kernel.state().terminal.is_none());

    for harness in [&cancellation_first, &uncertain] {
        let replayed = replay(&harness.batches);
        assert_eq!(replayed.state(), harness.kernel.state());
        assert_eq!(
            replayed.state().state_hash().expect("replay hash"),
            harness.kernel.state().state_hash().expect("live hash")
        );
    }
}
