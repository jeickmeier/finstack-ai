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
        ReducerStageOutcome::ToolBatchPrepared {
            calls: Arc::from([execute(
                &call(CALL_A, "alpha"),
                ToolExecutionMode::Sequential,
                ToolFailurePolicy::ReturnToModel,
            )]),
            continuation: ToolBatchContinuation::Finalize,
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
            .terminal_candidate()
            .expect("completed candidate"),
    );
    assert_json_round_trip_and_unknown_fields(
        completed
            .kernel
            .state()
            .terminal()
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
            .terminal_candidate()
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
            .terminal()
            .expect("failed terminal"),
    );
}

#[test]
fn v1_accept_and_prepare_snapshot_preserves_sidecar_fields() {
    let mut harness = Harness::default();
    harness.apply_input(
        transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
        accept_input(),
    );
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    let state = harness.kernel.state().clone();
    assert_eq!(state.state_version(), 1);
    assert!(state.accepted_at().is_some());
    assert_eq!(state.limit_usage().turns, 1);
    let before_hash = state.state_hash().expect("v1 hash");
    let json = serde_json::to_value(&state).expect("serialize v1");
    assert!(json.get("accepted_at").is_none());
    assert!(json.get("limit_usage").is_none());
    let restored: KernelState = serde_json::from_value(json).expect("restore v1");
    assert_eq!(restored.accepted_at(), state.accepted_at());
    assert_eq!(restored.limit_usage(), state.limit_usage());
    assert_eq!(restored.state_hash().expect("restored hash"), before_hash);
}
