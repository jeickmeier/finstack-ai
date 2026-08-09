use finstack_ai_kernel::{ArtifactRef, ArtifactTag, BlobRef, KernelState};

use super::*;

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

#[test]
fn apply_table_rejects_session_lane_and_run_identity_mismatches() {
    let mut harness = Harness::default();
    accept(&mut harness);
    settle_before_run(&mut harness);
    let decision = harness
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
    let cases = [
        IdentityOverride {
            session: Some(id::<finstack_ai_kernel::SessionTag>(900)),
            ..IdentityOverride::default()
        },
        IdentityOverride {
            lane: Some(id::<finstack_ai_kernel::LaneTag>(901)),
            ..IdentityOverride::default()
        },
        IdentityOverride {
            run: Some(id::<finstack_ai_kernel::RunTag>(902)),
            ..IdentityOverride::default()
        },
    ];
    for (index, identity) in cases.into_iter().enumerate() {
        let timestamp_offset = i64::try_from(index).expect("fixture index fits i64");
        let batch = commit_records(
            decision.expected_sequence,
            &decision.records,
            None,
            identity,
            21_000 + timestamp_offset,
        );
        assert_apply_rejected_without_mutation(
            &mut harness.kernel,
            &batch,
            "record_identity_mismatch",
        );
    }
}

#[test]
fn terminal_state_rejects_later_committed_mutation_without_state_change() {
    let mut terminal = drive_to_completed();
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<finstack_ai_kernel::RecordTag>(999),
        id::<finstack_ai_kernel::SessionTag>(SESSION),
        id::<finstack_ai_kernel::LaneTag>(LANE),
        Some(id::<finstack_ai_kernel::RunTag>(RUN)),
        timestamp(9_999),
        vec![],
        RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
            cursor: StageCursor {
                cycle: 1,
                stage: Stage::BeforeRun,
            },
            disposition: StageDisposition::Continued,
            settlement_digest: canonical_digest(
                "stage-settlement",
                &json!({"terminal": "mutation"}),
            ),
        }),
    )
    .expect("terminal mutation draft");
    let next = terminal.kernel.state().last_applied_sequence + 1;
    let batch = commit_records(next, &[draft], None, IdentityOverride::default(), 29_999);
    assert_apply_rejected_without_mutation(
        &mut terminal.kernel,
        &batch,
        "terminal_state_immutable",
    );
}

#[test]
fn all_new_input_and_record_payload_variants_use_strict_json() {
    assert_input_and_model_settlement_json();
    assert_stage_outcome_json();
    assert_external_outcome_json();
    assert_stage_disposition_json();
    assert_state_and_record_payload_json();
}

#[test]
fn external_outcome_vocabulary_rejects_cancelled_and_future_variants() {
    for value in [
        json!({"cancelled": {"reason": "late"}}),
        json!({"future_outcome": {}}),
    ] {
        assert!(
            serde_json::from_value::<ExternalEffectOutcome>(value).is_err(),
            "non-PR-009 external outcome was accepted"
        );
    }
}

#[test]
fn terminal_state_vocabulary_is_exactly_completed_and_failed() {
    let completed = drive_to_completed();
    let completed_terminal = completed
        .kernel
        .state()
        .terminal
        .as_ref()
        .expect("completed terminal");
    assert_json_round_trip_and_unknown_fields(completed_terminal);

    let failed = TerminalState::Failed(RunFailed {
        cycle: 0,
        turn_id: Some(id::<finstack_ai_kernel::TurnTag>(TURN_ONE)),
        model_request_id: Some(id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE)),
        effect_id: Some(id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)),
        error: fixture_error("failed"),
    });
    assert_json_round_trip_and_unknown_fields(&failed);
    for value in [
        json!({"cancelled": {"reason": null}}),
        json!({"run_cancelled": {}}),
        json!({"future_terminal": {}}),
    ] {
        assert!(serde_json::from_value::<TerminalState>(value).is_err());
    }
}

