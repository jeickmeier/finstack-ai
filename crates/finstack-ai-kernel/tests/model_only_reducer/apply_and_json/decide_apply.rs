#[test]
fn decide_is_pure_and_execute_effect_has_exact_preceding_request() {
    let mut harness = Harness::default();
    accept(&mut harness);
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    let before = harness.kernel.state().clone();
    let decision = harness
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
                    deadline: None,
                },
            ),
        )
        .expect("decision");
    assert_eq!(harness.kernel.state(), &before);
    assert_eq!(
        decision_body_names(&decision),
        ["stage_outcome_recorded", "effect_requested"]
    );
    assert_eq!(
        decision.actions,
        [PostCommitAction::ExecuteEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)
        }]
    );
    assert!(matches!(
        decision.records[1].body(),
        RecordBody::EffectRequested(requested)
            if requested.kind() == EffectKind::Model
                && requested.effect_id() == id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)
    ));
}

#[test]
fn apply_rejects_pr009_model_request_relation_and_pipeline_injection() {
    for (field, injected) in [
        (
            "relation",
            json!({
                "parent_effect_id": id::<finstack_ai_kernel::EffectTag>(999),
                "purpose": {
                    "kind": "compaction_summary",
                    "middleware_component_id": "finstack.middleware.fixture"
                }
            }),
        ),
        (
            "pipeline",
            json!({
                "chain_digest": Digest::raw_json(b"pipeline"),
                "stage": "before_model",
                "index": 0
            }),
        ),
    ] {
        let mut harness = Harness::default();
        accept(&mut harness);
        settle_before_run(&mut harness);
        prepare_context(&mut harness, 0, false);
        let mut decision = harness
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
                        deadline: None,
                    },
                ),
            )
            .expect("model request decision");
        let mut draft = serde_json::to_value(&decision.records[1]).expect("request draft");
        draft
            .get_mut("body")
            .and_then(Value::as_object_mut)
            .and_then(|body| body.get_mut("effect_requested"))
            .and_then(Value::as_object_mut)
            .expect("effect request body")
            .insert(field.to_owned(), injected);
        decision.records[1] = serde_json::from_value(draft).expect("shape-valid injected request");
        let batch = commit_decision(&decision, 20_050);
        assert_apply_rejected_without_mutation(&mut harness.kernel, &batch, "invalid_record_order");
    }
}

#[test]
fn model_request_contract_mismatch_is_stable() {
    let mut harness = Harness::default();
    accept(&mut harness);
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    let mut contract = output_contract();
    contract.kind = EffectOutputKind::ToolResult;
    assert_error_code(
        harness.kernel.decide(
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
                    output_contract: contract,
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: None,
                },
            ),
        ),
        "model_request_contract_mismatch",
    );
}

#[test]
fn stage_failure_rejects_programmatically_invalid_descriptor() {
    let mut harness = Harness::default();
    accept(&mut harness);
    let mut error = fixture_error("stage_failed");
    error.message = Arc::from("x".repeat(finstack_ai_kernel::TEXT_MAX_BYTES + 1));
    assert_error_code(
        harness.kernel.decide(
            &empty_env(1_100),
            stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Fail(error)),
        ),
        "invalid_input_payload",
    );
}

