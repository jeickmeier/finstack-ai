use super::*;
use finstack_ai_kernel::{JsonBlock, KernelState, ToolCallTag, ToolResultBlock};

#[test]
fn successful_model_only_path_has_exact_records_events_and_final_state() {
    let mut harness = Harness::default();
    let mut phases = Vec::new();

    accept(&mut harness);
    phases.push(harness.kernel.state().phase);
    settle_before_run(&mut harness);
    phases.push(harness.kernel.state().phase);
    prepare_context(&mut harness, 0, false);
    phases.push(harness.kernel.state().phase);
    request_model(&mut harness, 0, false);
    phases.push(harness.kernel.state().phase);
    complete_model(&mut harness, false, "completion-1");
    phases.push(harness.kernel.state().phase);
    let after_model = settle_after_model(&mut harness, 0, false);
    assert!(
        after_model.records.iter().all(|record| !matches!(
            record.body(),
            RecordBody::RunCompleted(_) | RecordBody::RunFailed(_)
        )),
        "after-model settlement must not propose a terminal record"
    );
    phases.push(harness.kernel.state().phase);
    finalize(&mut harness, 0, false);
    phases.push(harness.kernel.state().phase);

    assert_success_phases_and_records(&harness, &phases);
    assert_success_events(&harness);
    assert_success_state_identity_and_indexes(&harness);
    assert_success_turn_and_terminal(&harness);
}

fn assert_success_phases_and_records(harness: &Harness, phases: &[Option<RunPhase>]) {
    assert_eq!(
        phases,
        [
            Some(RunPhase::BeforeRun),
            Some(RunPhase::PreparingContext),
            Some(RunPhase::BeforeModel),
            Some(RunPhase::AwaitingModel),
            Some(RunPhase::AfterModel),
            Some(RunPhase::BeforeFinalize),
            Some(RunPhase::Completed),
        ]
    );
    assert_eq!(
        harness
            .batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .map(|record| record.body().kind_name())
            .collect::<Vec<_>>(),
        [
            "run_accepted",
            "stage_outcome_recorded",
            "stage_outcome_recorded",
            "context_prepared",
            "stage_outcome_recorded",
            "effect_requested",
            "effect_completed",
            "entry_appended",
            "stage_outcome_recorded",
            "stage_outcome_recorded",
            "run_completed",
        ]
    );
}

fn assert_success_events(harness: &Harness) {
    assert_exact_event_trace(harness, &direct_success_events());
}

fn assert_success_state_identity_and_indexes(harness: &Harness) {
    let state = harness.kernel.state();
    assert_eq!(state.state_version, 1);
    assert_eq!(state.last_applied_sequence, 11);
    assert_eq!(
        state.session_id,
        Some(id::<finstack_ai_kernel::SessionTag>(SESSION))
    );
    assert_eq!(state.lane_id, Some(id::<finstack_ai_kernel::LaneTag>(LANE)));
    assert_eq!(
        state.accepted.as_ref().map(RunAccepted::run_id),
        Some(id::<finstack_ai_kernel::RunTag>(RUN))
    );
    assert_eq!(state.accepted.as_ref(), Some(&root_acceptance()));
    assert_eq!(state.phase, Some(RunPhase::Completed));
    assert_eq!(state.cycle, 0);
    assert_eq!(
        state.messages.as_ref(),
        [assistant_message(FINAL_MESSAGE_ONE, 1_400, "hello")]
    );
    assert!(state.pending_model_effect.is_none());
    assert_eq!(state.stage_settlements.len(), 5);
    assert_eq!(state.model_settlements.len(), 1);
    assert_eq!(state.completion_identities.len(), 1);
    for stage in [
        Stage::BeforeRun,
        Stage::PrepareContext,
        Stage::BeforeModel,
        Stage::AfterModel,
        Stage::BeforeFinalize,
    ] {
        assert!(
            state
                .stage_settlements
                .contains_key(&StageCursor { cycle: 0, stage })
        );
    }
    let effect_id = id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE);
    let settlement = state
        .model_settlements
        .get(&effect_id)
        .expect("model settlement fingerprint");
    assert_eq!(settlement.kind, ModelSettlementKind::Completed);
    let identity = state
        .completion_identities
        .get("completion-1")
        .expect("completion identity");
    assert_eq!(identity.effect_id, effect_id);
    assert_eq!(identity.settlement_digest, settlement.digest);
}

