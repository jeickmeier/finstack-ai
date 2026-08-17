#[test]
fn disallowed_stage_outcomes_and_cursor_mismatches_use_frozen_codes() {
    let mut before_run = Harness::default();
    accept(&mut before_run);
    let mut preparing = Harness::default();
    accept(&mut preparing);
    settle_before_run(&mut preparing);
    let mut before_model = Harness::default();
    accept(&mut before_model);
    settle_before_run(&mut before_model);
    prepare_context(&mut before_model, 0, false);
    let after_model = drive_to_after_model();
    let before_tool_batch = drive_to_before_tool_batch_matrix();
    let awaiting_tools = drive_to_awaiting_tools_matrix();
    let after_tool_batch = drive_to_after_tool_batch_matrix();
    let before_finalize = drive_to_before_finalize();

    let cases = [
        (
            &before_run.kernel,
            Stage::BeforeRun,
            vec!["continue", "fail"],
        ),
        (
            &preparing.kernel,
            Stage::PrepareContext,
            vec!["context_prepared", "fail"],
        ),
        (
            &before_model.kernel,
            Stage::BeforeModel,
            vec!["model_request_prepared", "fail"],
        ),
        (
            &after_model.kernel,
            Stage::AfterModel,
            vec!["continue", "fail"],
        ),
        (
            &before_tool_batch.kernel,
            Stage::BeforeToolBatch,
            vec!["tool_batch_prepared", "fail"],
        ),
        (
            &after_tool_batch.kernel,
            Stage::AfterToolBatch,
            vec!["continue", "fail"],
        ),
        (
            &before_finalize.kernel,
            Stage::BeforeFinalize,
            vec!["finalize_accepted", "continue_model", "fail"],
        ),
    ];
    assert_eq!(
        awaiting_tools.kernel.state().phase,
        Some(RunPhase::AwaitingTools)
    );
    let stages = [
        Stage::BeforeRun,
        Stage::PrepareContext,
        Stage::BeforeModel,
        Stage::AfterModel,
        Stage::BeforeToolBatch,
        Stage::AfterToolBatch,
        Stage::BeforeFinalize,
    ];

    for (kernel, expected_stage, allowed) in cases {
        let wrong_cycle = kernel.state().cycle + 1;
        for (name, outcome) in outcome_cases() {
            if !allowed.contains(&name) {
                assert_error_code(
                    kernel.decide(&empty_env(9_000), stage_input(0, expected_stage, outcome)),
                    "invalid_phase_input",
                );
            }
        }
        for actual_stage in stages {
            if actual_stage != expected_stage {
                assert_error_code(
                    kernel.decide(
                        &empty_env(9_001),
                        stage_input(wrong_cycle, actual_stage, ReducerStageOutcome::Continue),
                    ),
                    "stage_cursor_mismatch",
                );
            }
        }
        assert_error_code(
            kernel.decide(
                &empty_env(9_002),
                stage_input(wrong_cycle, expected_stage, ReducerStageOutcome::Continue),
            ),
            "stage_cursor_mismatch",
        );
    }
}

fn outcome_cases() -> Vec<(&'static str, ReducerStageOutcome)> {
    vec![
        ("continue", ReducerStageOutcome::Continue),
        (
            "context_prepared",
            ReducerStageOutcome::ContextPrepared {
                messages: Arc::from(context_messages()),
            },
        ),
        (
            "model_request_prepared",
            ReducerStageOutcome::ModelRequestPrepared {
                request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
                component: None,
                output_contract: output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
        ),
        (
            "tool_batch_prepared",
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &call(CALL_A, "alpha"),
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
        ("finalize_accepted", ReducerStageOutcome::FinalizeAccepted),
        (
            "continue_model",
            ReducerStageOutcome::ContinueModel { reason: None },
        ),
        (
            "fail",
            ReducerStageOutcome::Fail(fixture_error("stage_failed")),
        ),
    ]
}
