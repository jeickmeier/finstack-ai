// ---- stage inputs -----------------------------------------------------

fn assistant_message(ordinal: u64, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        MessageRole::Assistant,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn state_with_messages(messages: Vec<Message>) -> finstack_ai_kernel::KernelState {
    finstack_ai_kernel::KernelState {
        messages: Arc::new(messages),
        ..finstack_ai_kernel::KernelState::default()
    }
}

#[test]
fn trailing_role_run_selects_only_the_final_contiguous_block() {
    let messages = vec![
        user_message(1, "u1"),
        assistant_message(2, "a1"),
        assistant_message(3, "a2"),
    ];
    let run = trailing_role_run(&messages, MessageRole::Assistant);
    assert_eq!(
        run.iter().map(message_text).collect::<Vec<_>>(),
        vec!["a1".to_owned(), "a2".to_owned()],
        "an earlier User message must terminate the trailing Assistant run"
    );
    assert!(
        trailing_role_run(&messages, MessageRole::Tool).is_empty(),
        "no trailing Tool message means an empty run, not the whole history"
    );
    assert!(trailing_role_run(&[], MessageRole::Tool).is_empty());
}

#[test]
fn after_model_stage_input_is_the_latest_message() {
    let state = state_with_messages(vec![user_message(1, "u1"), assistant_message(2, "a1")]);
    let input = stage_input(
        &state,
        Stage::AfterModel,
        &ReducerStageOutcome::Continue,
        &test_profile(),
        None,
    )
    .expect("after model input");
    assert_eq!(input.stage(), Stage::AfterModel);
    let StageInput::AfterModel { value } = input else {
        panic!("wrong variant");
    };
    let decoded: Message = serde_json::from_slice(value.as_bytes()).expect("one message");
    assert_eq!(message_text(&decoded), "a1");
}

#[test]
fn after_model_stage_input_without_a_message_is_a_stable_error() {
    let error = stage_input(
        &state_with_messages(Vec::new()),
        Stage::AfterModel,
        &ReducerStageOutcome::Continue,
        &test_profile(),
        None,
    )
    .expect_err("no message to observe");
    assert!(matches!(&error, RunHandleError::Middleware { code }
        if code.as_ref() == "middleware_stage_input_invalid"));
}

#[test]
fn after_tool_batch_stage_input_is_the_trailing_tool_result_run() {
    let state = state_with_messages(vec![user_message(1, "u1"), assistant_message(2, "a1")]);
    let input = stage_input(
        &state,
        Stage::AfterToolBatch,
        &ReducerStageOutcome::Continue,
        &test_profile(),
        None,
    )
    .expect("after tool batch input");
    let StageInput::AfterToolBatch { value } = input else {
        panic!("wrong variant");
    };
    let decoded: Vec<Message> = serde_json::from_slice(value.as_bytes()).expect("array");
    assert!(
        decoded.is_empty(),
        "with no trailing tool results the array is empty, not the whole history"
    );
}

#[test]
fn before_finalize_stage_input_is_the_live_terminal_candidate() {
    let candidate = finstack_ai_kernel::TerminalCandidate::Completed {
        cycle: 0,
        turn_id: id(20),
        model_request_id: id(21),
        effect_id: id(22),
        message_id: id(23),
        result_digest: Digest::raw_json(b"{}"),
    };
    let state = finstack_ai_kernel::KernelState {
        terminal_candidate: Some(candidate.clone()),
        ..finstack_ai_kernel::KernelState::default()
    };

    let input = stage_input(
        &state,
        Stage::BeforeFinalize,
        &ReducerStageOutcome::FinalizeAccepted,
        &test_profile(),
        None,
    )
    .expect("before finalize input");
    let StageInput::BeforeFinalize { candidate: value } = input else {
        panic!("wrong variant");
    };
    let decoded: finstack_ai_kernel::TerminalCandidate =
        serde_json::from_slice(value.as_bytes()).expect("candidate");
    assert_eq!(decoded, candidate);

    let error = stage_input(
        &finstack_ai_kernel::KernelState::default(),
        Stage::BeforeFinalize,
        &ReducerStageOutcome::FinalizeAccepted,
        &test_profile(),
        None,
    )
    .expect_err("no candidate to observe");
    assert!(matches!(&error, RunHandleError::Middleware { code }
        if code.as_ref() == "middleware_stage_input_invalid"));
}

#[test]
fn stage_input_refuses_the_one_stage_this_choke_point_does_not_fold() {
    assert!(
        !folds_at(Stage::BeforeToolBatch),
        "BeforeToolBatch never reaches this choke point"
    );
    assert!(
        stage_input(
            &state_with_messages(Vec::new()),
            Stage::BeforeToolBatch,
            &ReducerStageOutcome::Continue,
            &test_profile(),
            None,
        )
        .is_err(),
        "BeforeToolBatch has no stage input to build here"
    );
    assert!(
        folds_at(Stage::BeforeModel),
        "BeforeModel folds now that its typed stage input lands"
    );
}

#[test]
fn before_model_stage_input_is_the_typed_input_over_the_committed_draft() {
    let draft = request_draft(
        vec![assistant_message(5, "hello"), user_message(4, "hi")],
        vec![tool_spec("keep")],
    );
    let settled = model_request_settled(&draft);
    let profile = test_profile();

    let input = stage_input(
        &state_with_messages(Vec::new()),
        Stage::BeforeModel,
        &settled.outcome,
        &profile,
        None,
    )
    .expect("before model input");

    let StageInput::BeforeModel(before_model) = input else {
        panic!("BeforeModel must be the typed stage input");
    };
    assert_eq!(before_model.request, draft, "the draft round trip is exact");
    assert_eq!(before_model.model_context_profile_digest, profile.digest);
    assert_eq!(before_model.hard_input_tokens, FIXTURE_HARD_INPUT_TOKENS);
    assert!(before_model.checkpoint.is_none());
    assert_eq!(
        before_model
            .source_entries
            .iter()
            .map(|entry| entry.entry_id)
            .collect::<Vec<_>>(),
        draft
            .messages
            .iter()
            .map(|message| EntryId::from_bytes(message.id().to_bytes()))
            .collect::<Vec<_>>(),
        "entry ids must use the kernel's own message-to-entry mapping"
    );
    assert!(
        before_model
            .source_entries
            .iter()
            .all(|entry| entry.sensitivity == Sensitivity::Internal)
    );
    assert!(
        before_model.source_entries.last().is_some_and(|entry| {
            entry.protected && entry.message.role() == MessageRole::User
        }),
        "the trailing current user is structurally protected"
    );
    assert!(
        !before_model.source_entries[0].protected,
        "non-trailing history stays unprotected without a provider projection"
    );
    assert_eq!(
        before_model.source_entries[0].provenance_digest,
        Digest::raw_json(
            canonical_message(&draft.messages[0])
                .expect("canonical")
                .as_bytes()
        ),
        "provenance is derived from the message's own canonical bytes"
    );
}

#[test]
fn before_model_stage_input_without_a_prepared_request_is_a_stable_error() {
    let error = stage_input(
        &state_with_messages(Vec::new()),
        Stage::BeforeModel,
        &ReducerStageOutcome::Continue,
        &test_profile(),
        None,
    )
    .expect_err("nothing but a prepared request can carry a draft at this cursor");
    assert!(matches!(&error, RunHandleError::Middleware { code }
        if code.as_ref() == "middleware_stage_input_invalid"));
}