#[test]
fn reducer_collections_and_completion_ids_enforce_exact_decode_bounds() {
    let external = external_completed_input(
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_400,
        "bounded-completion",
        "hello",
    );
    let mut external_json = serde_json::to_value(&external).expect("external JSON");
    set_external_completion_id(
        &mut external_json,
        "x".repeat(finstack_ai_kernel::LABEL_MAX_BYTES),
    );
    assert!(serde_json::from_value::<KernelInput>(external_json.clone()).is_ok());
    set_external_completion_id(
        &mut external_json,
        "x".repeat(finstack_ai_kernel::LABEL_MAX_BYTES + 1),
    );
    assert!(serde_json::from_value::<KernelInput>(external_json).is_err());

    let mut outcome_json = serde_json::to_value(match external {
        KernelInput::ExternalEffectCompleted(value) => value.completion.outcome,
        _ => unreachable!("external input"),
    })
    .expect("outcome JSON");
    let artifact = serde_json::to_value(test_artifact()).expect("artifact JSON");
    outcome_json
        .get_mut("completed")
        .and_then(Value::as_object_mut)
        .expect("completed outcome")
        .insert(
            "artifacts".to_owned(),
            Value::Array(vec![
                artifact.clone();
                finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
            ]),
        );
    assert!(serde_json::from_value::<ExternalEffectOutcome>(outcome_json.clone()).is_ok());
    outcome_json
        .get_mut("completed")
        .and_then(Value::as_object_mut)
        .expect("completed outcome")
        .get_mut("artifacts")
        .and_then(Value::as_array_mut)
        .expect("artifact array")
        .push(artifact);
    assert!(serde_json::from_value::<ExternalEffectOutcome>(outcome_json).is_err());

    let mut state_json = serde_json::to_value(KernelState::default()).expect("state JSON");
    let message = serde_json::to_value(context_messages()[0].clone()).expect("message");
    state_json.as_object_mut().expect("state object").insert(
        "messages".to_owned(),
        Value::Array(vec![
            message.clone();
            finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS
        ]),
    );
    assert!(serde_json::from_value::<KernelState>(state_json.clone()).is_ok());
    state_json
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .expect("message array")
        .push(message);
    assert!(serde_json::from_value::<KernelState>(state_json).is_err());

    let mut harness = Harness::default();
    accept(&mut harness);
    let record = serde_json::to_value(&harness.batches[0].records[0]).expect("record JSON");
    let mut batch_json = serde_json::to_value(&harness.batches[0]).expect("batch JSON");
    batch_json.as_object_mut().expect("batch object").insert(
        "records".to_owned(),
        Value::Array(vec![
            record.clone();
            finstack_ai_kernel::APPEND_BATCH_MAX_RECORDS
        ]),
    );
    assert!(serde_json::from_value::<CommittedBatch>(batch_json.clone()).is_ok());
    batch_json
        .get_mut("records")
        .and_then(Value::as_array_mut)
        .expect("record array")
        .push(record);
    assert!(serde_json::from_value::<CommittedBatch>(batch_json).is_err());
}

