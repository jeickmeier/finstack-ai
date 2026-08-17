#[test]
fn tool_stage_failures_are_valid_only_at_their_frozen_boundaries() {
    let mut before = drive_to_before_tool_batch_matrix();
    let before_decision = before.apply_input(
        transition_env(6_600, &[6_600], &[], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::Fail(fixture_error("before_tool_batch_failed")),
        ),
    );
    assert_eq!(
        decision_body_names(&before_decision),
        ["stage_outcome_recorded"]
    );
    assert_eq!(before.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    assert!(matches!(
        before.kernel.state().terminal_candidate.as_ref(),
        Some(TerminalCandidate::Failed { .. })
    ));

    let mut after = drive_to_after_tool_batch_matrix();
    let after_decision = after.apply_input(
        transition_env(6_700, &[6_700], &[], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::AfterToolBatch,
            ReducerStageOutcome::Fail(fixture_error("after_tool_batch_failed")),
        ),
    );
    assert_eq!(
        decision_body_names(&after_decision),
        ["stage_outcome_recorded"]
    );
    assert_eq!(after.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    assert!(matches!(
        after.kernel.state().terminal_candidate.as_ref(),
        Some(TerminalCandidate::Failed { .. })
    ));
}

#[test]
fn allocated_id_shortage_and_extra_precedence_follow_canonical_queue_order() {
    let mut preparing = Harness::default();
    accept(&mut preparing);
    settle_before_run(&mut preparing);
    let input = stage_input(
        0,
        Stage::PrepareContext,
        ReducerStageOutcome::ContextPrepared {
            messages: Arc::from(context_messages()),
        },
    );

    assert!(matches!(
        preparing.kernel.decide(
            &transition_env(9_600, &[], &[], &[], &[], &[], &[]),
            input.clone()
        ),
        Err(KernelError::AllocatedIdsExhausted { kind: "record_ids" })
    ));
    assert!(matches!(
        preparing.kernel.decide(
            &transition_env(9_601, &[1, 2, 3], &[1], &[], &[1], &[], &[]),
            input,
        ),
        Err(KernelError::UnusedAllocatedIds { kind: "record_ids" })
    ));
}

#[test]
fn assistant_semantics_precede_message_id_allocation() {
    let harness = drive_to_awaiting_model();
    let mut invalid_semantics = completed_input(
        TURN_ONE,
        MODEL_REQUEST_ONE,
        EFFECT_ONE,
        FINAL_MESSAGE_ONE,
        1_399,
        "semantic-precedence",
        "hello",
    );
    let KernelInput::ModelSettled(ModelSettled {
        outcome: ModelSettlement::Completed {
            assistant_message, ..
        },
        ..
    }) = &mut invalid_semantics
    else {
        unreachable!("completed model input")
    };
    assert_eq!(assistant_message.created_at(), timestamp(1_399));
    assert_error_code(
        harness.kernel.decide(
            &transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[]),
            invalid_semantics,
        ),
        "assistant_message_mismatch",
    );
    assert!(matches!(
        harness.kernel.decide(
            &transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[]),
            completed_input(
                TURN_ONE,
                MODEL_REQUEST_ONE,
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_400,
                "missing-message-id",
                "hello",
            ),
        ),
        Err(KernelError::AllocatedIdsExhausted {
            kind: "message_ids"
        })
    ));
}

#[test]
fn run_phase_match_and_wire_vocabulary_are_compiler_exhaustive() {
    let cases = [
        (RunPhase::Accepted, "accepted"),
        (RunPhase::BeforeRun, "before_run"),
        (RunPhase::PreparingContext, "preparing_context"),
        (RunPhase::BeforeModel, "before_model"),
        (RunPhase::AwaitingModel, "awaiting_model"),
        (RunPhase::AfterModel, "after_model"),
        (RunPhase::BeforeToolBatch, "before_tool_batch"),
        (RunPhase::AwaitingTools, "awaiting_tools"),
        (RunPhase::AfterToolBatch, "after_tool_batch"),
        (RunPhase::BeforeFinalize, "before_finalize"),
        (RunPhase::AwaitingInteraction, "awaiting_interaction"),
        (RunPhase::AwaitingExternal, "awaiting_external"),
        (RunPhase::Sleeping, "sleeping"),
        (RunPhase::Cancelling, "cancelling"),
        (RunPhase::Suspended, "suspended"),
        (RunPhase::Completed, "completed"),
        (RunPhase::Failed, "failed"),
        (RunPhase::Cancelled, "cancelled"),
    ];
    for (phase, expected) in cases {
        assert_eq!(phase_wire_name(phase), expected);
        assert_eq!(
            serde_json::to_value(phase).expect("phase JSON"),
            Value::String(expected.to_owned())
        );
    }
}

fn phase_wire_name(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Accepted => "accepted",
        RunPhase::BeforeRun => "before_run",
        RunPhase::PreparingContext => "preparing_context",
        RunPhase::BeforeModel => "before_model",
        RunPhase::AwaitingModel => "awaiting_model",
        RunPhase::AfterModel => "after_model",
        RunPhase::BeforeToolBatch => "before_tool_batch",
        RunPhase::AwaitingTools => "awaiting_tools",
        RunPhase::AfterToolBatch => "after_tool_batch",
        RunPhase::BeforeFinalize => "before_finalize",
        RunPhase::AwaitingInteraction => "awaiting_interaction",
        RunPhase::AwaitingExternal => "awaiting_external",
        RunPhase::Sleeping => "sleeping",
        RunPhase::Cancelling => "cancelling",
        RunPhase::Suspended => "suspended",
        RunPhase::Completed => "completed",
        RunPhase::Failed => "failed",
        RunPhase::Cancelled => "cancelled",
    }
}
