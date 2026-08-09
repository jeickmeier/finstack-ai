use std::collections::BTreeMap;

use finstack_ai_kernel::{ArtifactRef, ArtifactTag, BlobRef, Usage};

use super::*;

#[test]
fn equal_external_completion_is_empty_and_full_trace_replays() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "external-job-1"),
    );
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::AwaitingExternal)
    );
    let pending = harness
        .kernel
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending external model");
    assert_eq!(
        pending.requested.effect_id(),
        id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)
    );
    assert_eq!(
        pending.deferred.as_ref().map(|value| value.effect_id),
        Some(id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE))
    );
    assert_eq!(
        harness.kernel.state().state_hash().expect("deferred hash"),
        Digest::from_hex("6c2c1443a340414163927d41f0f1a1bb9b32a5de72f70feff362c59e833f3f2b")
            .expect("vector")
    );

    let external = external_completed_input(
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_500,
        "completion-external-1",
        "hello",
    );
    harness.apply_input(
        transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        external.clone(),
    );
    let duplicate = harness
        .kernel
        .decide(&empty_env(1_501), external)
        .expect("equal external replay");
    assert!(duplicate.records.is_empty());
    assert!(duplicate.actions.is_empty());
    assert!(
        duplicate
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "duplicate_settlement")
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AfterModel));
    assert_exact_event_trace(&harness, &external_success_events());
    assert!(
        harness
            .events
            .iter()
            .filter(|event| matches!(
                event.kind(),
                RunEventKind::EffectDeferred | RunEventKind::EffectCompleted
            ))
            .all(|event| {
                event.effect_id() == Some(id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE))
            })
    );

    let replayed = replay(&harness.batches);
    assert_eq!(harness.kernel.state(), replayed.state());
    assert_eq!(
        harness.kernel.state().state_hash().expect("live hash"),
        replayed.state().state_hash().expect("replayed hash")
    );
}

#[test]
fn external_completion_with_usage_and_artifacts_is_duplicate_live_and_after_replay() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "external-job-rich"),
    );
    let external = external_completed_with_evidence("external-rich-1");
    harness.apply_input(
        transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        external.clone(),
    );

    assert_duplicate(
        &harness
            .kernel
            .decide(&empty_env(1_501), external.clone())
            .expect("equal rich external replay"),
    );

    let replayed = replay(&harness.batches);
    assert_duplicate(
        &replayed
            .decide(&empty_env(1_502), external)
            .expect("equal rich replayed completion"),
    );
}

#[test]
fn completion_identity_precedes_per_effect_settlement_conflicts() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "external-job-precedence",
        ),
    );
    harness.apply_input(
        transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        external_completed_with_evidence("external-precedence-1"),
    );

    assert_error_code(
        harness.kernel.decide(
            &empty_env(1_501),
            external_completed_with_usage("external-precedence-1", 6),
        ),
        "conflicting_completion_id",
    );
    assert_error_code(
        harness.kernel.decide(
            &empty_env(1_502),
            external_completed_with_evidence("external-precedence-2"),
        ),
        "conflicting_settlement",
    );
}

#[test]
fn deferred_duplicate_requires_original_turn_and_request_correlations() {
    let mut harness = drive_to_awaiting_model();
    let original = deferred_input(
        TURN_ONE,
        MODEL_REQUEST_ONE,
        EFFECT_ONE,
        "external-job-correlations",
    );
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        original,
    );

    assert_error_code(
        harness.kernel.decide(
            &empty_env(1_401),
            deferred_input(
                TURN_TWO,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                "external-job-correlations",
            ),
        ),
        "conflicting_settlement",
    );
    assert_error_code(
        harness.kernel.decide(
            &empty_env(1_402),
            deferred_input(
                TURN_ONE,
                MODEL_REQUEST_TWO,
                EFFECT_ONE,
                "external-job-correlations",
            ),
        ),
        "conflicting_settlement",
    );
}

#[test]
fn opaque_output_shape_changes_are_conflicting_completion_identities() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "external-job-opaque",
        ),
    );
    let mut absent = external_completed_input(
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_500,
        "external-opaque-1",
        "hello",
    );
    set_external_output(&mut absent, r#"{"provider_ids":{}}"#);
    harness.apply_input(
        transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        absent,
    );

    let mut explicit_null = external_completed_input(
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_500,
        "external-opaque-1",
        "hello",
    );
    set_external_output(
        &mut explicit_null,
        r#"{"provider_ids":{"continuation_id":null,"request_id":null,"response_id":null}}"#,
    );
    assert_error_code(
        harness.kernel.decide(&empty_env(1_501), explicit_null),
        "conflicting_completion_id",
    );
}

