#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one compatibility test keeps all tool-batch wire baseline strict wire and state-v2 invariants together"
)]
fn tool_wire_contracts_state_v2_and_event_ordinals_are_strict() {
    let calls = [call(CALL_A, "alpha")];
    let plan = execute(
        &calls[0],
        ToolExecutionMode::Sequential,
        ToolFailurePolicy::ReturnToModel,
    );
    assert_json_round_trip_and_unknown_fields(&plan);
    assert_json_round_trip_and_unknown_fields(&ReducerStageOutcome::ToolBatchPrepared {
        calls: Arc::from([plan.clone()]),
        continuation: ToolBatchContinuation::Finalize,
    });
    assert_json_round_trip_and_unknown_fields(&KernelInput::ToolBatchSettled(ToolBatchSettled {
        tool_batch_id: id::<ToolBatchTag>(BATCH),
        outcome: ToolSettlement::Completed(tool_completed(TOOL_EFFECT_A, &calls[0])),
    }));
    assert!(serde_json::from_value::<KernelInput>(json!({"future_input": {}})).is_err());
    assert!(serde_json::from_value::<ToolSettlement>(json!({"future": {}})).is_err());

    let mut bounded = serde_json::to_value(ReducerStageOutcome::ToolBatchPrepared {
        calls: vec![plan; finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS].into(),
        continuation: ToolBatchContinuation::Finalize,
    })
    .expect("bounded plan JSON");
    assert!(serde_json::from_value::<ReducerStageOutcome>(bounded.clone()).is_ok());
    let extra = bounded
        .get("tool_batch_prepared")
        .and_then(|value| value.get("calls"))
        .and_then(Value::as_array)
        .and_then(|calls| calls.first())
        .cloned()
        .expect("plan entry");
    bounded
        .get_mut("tool_batch_prepared")
        .and_then(|value| value.get_mut("calls"))
        .and_then(Value::as_array_mut)
        .expect("plan array")
        .push(extra);
    assert!(serde_json::from_value::<ReducerStageOutcome>(bounded).is_err());

    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[5_100, 5_101, 5_102],
            &[5_100],
            &[TOOL_EFFECT_A],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &calls[0],
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    settle_tool(
        &mut harness,
        1_700,
        &[5_110, 5_111, 5_112],
        &[5_110, 5_111, 5_112],
        &[5_120],
        TOOL_EFFECT_A,
        &calls[0],
    );

    let opened: ToolBatchOpened = find_body(&harness.batches, |body| match body {
        RecordBody::ToolBatchOpened(value) => Some(value.clone()),
        _ => None,
    });
    let settled: ToolCallSettled = find_body(&harness.batches, |body| match body {
        RecordBody::ToolCallSettled(value) => Some(value.clone()),
        _ => None,
    });
    let closed: ToolBatchClosed = find_body(&harness.batches, |body| match body {
        RecordBody::ToolBatchClosed(value) => Some(value.clone()),
        _ => None,
    });
    assert_json_round_trip_and_unknown_fields(&opened);
    assert_json_round_trip_and_unknown_fields(&settled);
    assert_json_round_trip_and_unknown_fields(&closed);

    let settled_envelope = harness
        .batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .find(|record| matches!(record.body(), RecordBody::ToolCallSettled(_)))
        .expect("settled envelope");
    assert_eq!(settled_envelope.derived_event_ids().len(), 2);
    assert_eq!(
        RunEvent::try_from_record(settled_envelope, 0, 0)
            .expect("message event")
            .kind(),
        RunEventKind::MessageFinalized
    );
    assert_eq!(
        RunEvent::try_from_record(settled_envelope, 1, 1)
            .expect("tool event")
            .kind(),
        RunEventKind::ToolSettled
    );
    assert!(RunEvent::try_from_record(settled_envelope, 2, 2).is_err());

    let state = harness.kernel.state();
    assert_eq!(state.state_version, 2);
    assert_json_round_trip_and_unknown_fields(state);
    let state_json = serde_json::to_value(state).expect("state v2 JSON");
    for required in [
        "active_tool_batch",
        "tool_calls",
        "tool_settlements",
        "last_tool_batch",
    ] {
        let mut missing = state_json.clone();
        missing
            .as_object_mut()
            .expect("state object")
            .remove(required);
        assert!(
            serde_json::from_value::<finstack_ai_kernel::KernelState>(missing).is_err(),
            "v2 accepted missing {required}"
        );
    }
    for repeated in ["tool_calls", "tool_settlements"] {
        let mut duplicate = state_json.clone();
        let entries = duplicate
            .get_mut(repeated)
            .and_then(Value::as_array_mut)
            .expect("state map projection");
        entries.push(entries[0].clone());
        assert!(
            serde_json::from_value::<finstack_ai_kernel::KernelState>(duplicate).is_err(),
            "v2 accepted duplicate {repeated} key"
        );
    }
    let mut inconsistent = state_json;
    inconsistent
        .as_object_mut()
        .expect("state object")
        .insert("phase".to_owned(), json!("awaiting_tools"));
    assert!(serde_json::from_value::<finstack_ai_kernel::KernelState>(inconsistent).is_err());

    let mut v1 =
        serde_json::to_value(finstack_ai_kernel::KernelState::default()).expect("state v1 JSON");
    v1.as_object_mut()
        .expect("state object")
        .insert("active_tool_batch".to_owned(), Value::Null);
    assert!(serde_json::from_value::<finstack_ai_kernel::KernelState>(v1).is_err());
}
