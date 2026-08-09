use super::*;

#[derive(Clone, Copy, Debug)]
enum AllowedCase {
    BeforeRunContinue,
    BeforeRunFail,
    PrepareContextPrepared,
    PrepareContextFail,
    BeforeModelRequested,
    BeforeModelFail,
    AfterModelContinue,
    AfterModelFail,
    BeforeFinalizeAccepted,
    BeforeFinalizeContinueModel,
    BeforeFinalizeFail,
}

#[test]
fn every_allowed_stage_outcome_applies_exact_decision_and_state() {
    let cases = [
        AllowedCase::BeforeRunContinue,
        AllowedCase::BeforeRunFail,
        AllowedCase::PrepareContextPrepared,
        AllowedCase::PrepareContextFail,
        AllowedCase::BeforeModelRequested,
        AllowedCase::BeforeModelFail,
        AllowedCase::AfterModelContinue,
        AllowedCase::AfterModelFail,
        AllowedCase::BeforeFinalizeAccepted,
        AllowedCase::BeforeFinalizeContinueModel,
        AllowedCase::BeforeFinalizeFail,
    ];

    for case in cases {
        assert_allowed_case(case);
    }
}

fn assert_allowed_case(case: AllowedCase) {
    let (harness, decision, expected_bodies, expected_records, expected_events, phase, cycle) =
        run_allowed_case(case);
    assert_eq!(decision_body_names(&decision), expected_bodies, "{case:?}");
    assert_eq!(
        decision
            .records
            .iter()
            .map(RecordDraft::record_id)
            .collect::<Vec<_>>(),
        expected_records
            .into_iter()
            .map(id::<finstack_ai_kernel::RecordTag>)
            .collect::<Vec<_>>(),
        "{case:?}"
    );
    assert_eq!(
        decision
            .records
            .iter()
            .flat_map(|record| record.derived_event_ids().iter().copied())
            .collect::<Vec<_>>(),
        expected_events
            .into_iter()
            .map(id::<finstack_ai_kernel::EventTag>)
            .collect::<Vec<_>>(),
        "{case:?}"
    );
    assert_eq!(harness.kernel.state().phase, Some(phase), "{case:?}");
    assert_eq!(harness.kernel.state().cycle, cycle, "{case:?}");
    assert_eq!(
        decision.expected_sequence,
        expected_sequence(case),
        "{case:?}"
    );
    assert!(decision.diagnostics.is_empty(), "{case:?}");
    if !matches!(case, AllowedCase::BeforeModelRequested) {
        assert!(decision.actions.is_empty(), "{case:?}");
    }
    let replayed = replay(&harness.batches);
    assert_eq!(harness.kernel.state(), replayed.state(), "{case:?}");
    assert_allowed_state(case, &harness, &decision);
}

fn assert_allowed_state(case: AllowedCase, harness: &Harness, decision: &Decision) {
    match case {
        AllowedCase::PrepareContextPrepared => {
            assert_eq!(
                harness
                    .kernel
                    .state()
                    .current_turn
                    .as_ref()
                    .map(|turn| turn.turn_id),
                Some(id::<finstack_ai_kernel::TurnTag>(301))
            );
        }
        AllowedCase::BeforeModelRequested => {
            let pending = harness
                .kernel
                .state()
                .pending_model_effect
                .as_ref()
                .expect("pending model");
            assert_eq!(
                pending.model_request_id,
                id::<finstack_ai_kernel::ModelRequestTag>(302)
            );
            assert_eq!(
                pending.requested.effect_id(),
                id::<finstack_ai_kernel::EffectTag>(303)
            );
            assert_eq!(
                decision.actions,
                [PostCommitAction::ExecuteEffect {
                    effect_id: id::<finstack_ai_kernel::EffectTag>(303)
                }]
            );
        }
        AllowedCase::BeforeRunFail
        | AllowedCase::PrepareContextFail
        | AllowedCase::BeforeModelFail
        | AllowedCase::AfterModelFail => {
            assert!(matches!(
                harness.kernel.state().terminal_candidate.as_ref(),
                Some(TerminalCandidate::Failed { .. })
            ));
        }
        AllowedCase::BeforeFinalizeAccepted => {
            assert!(matches!(
                harness.kernel.state().terminal.as_ref(),
                Some(TerminalState::Completed(_))
            ));
        }
        AllowedCase::BeforeFinalizeFail => {
            assert!(matches!(
                harness.kernel.state().terminal.as_ref(),
                Some(TerminalState::Failed(_))
            ));
        }
        AllowedCase::BeforeFinalizeContinueModel => {
            assert!(harness.kernel.state().pending_model_effect.is_none());
            assert!(harness.kernel.state().terminal_candidate.is_none());
        }
        AllowedCase::BeforeRunContinue | AllowedCase::AfterModelContinue => {}
    }
}

