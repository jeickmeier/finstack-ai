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
    assert_eq!(harness.kernel.state().phase(), Some(phase), "{case:?}");
    assert_eq!(harness.kernel.state().cycle(), cycle, "{case:?}");
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
                    .current_turn()
                    .map(|turn| turn.turn_id),
                Some(id::<finstack_ai_kernel::TurnTag>(301))
            );
        }
        AllowedCase::BeforeModelRequested => {
            let pending = harness
                .kernel
                .state()
                .pending_model_effect()
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
                harness.kernel.state().terminal_candidate(),
                Some(TerminalCandidate::Failed { .. })
            ));
        }
        AllowedCase::BeforeFinalizeAccepted => {
            assert!(matches!(
                harness.kernel.state().terminal(),
                Some(TerminalState::Completed(_))
            ));
        }
        AllowedCase::BeforeFinalizeFail => {
            assert!(matches!(
                harness.kernel.state().terminal(),
                Some(TerminalState::Failed(_))
            ));
        }
        AllowedCase::BeforeFinalizeContinueModel => {
            assert!(harness.kernel.state().pending_model_effect().is_none());
            assert!(harness.kernel.state().terminal_candidate().is_none());
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