fn assert_success_turn_and_terminal(harness: &Harness) {
    let state = harness.kernel.state();
    let turn = state.current_turn.as_ref().expect("completed current turn");
    assert_eq!(turn.cycle, 0);
    assert_eq!(turn.turn_id, id::<finstack_ai_kernel::TurnTag>(TURN_ONE));
    assert_eq!(turn.context.cycle, 0);
    assert_eq!(
        turn.context.turn_id,
        id::<finstack_ai_kernel::TurnTag>(TURN_ONE)
    );
    assert_eq!(turn.context.messages.as_ref(), context_messages());
    assert_eq!(
        turn.context.context_digest,
        canonical_digest(
            "model-context",
            &serde_json::to_value(context_messages()).expect("context JSON")
        )
    );
    assert_eq!(
        turn.model_request_id,
        Some(id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE))
    );
    assert_eq!(
        turn.effect_id,
        Some(id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE))
    );
    assert_eq!(
        turn.final_message_id,
        Some(id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE))
    );

    let completion = completed_effect(EFFECT_ONE, "completion-1", "hello");
    assert!(matches!(
        state.terminal_candidate.as_ref(),
        Some(TerminalCandidate::Completed {
            cycle: 0,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            result_digest,
        }) if *turn_id == id::<finstack_ai_kernel::TurnTag>(TURN_ONE)
            && *model_request_id
                == id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE)
            && *effect_id == id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)
            && *message_id == id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE)
            && *result_digest == completion.output_digest()
    ));
    assert!(matches!(
        state.terminal.as_ref(),
        Some(TerminalState::Completed(value))
            if value.cycle == 0
                && value.turn_id == id::<finstack_ai_kernel::TurnTag>(TURN_ONE)
                && value.model_request_id
                    == id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE)
                && value.effect_id == id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)
                && value.result_message_id
                    == id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE)
                && value.result_digest == completion.output_digest()
    ));
}

#[test]
fn populated_completed_state_hash_matches_independent_jcs_oracle() {
    let completed = drive_to_completed();
    let expected = completed_state_hash_oracle();
    assert_eq!(
        completed.kernel.state().state_hash().expect("state hash"),
        expected
    );
}