#[test]
fn context_and_retained_completion_ids_enforce_bounds_and_semantics() {
    let message = context_messages()[0].clone();
    let messages = vec![message.clone(); finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS];
    let context_digest = canonical_digest(
        "model-context",
        &serde_json::to_value(&messages).expect("messages JSON"),
    );
    let context_json = json!({
        "cycle": 0,
        "turn_id": id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
        "messages": messages,
        "context_digest": context_digest,
    });
    assert!(serde_json::from_value::<ContextPrepared>(context_json.clone()).is_ok());
    let mut unknown_context = context_json.clone();
    unknown_context
        .as_object_mut()
        .expect("context object")
        .insert("future".to_owned(), Value::Bool(true));
    assert!(serde_json::from_value::<ContextPrepared>(unknown_context).is_err());

    let mut oversized_context = context_json.clone();
    oversized_context
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .expect("context messages")
        .push(serde_json::to_value(message).expect("message JSON"));
    assert!(serde_json::from_value::<ContextPrepared>(oversized_context).is_err());

    let mut mismatched_context = context_json.clone();
    mismatched_context
        .as_object_mut()
        .expect("context object")
        .insert(
            "context_digest".to_owned(),
            serde_json::to_value(Digest::raw_json(b"wrong")).expect("digest JSON"),
        );
    assert!(serde_json::from_value::<ContextPrepared>(mismatched_context).is_err());

    let completion_entry = |completion_id: String| {
        json!({
            "completion_id": completion_id,
            "effect_id": id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
            "settlement_digest": Digest::raw_json(b"settlement"),
        })
    };
    assert!(
        serde_json::from_value::<finstack_ai_kernel::CompletionIdentityHashEntryV1>(
            completion_entry("x".repeat(finstack_ai_kernel::LABEL_MAX_BYTES)),
        )
        .is_ok()
    );
    assert!(
        serde_json::from_value::<finstack_ai_kernel::CompletionIdentityHashEntryV1>(
            completion_entry("x".repeat(finstack_ai_kernel::LABEL_MAX_BYTES + 1)),
        )
        .is_err()
    );
    let mut unknown = completion_entry("valid".to_owned());
    unknown
        .as_object_mut()
        .expect("completion entry")
        .insert("future".to_owned(), Value::Bool(true));
    assert!(
        serde_json::from_value::<finstack_ai_kernel::CompletionIdentityHashEntryV1>(unknown)
            .is_err()
    );

    let mut invalid_state = KernelState::default();
    invalid_state.completion_identities.insert(
        Arc::from(""),
        finstack_ai_kernel::CompletionIdentity {
            effect_id: id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
            settlement_digest: Digest::raw_json(b"settlement"),
        },
    );
    assert!(matches!(
        invalid_state.validate(),
        Err(KernelError::InvalidInputPayload {
            field: "completion_identities",
            ..
        })
    ));
}

fn set_external_completion_id(value: &mut Value, completion_id: String) {
    value
        .get_mut("external_effect_completed")
        .and_then(Value::as_object_mut)
        .and_then(|payload| payload.get_mut("completion"))
        .and_then(Value::as_object_mut)
        .expect("completion")
        .insert("completion_id".to_owned(), Value::String(completion_id));
}

fn assert_input_and_model_settlement_json() {
    let completed_input = completed_input(
        TURN_ONE,
        MODEL_REQUEST_ONE,
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_400,
        "json-completed",
        "hello",
    );
    let deferred_input = deferred_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "json-deferred");
    let failed_kernel_input = failed_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "json-failed");
    let external_completed = external_completed_input(
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_400,
        "json-external",
        "hello",
    );
    let external_failed = external_failed_input(EFFECT_ONE, "json-external-failed", "failed");
    for input in [
        accept_input(),
        stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
        completed_input.clone(),
        deferred_input.clone(),
        failed_kernel_input.clone(),
        external_completed.clone(),
        external_failed.clone(),
    ] {
        assert_json_round_trip_and_unknown_fields(&input);
    }

    let model_settlements = [completed_input, deferred_input, failed_kernel_input]
        .into_iter()
        .map(|input| match input {
            KernelInput::ModelSettled(value) => value.outcome,
            _ => unreachable!("model input"),
        })
        .collect::<Vec<_>>();
    for settlement in model_settlements {
        assert_json_round_trip_and_unknown_fields(&settlement);
    }
}

fn test_artifact() -> ArtifactRef {
    let digest = Digest::blob_content(b"artifact");
    ArtifactRef::try_new(
        id::<ArtifactTag>(990),
        "test_artifact",
        BlobRef::try_new(
            "artifact-blob",
            "application/octet-stream",
            8,
            Some(digest),
            None::<&str>,
        )
        .expect("blob"),
        digest,
        Digest::raw_json(b"scope"),
        Metadata::empty(),
    )
    .expect("artifact")
}

fn assert_stage_outcome_json() {
    let stage_outcomes = [
        ReducerStageOutcome::Continue,
        ReducerStageOutcome::ContextPrepared {
            messages: Arc::from(context_messages()),
        },
        ReducerStageOutcome::ModelRequestPrepared {
            request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
            component: None,
            output_contract: output_contract(),
            retry_safety: RetrySafety::SafeToRetry,
            deadline: None,
        },
        ReducerStageOutcome::FinalizeAccepted,
        ReducerStageOutcome::ContinueModel {
            reason: Some(Arc::from("strict JSON")),
        },
        ReducerStageOutcome::Fail(fixture_error("stage_failed")),
    ];
    for outcome in stage_outcomes {
        let encoded = serde_json::to_value(&outcome).expect("stage outcome JSON");
        if encoded.is_object() {
            assert_json_round_trip_and_unknown_fields(&outcome);
        } else {
            assert_json_round_trip(&outcome);
        }
    }
}

