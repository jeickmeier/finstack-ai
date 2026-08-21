// ---- BeforeModel: sliding-window CompactContext lands; summarize does not

fn compactor_descriptor(component: &str) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        role: MiddlewareRole::ContextCompactor {
            strategy_id: Arc::from("fixture.strategy"),
            strategy_version: 1,
        },
        order: MiddlewareOrder {
            tier: OrderTier::ContextCompaction,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        ..descriptor(component, Stage::BeforeModel)
    }
}

fn validator_descriptor(component: &str) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        role: MiddlewareRole::PostCompactionValidator,
        order: MiddlewareOrder {
            tier: OrderTier::PostCompactionValidation,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        ..descriptor(component, Stage::BeforeModel)
    }
}

fn driver_from(descriptor: MiddlewareDescriptor, outcome: StageOutcome) -> StageDriver {
    let middleware: Arc<dyn crate::middleware::Middleware> = Arc::new(Fixed {
        descriptor,
        outcome,
    });
    StageDriver::new(
        Arc::new(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                .expect("chain"),
        ),
        CancellationSignal::new(),
    )
}

/// The `BeforeModelInput` this choke point actually assembles for `draft`.
fn assembled_before_model_input(draft: &crate::ModelRequestDraft) -> BeforeModelInput {
    let StageInput::BeforeModel(input) = stage_input(
        &state_with_messages(Vec::new()),
        Stage::BeforeModel,
        &model_request_settled(draft).outcome,
        &test_profile(),
        None,
    )
    .expect("before model input") else {
        panic!("BeforeModel must be the typed stage input");
    };
    *input
}

/// A `CompactionResult` whose evidence is computed from `input` and is
/// therefore correct in every respect the validator checks — digests,
/// covered/retained entry ids, token accounting. The *only* contract it can
/// still violate is the one this choke point cannot satisfy.
fn evidence_correct_compaction(
    input: &BeforeModelInput,
    retained: &[Message],
) -> crate::middleware::CompactionResult {
    let replacement_messages: Arc<[Message]> = Arc::from(retained.to_vec());
    let by_message = |message: &Message| {
        input
            .source_entries
            .iter()
            .find(|entry| entry.message.id() == message.id())
            .expect("retained message must come from a source entry")
            .entry_id
    };
    crate::middleware::CompactionResult {
        evidence: crate::middleware::CompactionEvidence {
            strategy_id: Arc::from("fixture.strategy"),
            strategy_version: 1,
            configuration_digest: Digest::raw_json(b"{}"),
            model_context_profile_digest: input.model_context_profile_digest,
            source_digest: crate::middleware::compaction_source_digest(&input.source_entries)
                .expect("source digest"),
            protected_item_set_digest: crate::middleware::compaction_protected_set_digest(
                &input
                    .source_entries
                    .iter()
                    .filter(|entry| entry.protected)
                    .map(|entry| entry.entry_id)
                    .collect::<Vec<_>>(),
            )
            .expect("protected digest"),
            covered_entry_ids: input
                .source_entries
                .iter()
                .map(|entry| entry.entry_id)
                .collect(),
            retained_entry_ids: retained.iter().map(by_message).collect(),
            projection_digest: crate::middleware::compaction_projection_digest(
                &replacement_messages,
            )
            .expect("projection digest"),
            estimated_tokens_before: 10,
            estimated_tokens_after: 5,
            summary_digest: None,
            cache_impact: crate::middleware::PromptCacheImpact::CacheInvalidated,
        },
        replacement_messages,
        derived_summaries: Arc::from([]),
        checkpoint: None,
    }
}

/// Sliding-window `CompactContext` is landable once the trailing current
/// user is structurally protected. `RequestCompactionModel` stays unlandable.
#[test]
fn compact_context_validates_when_the_trailing_user_is_protected() {
    let retained = user_message(4, "hi");
    let draft = request_draft(vec![retained.clone()], Vec::new());
    let descriptor = compactor_descriptor("fixture.compactor");
    let input = assembled_before_model_input(&draft);
    assert!(
        input.source_entries.last().is_some_and(|entry| {
            entry.protected && entry.message.role() == finstack_ai_kernel::MessageRole::User
        }),
        "the trailing current user is structurally protected"
    );
    let result = evidence_correct_compaction(&input, std::slice::from_ref(&retained));
    crate::middleware::validate_compaction_result(&descriptor, &input, &result)
        .expect("a protected trailing user entry makes CompactContext valid");
}

