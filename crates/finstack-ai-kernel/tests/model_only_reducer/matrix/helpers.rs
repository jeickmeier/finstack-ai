fn assert_wrong_inputs(
    kernel: &Kernel,
    allowed: InputFamily,
    stage: Stage,
    identity_seed: u64,
    expected_error: &str,
) {
    for (family, input) in family_inputs(stage, identity_seed) {
        if family != allowed {
            let expected = if family == InputFamily::Tool
                && kernel.state().phase == Some(RunPhase::AwaitingExternal)
                && kernel.state().active_tool_batch.is_none()
            {
                "tool_settlement_mismatch"
            } else {
                expected_error
            };
            assert_error_code(kernel.decide(&empty_env(9_400), input), expected);
        }
    }
}

fn family_inputs(stage: Stage, identity_seed: u64) -> Vec<(InputFamily, KernelInput)> {
    vec![
        (InputFamily::Accept, accept_input()),
        (
            InputFamily::Stage,
            stage_input(identity_seed, stage, ReducerStageOutcome::Continue),
        ),
        (
            InputFamily::Model,
            completed_input(
                identity_seed + 1,
                identity_seed + 2,
                identity_seed + 3,
                identity_seed + 4,
                9_400,
                &format!("phase-model-{identity_seed}"),
                "hello",
            ),
        ),
        (
            InputFamily::External,
            external_completed_input(
                identity_seed + 5,
                identity_seed + 6,
                9_400,
                &format!("phase-external-{identity_seed}"),
                "hello",
            ),
        ),
        (
            InputFamily::Tool,
            KernelInput::ToolBatchSettled(ToolBatchSettled {
                tool_batch_id: id::<ToolBatchTag>(identity_seed + 7),
                outcome: ToolSettlement::Completed(tool_completed(
                    identity_seed + 8,
                    &call(identity_seed + 9, "phase-tool"),
                )),
            }),
        ),
    ]
}

fn drive_to_before_tool_batch_matrix() -> Harness {
    let calls = [call(CALL_A, "alpha")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness
}

fn drive_to_awaiting_tools_matrix() -> Harness {
    let calls = [call(CALL_A, "alpha")];
    let mut harness = drive_to_before_tool_batch_matrix();
    harness.apply_input(
        tool_env(
            6_400,
            &[6_400, 6_401, 6_402],
            &[6_400],
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
    harness
}

fn drive_to_after_tool_batch_matrix() -> Harness {
    let calls = [call(CALL_A, "alpha")];
    let mut harness = drive_to_awaiting_tools_matrix();
    harness.apply_input(
        tool_env(
            6_500,
            &[6_410, 6_411, 6_412],
            &[6_410, 6_411, 6_412],
            &[],
            &[6_420],
            &[],
            &[],
        ),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Completed(tool_completed(TOOL_EFFECT_A, &calls[0])),
        }),
    );
    harness
}

fn failed_terminal() -> Harness {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        failed_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "phase-matrix-failure",
        ),
    );
    harness.apply_input(
        transition_env(1_500, &[8, 9], &[4], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    );
    harness
}

