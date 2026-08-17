#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end flow proves grouping, reverse completion, events, finalization, and replay"
)]
fn mixed_groups_reverse_completion_preserves_source_order_and_finalize_candidate() {
    let calls = vec![
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
        call(CALL_D, "delta"),
    ];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);

    let plans = vec![
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
        execute(
            &calls[3],
            ToolExecutionMode::Barrier,
            ToolFailurePolicy::ReturnToModel,
        ),
    ];
    let opened = harness.apply_input(
        tool_env(
            1_600,
            &[1_000, 1_001, 1_002, 1_003],
            &[1_000, 1_001],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C, TOOL_EFFECT_D],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: plans.into(),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    assert_eq!(opened.actions.len(), 2);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AwaitingTools));
    assert_tool_golden("valid--pr010-mixed-groups.json", &harness);

    let reverse = settle_tool(
        &mut harness,
        1_700,
        &[1_010],
        &[1_010],
        &[],
        TOOL_EFFECT_B,
        &calls[1],
    );
    assert_eq!(reverse.records.len(), 1, "later result remains buffered");
    assert_eq!(tool_message_call_ids(&harness), Vec::<u64>::new());
    assert_tool_golden("valid--pr010-reverse-parallel.json", &harness);

    let first = settle_tool(
        &mut harness,
        1_800,
        &[1_020, 1_021, 1_022, 1_023],
        &[1_020, 1_021, 1_022, 1_023, 1_024, 1_025],
        &[1_100, 1_101],
        TOOL_EFFECT_A,
        &calls[0],
    );
    assert_eq!(
        first.actions,
        vec![PostCommitAction::ExecuteEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_C),
        }]
    );
    assert_eq!(tool_message_call_ids(&harness), vec![CALL_A, CALL_B]);

    settle_tool(
        &mut harness,
        1_900,
        &[1_030, 1_031, 1_032],
        &[1_030, 1_031, 1_032, 1_033],
        &[1_102],
        TOOL_EFFECT_C,
        &calls[2],
    );
    settle_tool(
        &mut harness,
        2_000,
        &[1_040, 1_041, 1_042],
        &[1_040, 1_041, 1_042],
        &[1_103],
        TOOL_EFFECT_D,
        &calls[3],
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AfterToolBatch));
    assert_eq!(
        tool_message_call_ids(&harness),
        vec![CALL_A, CALL_B, CALL_C, CALL_D]
    );
    assert_eq!(
        tool_event_call_ids(&harness),
        vec![CALL_A, CALL_B, CALL_C, CALL_D]
    );

    harness.apply_input(
        transition_env(2_100, &[1_050], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterToolBatch, ReducerStageOutcome::Continue),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    harness.apply_input(
        transition_env(2_200, &[1_060, 1_061], &[1_060], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    );
    let TerminalState::Completed(completed) =
        harness.kernel.state().terminal.as_ref().expect("terminal")
    else {
        panic!("expected completed run");
    };
    assert_eq!(
        completed.result_message_id,
        id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE)
    );
    assert_tool_golden("valid--pr010-finalize-after-batch.json", &harness);
    assert_replay_prefixes(&harness);
}

#[test]
fn unknown_tool_is_synthetic_and_continue_model_uses_state_v2() {
    let calls = vec![call(CALL_A, "missing")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    let unknown = fixture_error_with_message("unknown_tool", "unknown tool: missing");
    let decision = harness.apply_input(
        tool_env(
            1_600,
            &[2_000, 2_001, 2_002, 2_003],
            &[2_000, 2_001],
            &[TOOL_EFFECT_A],
            &[2_100],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([ToolCallPlan::SyntheticClosure(
                    finstack_ai_kernel::SyntheticToolClosure {
                        call: calls[0].clone(),
                        execution: ToolExecutionMode::Sequential,
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        error: unknown.clone(),
                    },
                )]),
                continuation: ToolBatchContinuation::ContinueModel,
            },
        ),
    );
    assert!(decision.actions.is_empty());
    assert_eq!(harness.kernel.state().state_version, 2);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AfterToolBatch));
    let result = last_tool_result(&harness);
    assert!(result.is_error());
    let ContentBlock::Json(value) = &result.content()[0] else {
        panic!("synthetic result must contain JSON");
    };
    assert!(value.value().as_str().contains("unknown_tool"));
    assert_tool_golden("valid--pr010-unknown-tool.json", &harness);

    harness.apply_input(
        transition_env(1_700, &[2_010], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterToolBatch, ReducerStageOutcome::Continue),
    );
    assert_eq!(harness.kernel.state().cycle, 1);
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::PreparingContext)
    );
    let encoded = serde_json::to_value(harness.kernel.state()).expect("state v2 JSON");
    assert_eq!(encoded.get("state_version"), Some(&json!(2)));
    assert!(encoded.get("tool_calls").is_some());
    assert!(harness.kernel.state().state_hash().is_ok());
    assert_tool_golden("valid--pr010-continue-model.json", &harness);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end failure flow proves draining and undispatched closure together"
)]
fn fail_run_drains_parallel_group_and_closes_only_undispatched_calls() {
    let calls = vec![
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
    ];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    let plans = vec![
        execute(
            &calls[0],
            ToolExecutionMode::Parallel,
            ToolFailurePolicy::FailRun,
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
    ];
    harness.apply_input(
        tool_env(
            1_600,
            &[3_000, 3_001, 3_002, 3_003],
            &[3_000, 3_001],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: plans.into(),
                continuation: ToolBatchContinuation::ContinueModel,
            },
        ),
    );
    let failure = EffectFailed::try_new(
        id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
        tool_contract(),
        fixture_error("tool_framework_failed"),
        None,
        Some("tool-failure-a"),
    )
    .expect("tool failure");
    harness.apply_input(
        tool_env(
            1_700,
            &[3_010, 3_011],
            &[3_010, 3_011, 3_012],
            &[],
            &[3_100],
            &[],
            &[],
        ),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Failed(failure),
        }),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AwaitingTools));
    let active = harness
        .kernel
        .state()
        .active_tool_batch
        .as_ref()
        .expect("active batch");
    assert!(matches!(
        active.calls[2].status,
        ActiveToolCallStatus::Undispatched
    ));

    let decision = settle_tool(
        &mut harness,
        1_800,
        &[3_020, 3_021, 3_022, 3_023],
        &[3_020, 3_021, 3_022, 3_023, 3_024],
        &[3_101, 3_102],
        TOOL_EFFECT_B,
        &calls[1],
    );
    assert!(decision.actions.is_empty());
    assert!(decision.records.iter().all(|record| !matches!(
        record.body(),
        RecordBody::EffectRequested(requested)
            if requested.effect_id() == id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_C)
    )));
    let closed = harness
        .kernel
        .state()
        .last_tool_batch
        .as_ref()
        .expect("failed close");
    assert!(matches!(closed.outcome, ToolBatchOutcome::Failed { .. }));
    assert_eq!(
        tool_message_call_ids(&harness),
        vec![CALL_A, CALL_B, CALL_C]
    );
    let result = last_tool_result(&harness);
    let ContentBlock::Json(value) = &result.content()[0] else {
        panic!("aborted result JSON");
    };
    assert!(value.value().as_str().contains("tool_batch_aborted"));
    assert_tool_golden("valid--pr010-fail-run.json", &harness);
}