/// The same contract through the choke point: a registered sliding-window
/// `CompactContext` lands and commits the model effect.
#[test]
fn a_compact_context_chain_lands_when_the_trailing_user_is_protected() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let retained = user_message(4, "hi");
    let draft = request_draft(vec![retained.clone()], Vec::new());
    let input = assembled_before_model_input(&draft);
    let driver = driver_from(
        compactor_descriptor("fixture.compactor"),
        StageOutcome::CompactContext(Box::new(evidence_correct_compaction(
            &input,
            std::slice::from_ref(&retained),
        ))),
    );

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect("CompactContext must land once the trailing user is protected");

    assert!(
        coordinator.state().pending_model_effect.is_some(),
        "a landed CompactContext must commit the model effect"
    );
}

/// Composition: a `PrepareContext` `AddInstructions` fold inserts protected
/// System messages *before* the trailing current user, so the no-provider
/// `BeforeModel` source entries still end with a protected user entry and a
/// compactor-role `CompactContext` over that context validates and lands,
/// preserving the injected System items byte-identically.
#[test]
fn prepare_context_instructions_compose_with_a_before_model_compaction() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    let sources = test_sources();
    let instructions_driver = driver_for(
        "fixture.instructions",
        Stage::PrepareContext,
        StageOutcome::AddInstructions(Arc::from([item("policy-instruction")])),
    );

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&instructions_driver),
        &sources,
        &test_profile(),
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        prepare_context_settled(),
    ))
    .expect("prepare context settles with the instructions fold");

    let context_messages = coordinator
        .state()
        .current_turn
        .as_ref()
        .expect("current turn")
        .context
        .messages
        .to_vec();
    assert_eq!(
        context_messages
            .iter()
            .map(message_text)
            .collect::<Vec<_>>(),
        vec!["policy-instruction".to_owned(), "hi".to_owned()],
        "the instruction inserts before the trailing current user"
    );
    assert_eq!(context_messages[0].role(), MessageRole::System);

    let draft = request_draft(context_messages.clone(), Vec::new());
    let input = assembled_before_model_input(&draft);
    // The no-provider structural fallback: the injected System entry is
    // protected, and the current user — last again — is protected too.
    assert!(
        input.source_entries[0].protected
            && input.source_entries[0].message.role() == MessageRole::System,
        "the injected instruction must be a protected System source entry"
    );
    assert!(
        input.source_entries.last().is_some_and(|entry| {
            entry.protected && entry.message.role() == MessageRole::User
        }),
        "the trailing current user must stay last and structurally protected"
    );

    let compactor_driver = driver_from(
        compactor_descriptor("fixture.compactor"),
        StageOutcome::CompactContext(Box::new(evidence_correct_compaction(
            &input,
            &context_messages,
        ))),
    );
    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&compactor_driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect("CompactContext must validate and land over an instruction-bearing context");

    let committed = committed_model_request(&coordinator);
    assert_eq!(
        committed
            .messages
            .iter()
            .map(message_text)
            .collect::<Vec<_>>(),
        vec!["policy-instruction".to_owned(), "hi".to_owned()],
        "the landed compaction must preserve the injected System item byte-identically"
    );
    assert_eq!(committed.messages[0].role(), MessageRole::System);
    assert!(
        coordinator.state().pending_model_effect.is_some(),
        "a landed CompactContext must commit the model effect"
    );
}

#[test]
fn post_compaction_validator_may_not_add_context() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let driver = driver_from(
        validator_descriptor("fixture.validator"),
        StageOutcome::AddContext(Arc::from([item("not-allowed-here")])),
    );
    let draft = request_draft(vec![user_message(4, "hi")], Vec::new());

    let error = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect_err("the post-compaction narrowing must reject AddContext");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == "middleware_outcome_not_allowed"),
        "expected the post-compaction narrowing (middleware.rs:1012) to reject it, got {error:?}"
    );
}

#[test]
fn request_compaction_model_folds_to_unlandable_rather_than_being_dropped() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let driver = driver_from(
        compactor_descriptor("fixture.compactor"),
        StageOutcome::RequestCompactionModel(Box::new(crate::middleware::CompactionModelRequest {
            model: finstack_ai_kernel::ComponentRef::new(
                finstack_ai_kernel::ComponentId::parse("fixture.child-model").expect("component"),
                None,
            ),
            request: request_draft(Vec::new(), Vec::new()),
            budget_scope_id: id(9),
            source_sensitivity: Sensitivity::Internal,
            residency_policy_digest: Digest::raw_json(b"residency"),
            resume_state: RawJson::parse(b"{}").expect("resume"),
        })),
    );
    let draft = request_draft(vec![user_message(4, "hi")], Vec::new());

    let error = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect_err("RequestCompactionModel has no StageSettled landing path");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == MIDDLEWARE_STAGE_UNLANDABLE),
        "expected the stable unlandable code, got {error:?}"
    );
}

