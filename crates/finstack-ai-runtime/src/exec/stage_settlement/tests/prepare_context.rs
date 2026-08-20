// ---- PrepareContext fold ---------------------------------------------

#[test]
fn prepare_context_middleware_adds_messages_to_the_committed_outcome() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    let sources = test_sources();
    let driver = driver_for(
        "fixture.context",
        Stage::PrepareContext,
        StageOutcome::AddContext(Arc::from([item("injected-by-middleware")])),
    );

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([user_message(4, "hi")]),
            },
        },
    ))
    .expect("settles");

    let committed = coordinator
        .state()
        .current_turn
        .as_ref()
        .expect("current turn");
    assert!(
        committed
            .context
            .messages
            .iter()
            .any(|message| message_text(message).contains("injected-by-middleware")),
        "middleware context never reached the committed ContextPrepared"
    );
    assert!(
        committed
            .context
            .messages
            .iter()
            .any(|message| message_text(message) == "hi"),
        "the facade's own base message must survive the fold"
    );
}

// ---- replacement vs additive precedence ------------------------------

#[test]
fn replacement_rebases_and_additive_contributions_apply_on_top() {
    let sources = test_sources();
    let canonical =
        serde_json_canonicalizer::to_vec(&[user_message(11, "replaced")]).expect("canonical");
    let replacement = RawJson::parse(&canonical).expect("raw json");
    let fold = StageFold {
        replacement: Some(replacement),
        context: vec![item("added-after-replace")],
        ..StageFold::default()
    };

    let applied =
        apply_context_prepared(&fold, &[user_message(4, "base")], &sources).expect("applied");

    let texts = applied.iter().map(message_text).collect::<Vec<_>>();
    assert_eq!(
        texts,
        vec!["added-after-replace".to_owned(), "replaced".to_owned()],
        "Replace rebases the base payload; additive contributions still land, \
         inserted before its trailing user message"
    );
}

#[test]
fn additive_contributions_insert_before_the_trailing_user_when_nothing_replaced() {
    let sources = test_sources();
    let fold = StageFold {
        instructions: vec![item("system-add")],
        context: vec![item("context-add")],
        ..StageFold::default()
    };

    let applied = apply_context_prepared(
        &fold,
        &[user_message(3, "history"), user_message(4, "base")],
        &sources,
    )
    .expect("applied");

    assert_eq!(
        applied.iter().map(message_text).collect::<Vec<_>>(),
        vec![
            "history".to_owned(),
            "system-add".to_owned(),
            "context-add".to_owned(),
            "base".to_owned(),
        ],
        "instructions then context insert before the trailing current user, which stays last"
    );
    assert_eq!(applied[1].role(), MessageRole::System);
    assert_eq!(applied[2].role(), MessageRole::User);
    assert_eq!(applied[3].role(), MessageRole::User);
}

/// When the base does not end with a user message (or is empty) there is no
/// current-user slot to preserve, so additions fall back to the tail.
#[test]
fn additive_contributions_append_at_the_tail_without_a_trailing_user() {
    let sources = test_sources();
    let fold = StageFold {
        instructions: vec![item("system-add")],
        context: vec![item("context-add")],
        ..StageFold::default()
    };

    let applied = apply_context_prepared(
        &fold,
        &[user_message(4, "base"), assistant_message(5, "reply")],
        &sources,
    )
    .expect("applied");
    assert_eq!(
        applied.iter().map(message_text).collect::<Vec<_>>(),
        vec![
            "base".to_owned(),
            "reply".to_owned(),
            "system-add".to_owned(),
            "context-add".to_owned(),
        ],
        "a non-user tail is not reshaped; additions append after it"
    );

    let applied_empty = apply_context_prepared(&fold, &[], &sources).expect("applied to empty");
    assert_eq!(
        applied_empty.iter().map(message_text).collect::<Vec<_>>(),
        vec!["system-add".to_owned(), "context-add".to_owned()],
        "an empty base takes the additions as the whole array"
    );
}

#[test]
fn oversized_context_fold_is_a_stable_bounds_error() {
    let sources = test_sources();
    let fold = StageFold {
        context: (0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
            .map(|_| item("x"))
            .collect(),
        ..StageFold::default()
    };

    let error = apply_context_prepared(&fold, &[user_message(4, "base")], &sources)
        .expect_err("base + additions exceed the semantic array bound");
    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == crate::middleware_driver::MIDDLEWARE_STAGE_BOUNDS_EXCEEDED),
        "expected a stable bounds error, got {error:?}"
    );
}
