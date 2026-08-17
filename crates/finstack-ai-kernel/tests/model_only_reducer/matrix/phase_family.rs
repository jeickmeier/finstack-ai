#[derive(Clone, Copy, PartialEq, Eq)]
enum InputFamily {
    Accept,
    Stage,
    Model,
    External,
    Tool,
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the phase/input matrix intentionally lists every reachable reducer phase"
)]
fn every_reachable_phase_rejects_nonmatching_input_families() {
    let unaccepted = Harness::default();
    assert_wrong_inputs(
        &unaccepted.kernel,
        InputFamily::Accept,
        Stage::BeforeRun,
        1_000,
        "invalid_phase_input",
    );

    let mut before_run = Harness::default();
    accept(&mut before_run);
    assert_wrong_inputs(
        &before_run.kernel,
        InputFamily::Stage,
        Stage::BeforeRun,
        2_000,
        "invalid_phase_input",
    );
    let mut preparing = Harness::default();
    accept(&mut preparing);
    settle_before_run(&mut preparing);
    assert_wrong_inputs(
        &preparing.kernel,
        InputFamily::Stage,
        Stage::PrepareContext,
        3_000,
        "invalid_phase_input",
    );
    let mut before_model = Harness::default();
    accept(&mut before_model);
    settle_before_run(&mut before_model);
    prepare_context(&mut before_model, 0, false);
    assert_wrong_inputs(
        &before_model.kernel,
        InputFamily::Stage,
        Stage::BeforeModel,
        4_000,
        "invalid_phase_input",
    );
    let awaiting_model = drive_to_awaiting_model();
    assert_wrong_inputs(
        &awaiting_model.kernel,
        InputFamily::Model,
        Stage::BeforeModel,
        5_000,
        "invalid_phase_input",
    );
    let after_model = drive_to_after_model();
    assert_wrong_inputs(
        &after_model.kernel,
        InputFamily::Stage,
        Stage::AfterModel,
        6_000,
        "invalid_phase_input",
    );
    let before_tool_batch = drive_to_before_tool_batch_matrix();
    assert_wrong_inputs(
        &before_tool_batch.kernel,
        InputFamily::Stage,
        Stage::BeforeToolBatch,
        6_100,
        "invalid_phase_input",
    );
    let awaiting_tools = drive_to_awaiting_tools_matrix();
    assert_wrong_inputs(
        &awaiting_tools.kernel,
        InputFamily::Tool,
        Stage::BeforeToolBatch,
        6_200,
        "invalid_phase_input",
    );
    let after_tool_batch = drive_to_after_tool_batch_matrix();
    assert_wrong_inputs(
        &after_tool_batch.kernel,
        InputFamily::Stage,
        Stage::AfterToolBatch,
        6_300,
        "invalid_phase_input",
    );
    let before_finalize = drive_to_before_finalize();
    assert_wrong_inputs(
        &before_finalize.kernel,
        InputFamily::Stage,
        Stage::BeforeFinalize,
        7_000,
        "invalid_phase_input",
    );

    let mut awaiting_external = drive_to_awaiting_model();
    awaiting_external.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "phase-matrix-external",
        ),
    );
    assert_wrong_inputs(
        &awaiting_external.kernel,
        InputFamily::External,
        Stage::BeforeModel,
        8_000,
        "invalid_phase_input",
    );

    let completed = drive_to_completed();
    let failed = failed_terminal();
    for (index, kernel) in [&completed.kernel, &failed.kernel].into_iter().enumerate() {
        for (_, input) in family_inputs(Stage::BeforeRun, 9_000 + index as u64 * 100) {
            assert_error_code(
                kernel.decide(&empty_env(9_500), input),
                "terminal_state_immutable",
            );
        }
    }
}

#[test]
fn cancellation_is_accepted_idempotently_from_every_reachable_nonterminal_phase() {
    let mut before_run = Harness::default();
    accept(&mut before_run);
    let mut preparing = Harness::default();
    accept(&mut preparing);
    settle_before_run(&mut preparing);
    let mut before_model = Harness::default();
    accept(&mut before_model);
    settle_before_run(&mut before_model);
    prepare_context(&mut before_model, 0, false);
    let awaiting_model = drive_to_awaiting_model();
    let after_model = drive_to_after_model();
    let before_tool_batch = drive_to_before_tool_batch_matrix();
    let awaiting_tools = drive_to_awaiting_tools_matrix();
    let after_tool_batch = drive_to_after_tool_batch_matrix();
    let before_finalize = drive_to_before_finalize();
    let mut awaiting_external = drive_to_awaiting_model();
    awaiting_external.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "cancel-phase-external",
        ),
    );

    let awaiting_interaction = super::interactions::request_and_await(InteractionKind::Approval);
    let sleeping = super::termination::drive_to_sleeping();

    let mut harnesses = Vec::with_capacity(12);
    harnesses.push(before_run);
    harnesses.push(preparing);
    harnesses.push(before_model);
    harnesses.push(awaiting_model);
    harnesses.push(after_model);
    harnesses.push(before_tool_batch);
    harnesses.push(awaiting_tools);
    harnesses.push(after_tool_batch);
    harnesses.push(before_finalize);
    harnesses.push(awaiting_external);
    harnesses.push(awaiting_interaction);
    harnesses.push(sleeping);
    for (index, harness) in harnesses.iter().enumerate() {
        let cancel = KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("phase-matrix")),
        });
        let decision = harness
            .kernel
            .decide(
                &cancellation_env(
                    10_000 + i64::try_from(index).expect("index"),
                    &[8_000 + u64::try_from(index).expect("index")],
                    &[],
                    &[8_500 + u64::try_from(index).expect("index")],
                ),
                cancel,
            )
            .expect("cancellation must be accepted from reachable phase");
        assert!(matches!(
            decision.records.as_slice(),
            [record] if matches!(record.body(), RecordBody::CancellationRequested(_))
        ));
    }
}