fn request_shaping_descriptor(component: &str) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        order: MiddlewareOrder {
            tier: OrderTier::RequestShaping,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        ..descriptor(component, Stage::BeforeModel)
    }
}

fn context_mutation_descriptor(component: &str) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        order: MiddlewareOrder {
            tier: OrderTier::ContextMutation,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        ..descriptor(component, Stage::BeforeModel)
    }
}

fn driver_from_pair(
    first: (MiddlewareDescriptor, StageOutcome),
    second: (MiddlewareDescriptor, StageOutcome),
) -> StageDriver {
    let middleware: Vec<MiddlewareRegistration> = [first, second]
        .into_iter()
        .map(|(descriptor, outcome)| {
            let middleware: Arc<dyn crate::middleware::Middleware> =
                Arc::new(Fixed { descriptor, outcome });
            MiddlewareRegistration { middleware }
        })
        .collect();
    StageDriver::new(
        Arc::new(ResolvedMiddlewareChain::try_new(middleware).expect("chain")),
        CancellationSignal::new(),
    )
}

/// Redaction-shaped `Replace` (request-shaping) plus compaction must fail
/// before settlement rather than silently overwrite either projection.
#[test]
fn redaction_replace_plus_compaction_is_unlandable_before_settlement() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let retained = user_message(4, "hi");
    let draft = request_draft(vec![retained.clone()], Vec::new());
    let substitute = request_draft(vec![user_message(11, "redacted")], Vec::new());
    let input = assembled_before_model_input(&draft);
    let driver = driver_from_pair(
        (
            request_shaping_descriptor("fixture.redaction"),
            StageOutcome::Replace(canonical_draft(&substitute).expect("replacement")),
        ),
        (
            compactor_descriptor("fixture.compactor"),
            StageOutcome::CompactContext(Box::new(evidence_correct_compaction(
                &input,
                std::slice::from_ref(&retained),
            ))),
        ),
    );

    let error = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect_err("Replace + CompactContext must fail closed before settlement");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == MIDDLEWARE_STAGE_UNLANDABLE),
        "expected middleware_stage_unlandable, got {error:?}"
    );
    assert!(
        coordinator.state().pending_model_effect.is_none(),
        "a conflict must not open a model effect"
    );
}

/// Document-ingest-shaped `Replace` (context mutation) plus compaction must
/// likewise fail closed before settlement.
#[test]
fn document_ingest_replace_plus_compaction_is_unlandable_before_settlement() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let retained = user_message(4, "hi");
    let draft = request_draft(vec![retained.clone()], Vec::new());
    let substitute = request_draft(vec![user_message(11, "ingested")], Vec::new());
    let input = assembled_before_model_input(&draft);
    let driver = driver_from_pair(
        (
            context_mutation_descriptor("fixture.document-ingest"),
            StageOutcome::Replace(canonical_draft(&substitute).expect("replacement")),
        ),
        (
            compactor_descriptor("fixture.compactor"),
            StageOutcome::CompactContext(Box::new(evidence_correct_compaction(
                &input,
                std::slice::from_ref(&retained),
            ))),
        ),
    );

    let error = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect_err("document-ingest Replace + CompactContext must fail closed");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == MIDDLEWARE_STAGE_UNLANDABLE),
        "expected middleware_stage_unlandable, got {error:?}"
    );
    assert!(coordinator.state().pending_model_effect.is_none());
}

#[test]
fn redaction_replace_alone_still_lands() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let draft = request_draft(vec![user_message(4, "hi")], Vec::new());
    let substitute = request_draft(vec![user_message(11, "redacted")], Vec::new());
    let driver = driver_from(
        request_shaping_descriptor("fixture.redaction"),
        StageOutcome::Replace(canonical_draft(&substitute).expect("replacement")),
    );

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect("redaction Replace alone must still land");

    assert_eq!(
        committed_model_request(&coordinator)
            .messages
            .iter()
            .map(message_text)
            .collect::<Vec<_>>(),
        vec!["redacted".to_owned()],
    );
    assert!(coordinator.state().pending_model_effect.is_some());
}

#[test]
fn document_ingest_replace_alone_still_lands() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let draft = request_draft(vec![user_message(4, "hi")], Vec::new());
    let substitute = request_draft(vec![user_message(11, "ingested-note")], Vec::new());
    let driver = driver_from(
        context_mutation_descriptor("fixture.document-ingest"),
        StageOutcome::Replace(canonical_draft(&substitute).expect("replacement")),
    );

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect("document-ingest Replace alone must still land");

    assert_eq!(
        committed_model_request(&coordinator)
            .messages
            .iter()
            .map(message_text)
            .collect::<Vec<_>>(),
        vec!["ingested-note".to_owned()],
    );
    assert!(coordinator.state().pending_model_effect.is_some());
}
