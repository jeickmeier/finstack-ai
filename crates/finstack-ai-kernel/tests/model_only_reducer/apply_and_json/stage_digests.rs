#[test]
fn every_stage_disposition_reconstructs_and_verifies_its_digest() {
    let mut before_run = Harness::default();
    accept(&mut before_run);
    let continued = before_run
        .kernel
        .decide(
            &transition_env(1_100, &[2], &[], &[], &[], &[], &[]),
            stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .expect("continued");
    assert_tampered_stage_digest_rejected(&mut before_run, continued);

    let mut preparing = Harness::default();
    accept(&mut preparing);
    settle_before_run(&mut preparing);
    let context = preparing
        .kernel
        .decide(
            &transition_env(1_200, &[3, 4], &[], &[], &[TURN_ONE], &[], &[]),
            stage_input(
                0,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from(context_messages()),
                },
            ),
        )
        .expect("context");
    assert_tampered_stage_digest_rejected(&mut preparing, context);

    let mut before_model = Harness::default();
    accept(&mut before_model);
    settle_before_run(&mut before_model);
    prepare_context(&mut before_model, 0, false);
    let requested = before_model
        .kernel
        .decide(
            &transition_env(
                1_300,
                &[5, 6],
                &[2],
                &[EFFECT_ONE],
                &[],
                &[MODEL_REQUEST_ONE],
                &[],
            ),
            stage_input(
                0,
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
                    component: None,
                    output_contract: output_contract(),
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(8_000)),
                },
            ),
        )
        .expect("requested");
    assert_tampered_stage_digest_rejected(&mut before_model, requested);

    let mut finalized = drive_to_before_finalize();
    let accepted = finalized
        .kernel
        .decide(
            &transition_env(1_600, &[10, 11], &[5], &[], &[], &[], &[]),
            stage_input(
                0,
                Stage::BeforeFinalize,
                ReducerStageOutcome::FinalizeAccepted,
            ),
        )
        .expect("finalized");
    assert_tampered_stage_digest_rejected(&mut finalized, accepted);

    let mut continued_model = drive_to_before_finalize();
    let continued = continued_model
        .kernel
        .decide(
            &transition_env(2_000, &[12], &[], &[], &[], &[], &[]),
            stage_input(
                0,
                Stage::BeforeFinalize,
                ReducerStageOutcome::ContinueModel {
                    reason: Some(Arc::from("not semantic")),
                },
            ),
        )
        .expect("continued model");
    assert_tampered_stage_digest_rejected(&mut continued_model, continued);

    let mut failed = Harness::default();
    accept(&mut failed);
    let failure = failed
        .kernel
        .decide(
            &transition_env(1_100, &[2], &[], &[], &[], &[], &[]),
            stage_input(
                0,
                Stage::BeforeRun,
                ReducerStageOutcome::Fail(fixture_error("stage_failed")),
            ),
        )
        .expect("failed stage");
    assert_tampered_stage_digest_rejected(&mut failed, failure);
}

fn assert_tampered_stage_digest_rejected(harness: &mut Harness, mut decision: Decision) {
    let stage_index = decision
        .records
        .iter()
        .position(|record| matches!(record.body(), RecordBody::StageOutcomeRecorded(_)))
        .expect("stage record");
    let mut value = serde_json::to_value(&decision.records[stage_index]).expect("record JSON");
    value
        .get_mut("body")
        .and_then(Value::as_object_mut)
        .and_then(|body| body.get_mut("stage_outcome_recorded"))
        .and_then(Value::as_object_mut)
        .expect("stage outcome")
        .insert(
            "settlement_digest".to_owned(),
            serde_json::to_value(Digest::raw_json(b"tampered")).expect("digest JSON"),
        );
    decision.records[stage_index] = serde_json::from_value(value).expect("tampered stage record");
    let batch = commit_decision(&decision, 31_000);
    assert_apply_rejected_without_mutation(
        &mut harness.kernel,
        &batch,
        "settlement_digest_mismatch",
    );
}

fn rebuild_draft(original: &RecordDraft, body: RecordBody) -> RecordDraft {
    RecordDraft::try_new(
        original.format_version(),
        original.kind_version(),
        original.record_id(),
        original.session_id(),
        original.lane_id(),
        original.run_id(),
        original.timestamp(),
        original.derived_event_ids().to_vec(),
        body,
    )
    .expect("rebuilt draft")
}