#[test]
fn default_and_non_text_state_hash_vectors_use_recursive_explicit_nulls() {
    let default_projection = json!({
        "state_version": 1,
        "last_applied_sequence": 0,
        "session_id": null,
        "lane_id": null,
        "accepted": null,
        "phase": null,
        "cycle": 0,
        "current_turn": null,
        "messages": [],
        "pending_model_effect": null,
        "terminal_candidate": null,
        "stage_settlements": [],
        "model_settlements": [],
        "completion_identities": [],
        "terminal": null,
    });
    assert_eq!(
        KernelState::default().state_hash().expect("default hash"),
        canonical_digest("kernel-state", &default_projection)
    );

    let nested_json = ContentBlock::Json(JsonBlock::new(
        RawJson::parse(r#"{"answer":42}"#).expect("nested JSON"),
    ));
    let tool_result = ContentBlock::ToolResult(
        ToolResultBlock::try_new(id::<ToolCallTag>(901), vec![nested_json], false)
            .expect("tool result"),
    );
    let message = Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(902),
        MessageRole::Tool,
        vec![tool_result],
        timestamp(4_000),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("non-text message");
    let state = KernelState {
        messages: Arc::from([message]),
        ..KernelState::default()
    };
    let non_text_projection = json!({
        "state_version": 1,
        "last_applied_sequence": 0,
        "session_id": null,
        "lane_id": null,
        "accepted": null,
        "phase": null,
        "cycle": 0,
        "current_turn": null,
        "messages": [{
            "id": id::<finstack_ai_kernel::MessageTag>(902),
            "role": "tool",
            "content": [{
                "kind": "tool_result",
                "tool_call_id": id::<ToolCallTag>(901),
                "content": [{"kind": "json", "value": {"answer": 42}}],
                "is_error": false,
            }],
            "created_at": timestamp(4_000),
            "model": null,
            "provider_ids": {
                "request_id": null,
                "response_id": null,
                "continuation_id": null,
            },
            "metadata": {},
        }],
        "pending_model_effect": null,
        "terminal_candidate": null,
        "stage_settlements": [],
        "model_settlements": [],
        "completion_identities": [],
        "terminal": null,
    });
    assert_eq!(
        state.state_hash().expect("non-text hash"),
        canonical_digest("kernel-state", &non_text_projection)
    );
}

struct CompletedHashMaterial {
    context_digest: Digest,
    model_digest: Digest,
    completion: EffectCompleted,
    accepted: Value,
    context_message: Value,
    final_message: Value,
    before_run_digest: Digest,
    prepare_digest: Digest,
    before_model_digest: Digest,
    after_model_digest: Digest,
    finalize_digest: Digest,
}

fn completed_hash_material() -> CompletedHashMaterial {
    let context_digest = canonical_digest(
        "model-context",
        &serde_json::to_value(context_messages()).expect("context messages"),
    );
    let stage_digest = |stage, outcome| {
        canonical_digest(
            "stage-settlement",
            &stage_fingerprint(StageCursor { cycle: 0, stage }, &outcome),
        )
    };
    let completed_input = completed_input(
        TURN_ONE,
        MODEL_REQUEST_ONE,
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_400,
        "completion-1",
        "hello",
    );
    let model_digest = canonical_digest(
        "model-settlement",
        &direct_completed_fingerprint(&completed_input),
    );
    let completion = completed_effect(EFFECT_ONE, "completion-1", "hello");

    let mut accepted = serde_json::to_value(root_acceptance()).expect("accepted JSON");
    make_hash_acceptance_explicit(&mut accepted);
    let mut context_message =
        serde_json::to_value(&context_messages()[0]).expect("context message JSON");
    make_hash_message_explicit(&mut context_message);
    let mut final_message =
        serde_json::to_value(assistant_message(FINAL_MESSAGE_ONE, 1_400, "hello"))
            .expect("assistant message JSON");
    make_hash_message_explicit(&mut final_message);

    let before_run_digest = stage_digest(Stage::BeforeRun, ReducerStageOutcome::Continue);
    let prepare_digest = stage_digest(
        Stage::PrepareContext,
        ReducerStageOutcome::ContextPrepared {
            messages: Arc::from(context_messages()),
        },
    );
    let before_model_digest = stage_digest(
        Stage::BeforeModel,
        ReducerStageOutcome::ModelRequestPrepared {
            request: RawJson::parse(r#"{"messages":[{"role":"user","text":"Say hello."}]}"#)
                .expect("request"),
            component: None,
            output_contract: output_contract(),
            retry_safety: RetrySafety::SafeToRetry,
            deadline: Some(timestamp(8_000)),
        },
    );
    let after_model_digest = stage_digest(Stage::AfterModel, ReducerStageOutcome::Continue);
    let finalize_digest =
        stage_digest(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted);

    CompletedHashMaterial {
        context_digest,
        model_digest,
        completion,
        accepted,
        context_message,
        final_message,
        before_run_digest,
        prepare_digest,
        before_model_digest,
        after_model_digest,
        finalize_digest,
    }
}

fn direct_completed_fingerprint(input: &KernelInput) -> Value {
    let payload = input_payload(input, "model_settled");
    let payload = payload.as_object().expect("model-settled payload");
    let completed = payload
        .get("outcome")
        .and_then(Value::as_object)
        .and_then(|outcome| outcome.get("completed"))
        .and_then(Value::as_object)
        .expect("completed settlement payload");
    let mut completion = completed.get("completion").expect("completion").clone();
    make_completion_explicit(&mut completion);
    let mut assistant_message = completed
        .get("assistant_message")
        .expect("assistant message")
        .clone();
    make_hash_message_explicit(&mut assistant_message);
    json!({
        "direct_completed": {
            "turn_id": payload.get("turn_id").expect("turn id"),
            "model_request_id": payload.get("model_request_id").expect("model request id"),
            "completion": completion,
            "assistant_message": assistant_message,
        }
    })
}

fn stage_fingerprint(cursor: StageCursor, outcome: &ReducerStageOutcome) -> Value {
    match outcome {
        ReducerStageOutcome::Continue => json!({"continue": {"cursor": cursor}}),
        ReducerStageOutcome::ContextPrepared { messages } => {
            let mut messages = serde_json::to_value(messages).expect("stage context messages JSON");
            for message in messages.as_array_mut().expect("message array") {
                make_hash_message_explicit(message);
            }
            json!({"context_prepared": {"cursor": cursor, "messages": messages}})
        }
        ReducerStageOutcome::ModelRequestPrepared {
            request,
            component,
            output_contract,
            retry_safety,
            deadline,
        } => json!({"model_request_prepared": {
            "cursor": cursor,
            "request": request,
            "component": component,
            "output_contract": output_contract,
            "retry_safety": retry_safety,
            "deadline": deadline,
        }}),
        ReducerStageOutcome::FinalizeAccepted => {
            json!({"finalize_accepted": {"cursor": cursor}})
        }
        ReducerStageOutcome::ContinueModel { .. } => {
            json!({"continue_model": {"cursor": cursor}})
        }
        ReducerStageOutcome::Fail(error) => {
            json!({"fail": {"cursor": cursor, "error": error}})
        }
    }
}

fn make_completion_explicit(value: &mut Value) {
    let completion = value.as_object_mut().expect("completion object");
    for field in ["usage", "usage_digest", "completion_id", "reservation_id"] {
        completion.entry(field.to_owned()).or_insert(Value::Null);
    }
    let provider_ids = completion
        .get_mut("provider_ids")
        .and_then(Value::as_object_mut)
        .expect("provider ids");
    for field in ["request_id", "response_id", "continuation_id"] {
        provider_ids.entry(field.to_owned()).or_insert(Value::Null);
    }
}

fn completed_state_hash_oracle() -> Digest {
    let CompletedHashMaterial {
        context_digest,
        model_digest,
        completion,
        accepted,
        context_message,
        final_message,
        before_run_digest,
        prepare_digest,
        before_model_digest,
        after_model_digest,
        finalize_digest,
    } = completed_hash_material();
    let terminal = json!({
        "cycle": 0,
        "turn_id": id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
        "model_request_id": id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
        "effect_id": id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
        "result_message_id": id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
        "result_digest": completion.output_digest(),
    });
    let projection = json!({
        "state_version": 1,
        "last_applied_sequence": 11,
        "session_id": id::<finstack_ai_kernel::SessionTag>(SESSION),
        "lane_id": id::<finstack_ai_kernel::LaneTag>(LANE),
        "accepted": accepted,
        "phase": "completed",
        "cycle": 0,
        "current_turn": {
            "cycle": 0,
            "turn_id": id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
            "context": {
                "cycle": 0,
                "turn_id": id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
                "messages": [context_message],
                "context_digest": context_digest,
            },
            "model_request_id": id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
            "effect_id": id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
            "final_message_id": id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
        },
        "messages": [final_message],
        "pending_model_effect": null,
        "terminal_candidate": {
            "completed": {
                "cycle": 0,
                "turn_id": id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
                "model_request_id": id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
                "effect_id": id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
                "message_id": id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
                "result_digest": completion.output_digest(),
            }
        },
        "stage_settlements": [
            {"cycle": 0, "stage": "after_model", "settlement_digest": after_model_digest},
            {"cycle": 0, "stage": "before_finalize", "settlement_digest": finalize_digest},
            {"cycle": 0, "stage": "before_model", "settlement_digest": before_model_digest},
            {"cycle": 0, "stage": "before_run", "settlement_digest": before_run_digest},
            {"cycle": 0, "stage": "prepare_context", "settlement_digest": prepare_digest},
        ],
        "model_settlements": [{
            "effect_id": id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
            "kind": "completed",
            "settlement_digest": model_digest,
        }],
        "completion_identities": [{
            "completion_id": "completion-1",
            "effect_id": id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE),
            "settlement_digest": model_digest,
        }],
        "terminal": {"completed": terminal},
    });
    canonical_digest("kernel-state", &projection)
}

fn make_hash_message_explicit(message: &mut Value) {
    let object = message.as_object_mut().expect("message object");
    object.insert("model".to_owned(), Value::Null);
    let provider_ids = object
        .get_mut("provider_ids")
        .and_then(Value::as_object_mut)
        .expect("provider ids");
    provider_ids
        .entry("request_id".to_owned())
        .or_insert(Value::Null);
    provider_ids
        .entry("response_id".to_owned())
        .or_insert(Value::Null);
    provider_ids
        .entry("continuation_id".to_owned())
        .or_insert(Value::Null);
}

fn make_hash_acceptance_explicit(accepted: &mut Value) {
    let object = accepted.as_object_mut().expect("accepted object");
    object.insert("effective_deadline".to_owned(), Value::Null);
    let relation = object
        .get_mut("relation")
        .and_then(Value::as_object_mut)
        .expect("relation");
    for field in [
        "parent_run_id",
        "parent_effect_id",
        "budget_scope_id",
        "external_work_ref",
    ] {
        relation.insert(field.to_owned(), Value::Null);
    }
    object
        .get_mut("security")
        .and_then(Value::as_object_mut)
        .expect("security")
        .insert("delegated_from".to_owned(), Value::Null);
    let limits = object
        .get_mut("limits")
        .and_then(Value::as_object_mut)
        .expect("limits");
    for field in [
        "max_model_requests",
        "max_turns",
        "max_tool_calls",
        "max_parallel_tools",
        "max_input_tokens",
        "max_output_tokens",
        "max_context_bytes",
        "max_output_bytes",
        "max_retries",
        "max_wall_time",
        "max_cost",
    ] {
        limits.insert(field.to_owned(), Value::Null);
    }
    limits.insert("extension_counters".to_owned(), json!({}));
}

#[test]
fn transient_chunks_are_not_inputs_and_do_not_change_durable_output() {
    let delta = ModelTextDelta::try_new("hel").expect("delta");
    assert!(
        serde_json::from_value::<KernelInput>(json!({
            "model_text_delta": serde_json::to_value(delta).expect("delta JSON")
        }))
        .is_err(),
        "transient model_text_delta must not deserialize as KernelInput"
    );

    let assembled_two = ["hel", "lo"].concat();
    let assembled_five = ["h", "e", "l", "l", "o"].concat();
    let trace = |assembled: &str| {
        let mut harness = drive_to_awaiting_model();
        harness.apply_input(
            transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
            completed_input(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "completion-1",
                assembled,
            ),
        );
        settle_after_model(&mut harness, 0, false);
        finalize(&mut harness, 0, false);
        harness
    };
    let two = trace(&assembled_two);
    let five = trace(&assembled_five);
    assert_eq!(two.batches, five.batches);
    assert_eq!(two.kernel.state(), five.kernel.state());
    assert_eq!(
        two.kernel.state().state_hash().expect("two chunk hash"),
        five.kernel.state().state_hash().expect("five chunk hash")
    );
}

#[test]
fn continuation_consumes_exact_fresh_ids_and_replays() {
    let mut harness = drive_to_before_finalize();
    let continued = harness.apply_input(
        transition_env(2_000, &[12], &[], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::ContinueModel {
                reason: Some(Arc::from("ask again")),
            },
        ),
    );
    assert_eq!(
        continued
            .records
            .iter()
            .map(RecordDraft::record_id)
            .collect::<Vec<_>>(),
        [id::<finstack_ai_kernel::RecordTag>(12)]
    );

    let prepared = prepare_context(&mut harness, 1, true);
    assert_eq!(
        prepared
            .records
            .iter()
            .map(RecordDraft::record_id)
            .collect::<Vec<_>>(),
        [
            id::<finstack_ai_kernel::RecordTag>(13),
            id::<finstack_ai_kernel::RecordTag>(14),
        ]
    );
    assert!(
        prepared
            .records
            .iter()
            .all(|record| record.derived_event_ids().is_empty())
    );

    let requested = request_model(&mut harness, 1, true);
    assert_eq!(
        requested
            .records
            .iter()
            .map(RecordDraft::record_id)
            .collect::<Vec<_>>(),
        [
            id::<finstack_ai_kernel::RecordTag>(15),
            id::<finstack_ai_kernel::RecordTag>(16),
        ]
    );
    assert_eq!(
        requested.records[1].derived_event_ids(),
        [id::<finstack_ai_kernel::EventTag>(6)]
    );
    assert_eq!(
        requested.actions,
        [PostCommitAction::ExecuteEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(EFFECT_TWO)
        }]
    );

    let pending = harness
        .kernel
        .state()
        .pending_model_effect
        .as_ref()
        .expect("second pending model effect");
    assert_eq!(pending.cycle, 1);
    assert_eq!(pending.turn_id, id::<finstack_ai_kernel::TurnTag>(TURN_TWO));
    assert_eq!(
        pending.model_request_id,
        id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_TWO)
    );
    assert_eq!(
        pending.requested.effect_id(),
        id::<finstack_ai_kernel::EffectTag>(EFFECT_TWO)
    );

    complete_model(&mut harness, true, "completion-2");
    settle_after_model(&mut harness, 1, true);
    finalize(&mut harness, 1, true);
    let replayed = replay(&harness.batches);
    assert_eq!(harness.kernel.state(), replayed.state());
    assert_eq!(
        harness.kernel.state().state_hash().expect("live hash"),
        replayed.state().state_hash().expect("replay hash")
    );
}