const fn expected_sequence(case: AllowedCase) -> u64 {
    match case {
        AllowedCase::BeforeRunContinue | AllowedCase::BeforeRunFail => 2,
        AllowedCase::PrepareContextPrepared | AllowedCase::PrepareContextFail => 3,
        AllowedCase::BeforeModelRequested | AllowedCase::BeforeModelFail => 5,
        AllowedCase::AfterModelContinue | AllowedCase::AfterModelFail => 9,
        AllowedCase::BeforeFinalizeAccepted
        | AllowedCase::BeforeFinalizeContinueModel
        | AllowedCase::BeforeFinalizeFail => 10,
    }
}

type AllowedResult = (
    Harness,
    Decision,
    Vec<&'static str>,
    Vec<u64>,
    Vec<u64>,
    RunPhase,
    u64,
);

fn run_allowed_case(case: AllowedCase) -> AllowedResult {
    match case {
        AllowedCase::BeforeRunContinue | AllowedCase::BeforeRunFail => run_before_run_case(case),
        AllowedCase::PrepareContextPrepared | AllowedCase::PrepareContextFail => {
            run_prepare_context_case(case)
        }
        AllowedCase::BeforeModelRequested | AllowedCase::BeforeModelFail => {
            run_before_model_case(case)
        }
        AllowedCase::AfterModelContinue | AllowedCase::AfterModelFail => run_after_model_case(case),
        AllowedCase::BeforeFinalizeAccepted
        | AllowedCase::BeforeFinalizeContinueModel
        | AllowedCase::BeforeFinalizeFail => run_before_finalize_case(case),
    }
}