fn assert_external_outcome_json() {
    let external_completed = external_completed_input(
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_400,
        "json-external",
        "hello",
    );
    let external_failed = external_failed_input(EFFECT_ONE, "json-external-failed", "failed");
    let external_outcomes = [
        match external_completed {
            KernelInput::ExternalEffectCompleted(value) => value.completion.outcome,
            _ => unreachable!("external input"),
        },
        match external_failed {
            KernelInput::ExternalEffectCompleted(value) => value.completion.outcome,
            _ => unreachable!("external input"),
        },
    ];
    for outcome in external_outcomes {
        assert_json_round_trip_and_unknown_fields(&outcome);
    }
}

fn assert_stage_disposition_json() {
    let dispositions = [
        StageDisposition::Continued,
        StageDisposition::ContextPrepared {
            turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
            context_digest: Digest::raw_json(b"context"),
        },
        StageDisposition::ModelRequested {
            turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
            model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
            effect_id: id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
        },
        StageDisposition::FinalizeAccepted,
        StageDisposition::ContinueModel { next_cycle: 1 },
        StageDisposition::Failed {
            error: fixture_error("stage_failed"),
        },
    ];
    for disposition in dispositions {
        let encoded = serde_json::to_value(&disposition).expect("disposition JSON");
        if encoded.is_object() {
            assert_json_round_trip_and_unknown_fields(&disposition);
        } else {
            assert_json_round_trip(&disposition);
        }
    }

    for value in [ModelSettlementKind::Completed, ModelSettlementKind::Failed] {
        assert_json_round_trip(&value);
    }
}

fn assert_state_and_record_payload_json() {
    let completed = drive_to_completed();
    let stage: StageOutcomeRecorded = find_body(&completed.batches, |body| match body {
        RecordBody::StageOutcomeRecorded(value) => Some(value.clone()),
        _ => None,
    });
    let context: ContextPrepared = find_body(&completed.batches, |body| match body {
        RecordBody::ContextPrepared(value) => Some(value.clone()),
        _ => None,
    });
    let entry: EntryAppended = find_body(&completed.batches, |body| match body {
        RecordBody::EntryAppended(value) => Some(value.clone()),
        _ => None,
    });
    let run_completed: RunCompleted = find_body(&completed.batches, |body| match body {
        RecordBody::RunCompleted(value) => Some(value.clone()),
        _ => None,
    });

    assert_json_round_trip_and_unknown_fields(completed.kernel.state());
    assert_json_round_trip_and_unknown_fields(&stage);
    assert_json_round_trip_and_unknown_fields(&context);
    assert_json_round_trip_and_unknown_fields(&entry);
    assert_json_round_trip_and_unknown_fields(&run_completed);
    assert_json_round_trip_and_unknown_fields(
        completed
            .kernel
            .state()
            .terminal_candidate
            .as_ref()
            .expect("completed candidate"),
    );
    assert_json_round_trip_and_unknown_fields(
        completed
            .kernel
            .state()
            .terminal
            .as_ref()
            .expect("completed terminal"),
    );
    assert_json_round_trip_and_unknown_fields(&RunFailed {
        cycle: 0,
        turn_id: Some(id::<finstack_ai_kernel::TurnTag>(TURN_ONE)),
        model_request_id: Some(id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE)),
        effect_id: Some(id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)),
        error: fixture_error("provider_failed"),
    });

    let mut failed = drive_to_awaiting_model();
    failed.apply_input(
        transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
        failed_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            "strict-json-failure",
        ),
    );
    assert_json_round_trip_and_unknown_fields(
        failed
            .kernel
            .state()
            .terminal_candidate
            .as_ref()
            .expect("failed candidate"),
    );
    failed.apply_input(
        transition_env(1_500, &[8, 9], &[4], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    );
    assert_json_round_trip_and_unknown_fields(
        failed
            .kernel
            .state()
            .terminal
            .as_ref()
            .expect("failed terminal"),
    );
}