#[test]
fn apply_validation_is_transactional_and_sequence_error_is_isolated() {
    let mut range_harness = Harness::default();
    accept(&mut range_harness);
    let decision = range_harness
        .kernel
        .decide(
            &transition_env(1_100, &[2], &[], &[], &[], &[], &[]),
            stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .expect("before-run decision");
    let mut bad_range = commit_decision(&decision, 20_100);
    bad_range.last_sequence += 1;
    assert_apply_rejected_without_mutation(
        &mut range_harness.kernel,
        &bad_range,
        "committed_batch_range_mismatch",
    );

    let mut harness = Harness::default();
    accept(&mut harness);
    settle_before_run(&mut harness);
    let context = harness
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
        .expect("context decision");
    let first = context.expected_sequence;
    let bad_sequence = commit_records(
        first,
        &context.records,
        Some(&[first + 1, first]),
        IdentityOverride::default(),
        20_200,
    );
    assert_eq!(bad_sequence.first_sequence, first);
    assert_eq!(
        bad_sequence.last_sequence,
        first + context.records.len() as u64 - 1,
        "batch range remains internally consistent"
    );
    assert_eq!(bad_sequence.records[0].sequence(), first + 1);
    assert_eq!(bad_sequence.records[1].sequence(), first);
    assert_apply_rejected_without_mutation(
        &mut harness.kernel,
        &bad_sequence,
        "non_contiguous_record_sequence",
    );

    let reversed = context.records.iter().rev().cloned().collect::<Vec<_>>();
    let bad_order = commit_records(first, &reversed, None, IdentityOverride::default(), 20_201);
    assert_apply_rejected_without_mutation(&mut harness.kernel, &bad_order, "invalid_record_order");
}

#[test]
fn apply_reports_context_digest_mismatch_after_structural_shape_passes() {
    let mut harness = Harness::default();
    accept(&mut harness);
    settle_before_run(&mut harness);
    let mut decision = harness
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
        .expect("context decision");
    let tampered = Digest::raw_json(b"tampered-context");
    let mut stage = match decision.records[0].body().clone() {
        RecordBody::StageOutcomeRecorded(stage) => stage,
        other => panic!("unexpected stage body: {other:?}"),
    };
    let StageDisposition::ContextPrepared { context_digest, .. } = &mut stage.disposition else {
        panic!("context disposition");
    };
    *context_digest = tampered;
    let mut context = match decision.records[1].body().clone() {
        RecordBody::ContextPrepared(context) => context,
        other => panic!("unexpected context body: {other:?}"),
    };
    context.context_digest = tampered;
    decision.records[0] = rebuild_draft(
        &decision.records[0],
        RecordBody::StageOutcomeRecorded(stage),
    );
    decision.records[1] = rebuild_draft(&decision.records[1], RecordBody::ContextPrepared(context));
    let batch = commit_decision(&decision, 20_250);
    assert_apply_rejected_without_mutation(&mut harness.kernel, &batch, "context_digest_mismatch");
}

#[test]
fn apply_rejects_transient_sequence_overflow_without_mutation() {
    let mut harness = drive_to_awaiting_model();
    let decision = harness
        .kernel
        .decide(
            &transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
            completed_input(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "sequence-overflow",
                "hello",
            ),
        )
        .expect("completion decision");
    let batch = commit_decision(&decision, 20_300);
    let before = harness.kernel.state().clone();
    assert_error_code(
        harness.kernel.apply(&batch, u64::MAX),
        "invalid_input_payload",
    );
    assert_eq!(harness.kernel.state(), &before);
}

#[test]
fn late_completion_identity_conflict_rolls_back_temporary_apply_state() {
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

    let mut decision = harness
        .kernel
        .decide(
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
                "fresh-completion",
                "hello",
            ),
        )
        .expect("shape-valid completion decision");
    let mut completed_json =
        serde_json::to_value(&decision.records[0]).expect("completed draft JSON");
    completed_json
        .get_mut("body")
        .and_then(Value::as_object_mut)
        .and_then(|body| body.get_mut("effect_completed"))
        .and_then(Value::as_object_mut)
        .expect("effect completed body")
        .insert(
            "completion_id".to_owned(),
            Value::String("completion-1".to_owned()),
        );
    decision.records[0] =
        serde_json::from_value(completed_json).expect("shape-valid changed completion");
    let batch = commit_decision(&decision, 30_000);
    assert_apply_rejected_without_mutation(
        &mut harness.kernel,
        &batch,
        "conflicting_completion_id",
    );
}