#[test]
fn failed_settlement_is_nonterminal_until_finalize_and_replays_exactly() {
    let mut harness = drive_to_awaiting_model();
    let failed = failed_input(
        TURN_ONE,
        MODEL_REQUEST_ONE,
        EFFECT_ONE,
        "failed-completion-1",
    );
    let decision = harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        failed.clone(),
    );
    assert_eq!(decision_body_names(&decision), ["effect_failed"]);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    assert!(decision.records.iter().all(|record| !matches!(
        record.body(),
        RecordBody::RunCompleted(_) | RecordBody::RunFailed(_)
    )));
    assert!(matches!(
        harness.kernel.state().terminal_candidate.as_ref(),
        Some(TerminalCandidate::Failed { .. })
    ));
    assert_eq!(
        harness.kernel.state().state_hash().expect("failed hash"),
        Digest::from_hex("fbcc1d33300cc026381380a7fdeb9c39e9682e8c885b36d697131e614fb73168")
            .expect("vector")
    );

    let duplicate = harness
        .kernel
        .decide(&empty_env(1_401), failed)
        .expect("equal failed settlement");
    assert!(duplicate.records.is_empty());
    assert!(duplicate.actions.is_empty());

    let before_finalize = replay(&harness.batches);
    assert_eq!(harness.kernel.state(), before_finalize.state());
    assert_eq!(
        harness.kernel.state().state_hash().expect("live hash"),
        before_finalize.state().state_hash().expect("replay hash")
    );

    let finalized = harness.apply_input(
        transition_env(1_500, &[8, 9], &[4], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    );
    assert_eq!(
        decision_body_names(&finalized),
        ["stage_outcome_recorded", "run_failed"]
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::Failed));
    assert_exact_event_trace(&harness, &direct_failure_events());
    let replayed = replay(&harness.batches);
    assert_eq!(harness.kernel.state(), replayed.state());
    assert_eq!(
        harness.kernel.state().state_hash().expect("failed hash"),
        replayed.state().state_hash().expect("failed replay hash")
    );
}

#[test]
fn turn_request_and_effect_correlation_conflicts_use_exact_code() {
    let harness = drive_to_awaiting_model();
    let cases = [
        completed_input(
            999,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_400,
            "wrong-turn",
            "hello",
        ),
        completed_input(
            TURN_ONE,
            999,
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_400,
            "wrong-request",
            "hello",
        ),
        completed_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            999,
            FINAL_MESSAGE_ONE,
            1_400,
            "wrong-effect",
            "hello",
        ),
    ];
    for input in cases {
        assert_error_code(
            harness.kernel.decide(&empty_env(1_400), input),
            "model_settlement_mismatch",
        );
    }
}

#[test]
fn equal_and_conflicting_stage_effect_and_deferral_settlements_are_stable() {
    let mut stage = Harness::default();
    accept(&mut stage);
    let original_stage = stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue);
    stage.apply_input(
        transition_env(1_100, &[2], &[], &[], &[], &[], &[]),
        original_stage.clone(),
    );
    let duplicate = stage
        .kernel
        .decide(&empty_env(1_101), original_stage)
        .expect("equal stage");
    assert!(duplicate.records.is_empty());
    assert_error_code(
        stage.kernel.decide(
            &empty_env(1_102),
            stage_input(
                0,
                Stage::BeforeRun,
                ReducerStageOutcome::Fail(fixture_error("different_stage")),
            ),
        ),
        "conflicting_settlement",
    );

    let mut effect = drive_to_awaiting_model();
    let original_completion = completed_input(
        TURN_ONE,
        MODEL_REQUEST_ONE,
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_400,
        "completion-1",
        "hello",
    );
    effect.apply_input(
        transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        original_completion.clone(),
    );
    let duplicate = effect
        .kernel
        .decide(&empty_env(1_401), original_completion)
        .expect("equal completion");
    assert!(duplicate.records.is_empty());
    assert_error_code(
        effect.kernel.decide(
            &empty_env(1_402),
            completed_input(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "completion-1",
                "different",
            ),
        ),
        "conflicting_completion_id",
    );

    let mut deferred = drive_to_awaiting_model();
    let original = deferred_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "external-job-1");
    deferred.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        original.clone(),
    );
    let duplicate = deferred
        .kernel
        .decide(&empty_env(1_401), original)
        .expect("equal deferral");
    assert!(duplicate.records.is_empty());
    assert_error_code(
        deferred.kernel.decide(
            &empty_env(1_402),
            deferred_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "external-job-2"),
        ),
        "conflicting_settlement",
    );
}