fn run_before_run_case(case: AllowedCase) -> AllowedResult {
    match case {
        AllowedCase::BeforeRunContinue => {
            let mut harness = Harness::default();
            accept(&mut harness);
            let decision = harness.apply_input(
                transition_env(3_000, &[50], &[], &[], &[], &[], &[]),
                stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded"],
                vec![50],
                vec![],
                RunPhase::PreparingContext,
                0,
            )
        }
        AllowedCase::BeforeRunFail => {
            let mut harness = Harness::default();
            accept(&mut harness);
            let decision = harness.apply_input(
                transition_env(3_001, &[51], &[], &[], &[], &[], &[]),
                stage_input(
                    0,
                    Stage::BeforeRun,
                    ReducerStageOutcome::Fail(fixture_error("before_run_failed")),
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded"],
                vec![51],
                vec![],
                RunPhase::BeforeFinalize,
                0,
            )
        }
        _ => unreachable!("before-run case"),
    }
}

fn run_prepare_context_case(case: AllowedCase) -> AllowedResult {
    match case {
        AllowedCase::PrepareContextPrepared => {
            let mut harness = Harness::default();
            accept(&mut harness);
            settle_before_run(&mut harness);
            let decision = harness.apply_input(
                transition_env(3_002, &[52, 53], &[], &[], &[301], &[], &[]),
                stage_input(
                    0,
                    Stage::PrepareContext,
                    ReducerStageOutcome::ContextPrepared {
                        messages: Arc::from(context_messages()),
                    },
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded", "context_prepared"],
                vec![52, 53],
                vec![],
                RunPhase::BeforeModel,
                0,
            )
        }
        AllowedCase::PrepareContextFail => {
            let mut harness = Harness::default();
            accept(&mut harness);
            settle_before_run(&mut harness);
            let decision = harness.apply_input(
                transition_env(3_003, &[54], &[], &[], &[], &[], &[]),
                stage_input(
                    0,
                    Stage::PrepareContext,
                    ReducerStageOutcome::Fail(fixture_error("context_failed")),
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded"],
                vec![54],
                vec![],
                RunPhase::BeforeFinalize,
                0,
            )
        }
        _ => unreachable!("prepare-context case"),
    }
}

fn run_before_model_case(case: AllowedCase) -> AllowedResult {
    match case {
        AllowedCase::BeforeModelRequested => {
            let mut harness = Harness::default();
            accept(&mut harness);
            settle_before_run(&mut harness);
            prepare_context(&mut harness, 0, false);
            let decision = harness.apply_input(
                transition_env(3_004, &[55, 56], &[50], &[303], &[], &[302], &[]),
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
            (
                harness,
                decision,
                vec!["stage_outcome_recorded", "effect_requested"],
                vec![55, 56],
                vec![50],
                RunPhase::AwaitingModel,
                0,
            )
        }
        AllowedCase::BeforeModelFail => {
            let mut harness = Harness::default();
            accept(&mut harness);
            settle_before_run(&mut harness);
            prepare_context(&mut harness, 0, false);
            let decision = harness.apply_input(
                transition_env(3_005, &[57], &[], &[], &[], &[], &[]),
                stage_input(
                    0,
                    Stage::BeforeModel,
                    ReducerStageOutcome::Fail(fixture_error("before_model_failed")),
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded"],
                vec![57],
                vec![],
                RunPhase::BeforeFinalize,
                0,
            )
        }
        _ => unreachable!("before-model case"),
    }
}

fn run_after_model_case(case: AllowedCase) -> AllowedResult {
    match case {
        AllowedCase::AfterModelContinue => {
            let mut harness = drive_to_after_model();
            let decision = harness.apply_input(
                transition_env(3_006, &[58], &[], &[], &[], &[], &[]),
                stage_input(0, Stage::AfterModel, ReducerStageOutcome::Continue),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded"],
                vec![58],
                vec![],
                RunPhase::BeforeFinalize,
                0,
            )
        }
        AllowedCase::AfterModelFail => {
            let mut harness = drive_to_after_model();
            let decision = harness.apply_input(
                transition_env(3_007, &[59], &[], &[], &[], &[], &[]),
                stage_input(
                    0,
                    Stage::AfterModel,
                    ReducerStageOutcome::Fail(fixture_error("after_model_failed")),
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded"],
                vec![59],
                vec![],
                RunPhase::BeforeFinalize,
                0,
            )
        }
        _ => unreachable!("after-model case"),
    }
}

fn run_before_finalize_case(case: AllowedCase) -> AllowedResult {
    match case {
        AllowedCase::BeforeFinalizeAccepted => {
            let mut harness = drive_to_before_finalize();
            let decision = harness.apply_input(
                transition_env(3_008, &[60, 61], &[51], &[], &[], &[], &[]),
                stage_input(
                    0,
                    Stage::BeforeFinalize,
                    ReducerStageOutcome::FinalizeAccepted,
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded", "run_completed"],
                vec![60, 61],
                vec![51],
                RunPhase::Completed,
                0,
            )
        }
        AllowedCase::BeforeFinalizeContinueModel => {
            let mut harness = drive_to_before_finalize();
            let decision = harness.apply_input(
                transition_env(3_009, &[62], &[], &[], &[], &[], &[]),
                stage_input(
                    0,
                    Stage::BeforeFinalize,
                    ReducerStageOutcome::ContinueModel { reason: None },
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded"],
                vec![62],
                vec![],
                RunPhase::PreparingContext,
                1,
            )
        }
        AllowedCase::BeforeFinalizeFail => {
            let mut harness = drive_to_before_finalize();
            let decision = harness.apply_input(
                transition_env(3_010, &[63, 64], &[52], &[], &[], &[], &[]),
                stage_input(
                    0,
                    Stage::BeforeFinalize,
                    ReducerStageOutcome::Fail(fixture_error("finalize_failed")),
                ),
            );
            (
                harness,
                decision,
                vec!["stage_outcome_recorded", "run_failed"],
                vec![63, 64],
                vec![52],
                RunPhase::Failed,
                0,
            )
        }
        _ => unreachable!("before-finalize case"),
    }
}

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
            &before_finalize.kernel,
            Stage::BeforeFinalize,
            vec!["finalize_accepted", "continue_model", "fail"],
        ),
    ];
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputFamily {
    Accept,
    Stage,
    Model,
    External,
}

#[test]
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
fn allocated_id_shortage_and_extra_precedence_follow_canonical_queue_order() {
    let mut preparing = Harness::default();
    accept(&mut preparing);
    settle_before_run(&mut preparing);
    let input = stage_input(
        0,
        Stage::PrepareContext,
        ReducerStageOutcome::ContextPrepared {
            messages: Arc::from(context_messages()),
        },
    );

    assert!(matches!(
        preparing.kernel.decide(
            &transition_env(9_600, &[], &[], &[], &[], &[], &[]),
            input.clone()
        ),
        Err(KernelError::AllocatedIdsExhausted { kind: "record_ids" })
    ));
    assert!(matches!(
        preparing.kernel.decide(
            &transition_env(9_601, &[1, 2, 3], &[1], &[], &[1], &[], &[]),
            input,
        ),
        Err(KernelError::UnusedAllocatedIds { kind: "record_ids" })
    ));
}

#[test]
fn assistant_semantics_precede_message_id_allocation() {
    let harness = drive_to_awaiting_model();
    let mut invalid_semantics = completed_input(
        TURN_ONE,
        MODEL_REQUEST_ONE,
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_399,
        "semantic-precedence",
        "hello",
    );
    let KernelInput::ModelSettled(ModelSettled {
        outcome: ModelSettlement::Completed {
            assistant_message, ..
        },
        ..
    }) = &mut invalid_semantics
    else {
        unreachable!("completed model input")
    };
    assert_eq!(assistant_message.created_at(), timestamp(1_399));
    assert_error_code(
        harness.kernel.decide(
            &transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[]),
            invalid_semantics,
        ),
        "assistant_message_mismatch",
    );
    assert!(matches!(
        harness.kernel.decide(
            &transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[]),
            completed_input(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "missing-message-id",
                "hello",
            ),
        ),
        Err(KernelError::AllocatedIdsExhausted {
            kind: "message_ids"
        })
    ));
}

fn assert_wrong_inputs(
    kernel: &Kernel,
    allowed: InputFamily,
    stage: Stage,
    identity_seed: u64,
    expected_error: &str,
) {
    for (family, input) in family_inputs(stage, identity_seed) {
        if family != allowed {
            assert_error_code(kernel.decide(&empty_env(9_400), input), expected_error);
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
    ]
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

#[test]
fn run_phase_match_and_wire_vocabulary_are_compiler_exhaustive() {
    let cases = [
        (RunPhase::Accepted, "accepted"),
        (RunPhase::BeforeRun, "before_run"),
        (RunPhase::PreparingContext, "preparing_context"),
        (RunPhase::BeforeModel, "before_model"),
        (RunPhase::AwaitingModel, "awaiting_model"),
        (RunPhase::AfterModel, "after_model"),
        (RunPhase::BeforeToolBatch, "before_tool_batch"),
        (RunPhase::AwaitingTools, "awaiting_tools"),
        (RunPhase::AfterToolBatch, "after_tool_batch"),
        (RunPhase::BeforeFinalize, "before_finalize"),
        (RunPhase::AwaitingInteraction, "awaiting_interaction"),
        (RunPhase::AwaitingExternal, "awaiting_external"),
        (RunPhase::Sleeping, "sleeping"),
        (RunPhase::Cancelling, "cancelling"),
        (RunPhase::Suspended, "suspended"),
        (RunPhase::Completed, "completed"),
        (RunPhase::Failed, "failed"),
        (RunPhase::Cancelled, "cancelled"),
    ];
    for (phase, expected) in cases {
        assert_eq!(phase_wire_name(phase), expected);
        assert_eq!(
            serde_json::to_value(phase).expect("phase JSON"),
            Value::String(expected.to_owned())
        );
    }
}

fn phase_wire_name(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Accepted => "accepted",
        RunPhase::BeforeRun => "before_run",
        RunPhase::PreparingContext => "preparing_context",
        RunPhase::BeforeModel => "before_model",
        RunPhase::AwaitingModel => "awaiting_model",
        RunPhase::AfterModel => "after_model",
        RunPhase::BeforeToolBatch => "before_tool_batch",
        RunPhase::AwaitingTools => "awaiting_tools",
        RunPhase::AfterToolBatch => "after_tool_batch",
        RunPhase::BeforeFinalize => "before_finalize",
        RunPhase::AwaitingInteraction => "awaiting_interaction",
        RunPhase::AwaitingExternal => "awaiting_external",
        RunPhase::Sleeping => "sleeping",
        RunPhase::Cancelling => "cancelling",
        RunPhase::Suspended => "suspended",
        RunPhase::Completed => "completed",
        RunPhase::Failed => "failed",
        RunPhase::Cancelled => "cancelled",
    }
}
