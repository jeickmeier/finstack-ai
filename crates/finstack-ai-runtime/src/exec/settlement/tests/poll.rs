fn poll_assignment(
    source_index: u32,
    effect_ordinal: u64,
) -> (AssignedToolCall, finstack_ai_kernel::EffectRequested) {
    let call = ToolCallBlock::try_new(
        fixed_id(100 + effect_ordinal),
        "echo",
        RawJson::parse(br#"{"value":1}"#).expect("arguments"),
    )
    .expect("tool call");
    let output_contract = finstack_ai_kernel::EffectOutputContract {
        kind: EffectOutputKind::ToolResult,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"tool-result"),
    };
    let requested = finstack_ai_kernel::EffectRequested::try_new(
        fixed_id(effect_ordinal),
        finstack_ai_kernel::EffectKind::Tool,
        None,
        None,
        None,
        output_contract.clone(),
        finstack_ai_kernel::EffectInput::Tool { call: call.clone() },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("effect request");
    let assigned = AssignedToolCall {
        source_index,
        group_index: 0,
        effect_id: fixed_id(effect_ordinal),
        plan: ToolCallPlan::Execute(ValidatedToolCall {
            call,
            tool_id: ToolId::parse("finstack.tools.echo").expect("tool id"),
            component: None,
            output_contract,
            retry_safety: RetrySafety::SafeToRetry,
            deadline: None,
            execution: ToolExecutionMode::Parallel,
            failure_policy: ToolFailurePolicy::ReturnToModel,
        }),
    };
    (assigned, requested)
}

fn poll_deferred(
    effect_ordinal: u64,
    reconciliation: ReconciliationPolicy,
    next_poll_at: Option<Timestamp>,
    expires_at: Option<Timestamp>,
) -> EffectDeferred {
    let (_, requested) = poll_assignment(0, effect_ordinal);
    EffectDeferred {
        effect_id: fixed_id(effect_ordinal),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tools.scripted").expect("component"),
            format!("handle-{effect_ordinal}"),
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation,
        next_poll_at,
        expires_at,
        output_contract: requested.output_contract().clone(),
    }
}

fn poll_requested_call(
    source_index: u32,
    effect_ordinal: u64,
    deferred: Option<EffectDeferred>,
) -> ActiveToolCall {
    let (assigned, requested) = poll_assignment(source_index, effect_ordinal);
    ActiveToolCall {
        assigned,
        status: ActiveToolCallStatus::Requested {
            requested,
            deferred,
        },
    }
}

fn poll_state(calls: Vec<ActiveToolCall>) -> KernelState {
    let assigned = calls
        .iter()
        .map(|call| call.assigned.clone())
        .collect::<Vec<_>>();
    let mut state = KernelState::default();
    state.set_active_tool_batch(Some(ActiveToolBatch::new(
        finstack_ai_kernel::ToolBatchOpened {
            cycle: 0,
            turn_id: fixed_id(1),
            tool_batch_id: fixed_id(2),
            source_message_id: fixed_id(3),
            calls: assigned.into(),
            continuation: ToolBatchContinuation::ContinueModel,
            plan_digest: Digest::raw_json(b"tool-batch-plan"),
        },
        calls,
        0,
        0,
        std::sync::Arc::from([]),
        None,
    )));
    state
}

#[test]
fn due_polls_includes_future_callback_or_poll_deadline() {
    let at = fixed_timestamp(200);
    let state = poll_state(vec![poll_requested_call(
        0,
        10,
        Some(poll_deferred(
            10,
            ReconciliationPolicy::CallbackOrPoll,
            Some(at),
            None,
        )),
    )]);

    let polls: Vec<DuePoll> = due_polls(&state, fixed_timestamp(100));

    assert_eq!(polls.len(), 1);
    assert_eq!(polls[0].effect_id, fixed_id(10));
    assert_eq!(polls[0].at, at);
}

#[test]
fn due_polls_includes_poll_deadline() {
    let at = fixed_timestamp(100);
    let state = poll_state(vec![poll_requested_call(
        0,
        10,
        Some(poll_deferred(
            10,
            ReconciliationPolicy::Poll,
            Some(at),
            None,
        )),
    )]);

    let polls = due_polls(&state, fixed_timestamp(200));

    assert_eq!(polls.len(), 1);
    assert_eq!(polls[0].effect_id, fixed_id(10));
    assert_eq!(polls[0].at, at);
}

#[test]
fn due_polls_excludes_non_pollable_and_non_requested_calls() {
    let at = fixed_timestamp(100);
    let (undispatched, _) = poll_assignment(4, 14);
    let (buffered, _) = poll_assignment(5, 15);
    let buffered_result = finstack_ai_kernel::ToolResultBlock::try_new(
        *buffered.plan.call().tool_call_id(),
        vec![],
        false,
    )
    .expect("buffered result");
    let (settled, _) = poll_assignment(6, 16);
    let state = poll_state(vec![
        poll_requested_call(
            0,
            10,
            Some(poll_deferred(
                10,
                ReconciliationPolicy::CallbackOnly,
                Some(at),
                None,
            )),
        ),
        poll_requested_call(
            1,
            11,
            Some(poll_deferred(
                11,
                ReconciliationPolicy::ExternalWorkflow,
                Some(at),
                None,
            )),
        ),
        poll_requested_call(
            2,
            12,
            Some(poll_deferred(
                12,
                ReconciliationPolicy::Poll,
                None,
                None,
            )),
        ),
        poll_requested_call(3, 13, None),
        ActiveToolCall {
            assigned: undispatched,
            status: ActiveToolCallStatus::Undispatched,
        },
        ActiveToolCall {
            assigned: buffered,
            status: ActiveToolCallStatus::Buffered {
                result: buffered_result,
                settlement_digest: Digest::raw_json(b"buffered"),
                synthetic: false,
                error: None,
            },
        },
        ActiveToolCall {
            assigned: settled,
            status: ActiveToolCallStatus::Settled {
                result_message_id: fixed_id(200),
                settlement_digest: Digest::raw_json(b"settled"),
            },
        },
    ]);

    assert!(due_polls(&state, fixed_timestamp(100)).is_empty());
    assert!(due_polls(&KernelState::default(), fixed_timestamp(100)).is_empty());
}

#[test]
fn expired_compares_present_deadline_inclusively() {
    let now = fixed_timestamp(100);

    assert!(expired(
        &poll_deferred(
            10,
            ReconciliationPolicy::Poll,
            None,
            Some(fixed_timestamp(99)),
        ),
        now,
    ));
    assert!(expired(
        &poll_deferred(10, ReconciliationPolicy::Poll, None, Some(now)),
        now,
    ));
    assert!(!expired(
        &poll_deferred(10, ReconciliationPolicy::Poll, None, None),
        now,
    ));
    assert!(!expired(
        &poll_deferred(
            10,
            ReconciliationPolicy::Poll,
            None,
            Some(fixed_timestamp(101)),
        ),
        now,
    ));
}
