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