#[test]
fn reused_completion_identity_across_cycles_is_rejected() {
    let mut harness = drive_to_before_finalize();
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
    assert_error_code(
        harness.kernel.decide(
            &transition_env(
                2_300,
                &[17, 18],
                &[7, 8],
                &[],
                &[],
                &[],
                &[FINAL_MESSAGE_TWO],
            ),
            completed_input(
                TURN_TWO,
                MODEL_REQUEST_TWO,
                EFFECT_TWO,
                FINAL_MESSAGE_TWO,
                2_300,
                "completion-1",
                "hello",
            ),
        ),
        "conflicting_completion_id",
    );
}

#[test]
fn failed_external_with_assistant_still_classifies_conflicting_completion_id() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "external-job-presence-order",
        ),
    );
    harness.apply_input(
        transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
        external_completed_input(
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_500,
            "external-presence-order",
            "hello",
        ),
    );

    let mut conflicting =
        external_failed_input(EFFECT_ONE, "external-presence-order", "provider_failed");
    let KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        assistant_message: message_slot,
        ..
    }) = &mut conflicting
    else {
        panic!("external failed input");
    };
    *message_slot = Some(assistant_message(
        FINAL_MESSAGE_TWO,
        1_501,
        "should-not-matter",
    ));

    assert_error_code(
        harness.kernel.decide(&empty_env(1_501), conflicting),
        "conflicting_completion_id",
    );
}

#[test]
fn failed_external_completion_preserves_effect_and_failure_candidate() {
    let mut harness = drive_to_awaiting_model();
    harness.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        deferred_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "external-job-failed",
        ),
    );
    let decision = harness.apply_input(
        transition_env(1_500, &[8], &[4], &[], &[], &[], &[]),
        external_failed_input(EFFECT_ONE, "external-failed-1", "provider_failed"),
    );
    assert_eq!(decision_body_names(&decision), ["effect_failed"]);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    assert_exact_event_trace(&harness, &external_failure_events());
    assert!(matches!(
        harness.kernel.state().terminal_candidate.as_ref(),
        Some(TerminalCandidate::Failed {
            effect_id: Some(effect_id),
            ..
        }) if *effect_id == id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)
    ));
    let replayed = replay(&harness.batches);
    assert_eq!(harness.kernel.state(), replayed.state());
}

#[test]
fn continue_model_reason_is_excluded_from_settlement_identity() {
    let harness = drive_to_before_finalize();
    let env = transition_env(2_000, &[12], &[], &[], &[], &[], &[]);
    let with_reason = harness
        .kernel
        .decide(
            &env,
            stage_input(
                0,
                Stage::BeforeFinalize,
                ReducerStageOutcome::ContinueModel {
                    reason: Some(Arc::from("retry")),
                },
            ),
        )
        .expect("reasoned continuation");
    let without_reason = harness
        .kernel
        .decide(
            &env,
            stage_input(
                0,
                Stage::BeforeFinalize,
                ReducerStageOutcome::ContinueModel { reason: None },
            ),
        )
        .expect("unreasoned continuation");
    assert_eq!(with_reason.records, without_reason.records);
}

fn external_completed_with_evidence(completion_id: &str) -> KernelInput {
    external_completed_with_usage(completion_id, 5)
}

fn external_completed_with_usage(completion_id: &str, total_tokens: u64) -> KernelInput {
    let usage = Usage::try_new(Some(3), Some(2), Some(total_tokens), None, BTreeMap::new())
        .expect("valid usage");
    let blob_digest = Digest::blob_content(b"{}");
    let blob = BlobRef::try_new(
        "external-artifact",
        "application/json",
        2,
        Some(blob_digest),
        Some("result.json"),
    )
    .expect("valid blob ref");
    let artifact = ArtifactRef::try_new(
        id::<ArtifactTag>(301),
        "model_output",
        blob,
        blob_digest,
        Digest::raw_json(b"artifact-scope"),
        Metadata::empty(),
    )
    .expect("valid artifact ref");
    KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion {
            effect_id: id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
            completion_id: Arc::from(completion_id),
            outcome: ExternalEffectOutcome::Completed {
                output: RawJson::parse(r#"{"text":"hello"}"#).expect("external output"),
                usage: Some(usage),
                artifacts: Arc::from([artifact]),
            },
        },
        assistant_message: Some(assistant_message(FINAL_MESSAGE_ONE, 1_500, "hello")),
    })
}

fn assert_duplicate(decision: &Decision) {
    assert!(decision.records.is_empty());
    assert!(decision.actions.is_empty());
    assert!(
        decision
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "duplicate_settlement")
    );
}

fn set_external_output(input: &mut KernelInput, json: &str) {
    let KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion:
            ExternalEffectCompletion {
                outcome: ExternalEffectOutcome::Completed { output, .. },
                ..
            },
        ..
    }) = input
    else {
        panic!("external completed input");
    };
    *output = RawJson::parse(json).expect("external output");
}
