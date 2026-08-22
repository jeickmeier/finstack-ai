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
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Cancelling));
    assert_eq!(harness.kernel.state().state_version(), 3);

    harness.apply_input(
        transition_env(1_200, &[3, 4], &[2], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Cancelled));
    assert!(matches!(
        harness.kernel.state().terminal(),
        Some(TerminalState::Cancelled(_))
    ));
    assert!(matches!(
        harness.events.last().map(RunEvent::kind),
        Some(RunEventKind::RunCancelled)
    ));
}

pub(super) fn drive_to_sleeping() -> Harness {
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
    harness
}

#[test]
fn retry_records_timer_intent_and_starts_a_fresh_cycle_after_firing() {
    let mut harness = drive_to_sleeping();
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Sleeping));
    assert_eq!(harness.kernel.state().retry().attempts, 1);

    harness.apply_input(
        transition_env(1_750, &[11], &[], &[], &[], &[], &[]),
        KernelInput::TimerFired(finstack_ai_kernel::TimerFiredInput {
            effect_id: id::<finstack_ai_kernel::EffectTag>(701),
            due_at: timestamp(1_750),
            fired_at: timestamp(1_750),
        }),
    );
    assert_eq!(harness.kernel.state().cycle(), 1);
    assert_eq!(
        harness.kernel.state().phase(),
        Some(RunPhase::PreparingContext)
    );
    assert!(harness.kernel.state().retry().pending.is_none());

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
fn cancel_requested_while_sleeping_includes_the_timer_effect() {
    let mut harness = drive_to_sleeping();
    let decision = harness.apply_input(
        cancellation_env(1_600, &[20], &[], &[800]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    assert_eq!(
        decision.actions,
        vec![PostCommitAction::CancelEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(701),
        }]
    );
    let state = harness.kernel.state();
    assert_eq!(state.phase(), Some(RunPhase::Cancelling));
    assert!(state.retry().pending.is_some());
    assert_eq!(
        state
            .cancellation()
            .expect("cancellation")
            .outstanding_effects
            .as_ref(),
        [id::<finstack_ai_kernel::EffectTag>(701)]
    );
}

#[test]
fn reconcile_cancelled_timer_does_not_start_a_fresh_cycle() {
    let mut harness = drive_to_sleeping();
    harness.apply_input(
        cancellation_env(1_600, &[20], &[], &[800]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    let decision = harness.apply_input(
        transition_env(1_650, &[21, 22, 23], &[10, 11], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(800),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(701)]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(
        decision_body_names(&decision),
        [
            "effect_cancelled",
            "cancellation_reconciled",
            "run_cancelled",
        ]
    );
    let state = harness.kernel.state();
    assert_eq!(state.phase(), Some(RunPhase::Cancelled));
    assert!(state.retry().pending.is_none());
    assert!(state.retry().timer_firings.is_empty());
    assert_eq!(state.cycle(), 0);
}

#[test]
fn timer_fired_after_uncertain_suspend_is_rejected() {
    let mut harness = drive_to_sleeping();
    harness.apply_input(
        cancellation_env(1_600, &[20], &[], &[800]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    harness.apply_input(
        transition_env(1_650, &[21, 22], &[10], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(800),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(701)]),
        }),
    );
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Suspended));
    let before = harness.kernel.state().clone();
    assert_error_code(
        harness.kernel.decide(
            &empty_env(1_750),
            KernelInput::TimerFired(finstack_ai_kernel::TimerFiredInput {
                effect_id: id::<finstack_ai_kernel::EffectTag>(701),
                due_at: timestamp(1_750),
                fired_at: timestamp(1_750),
            }),
        ),
        "invalid_phase_input",
    );
    assert_eq!(harness.kernel.state(), &before);
    let replayed = replay(&harness.batches);
    assert_eq!(replayed.state(), harness.kernel.state());
    assert_eq!(
        replayed.state().state_hash().expect("replay hash"),
        harness.kernel.state().state_hash().expect("live hash")
    );
}

#[test]
fn timer_fired_after_cancel_is_rejected() {
    let mut harness = drive_to_sleeping();
    harness.apply_input(
        cancellation_env(1_600, &[20], &[], &[800]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    let error = harness
        .kernel
        .decide(
            &empty_env(1_750),
            KernelInput::TimerFired(finstack_ai_kernel::TimerFiredInput {
                effect_id: id::<finstack_ai_kernel::EffectTag>(701),
                due_at: timestamp(1_750),
                fired_at: timestamp(1_750),
            }),
        )
        .expect_err("late timer");
    assert_eq!(error.code(), "invalid_phase_input");
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
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Suspended));
    assert_eq!(
        harness
            .kernel
            .state()
            .suspension()
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
    assert!(harness.kernel.state().pending_model_effect().is_none());
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Cancelled));
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
            .messages()
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
        .messages()
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
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Cancelled));
    assert!(harness.kernel.state().active_tool_batch().is_none());
    let replayed = replay(&harness.batches);
    assert_eq!(replayed.state(), harness.kernel.state());
    assert_eq!(
        replayed.state().state_hash().expect("replay hash"),
        harness.kernel.state().state_hash().expect("live hash")
    );
    assert_termination_golden("valid--pr011-tool-cancel.json", &harness);
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
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Cancelled));
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
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::Cancelled));
    assert!(harness.kernel.state().active_tool_batch().is_none());
    let tool_message = harness
        .kernel
        .state()
        .messages()
        .last()
        .expect("cancelled tool result");
    let ContentBlock::ToolResult(result) = &tool_message.content()[0] else {
        panic!("tool result");
    };
    assert!(result.is_error());
    assert_eq!(result.tool_call_id(), calls[0].tool_call_id());
}
