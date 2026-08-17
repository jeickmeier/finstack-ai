// ---- BeforeModel: compaction is unreachable until the context port runs

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

/// The causal chain, pinned end to end.
///
/// `protected` is authoritative-from-the-context-port (`context.rs:138`) and
/// a compactor may not set it; the `ContextProvider` port has no production
/// driver, so [`before_model_input`] can only assemble unprotected entries;
/// so `validate_compaction_result` (`middleware.rs:1054-1058`) refuses every
/// result. Wiring `ContextProvider` is the unblock — patching the compactor
/// is not.
///
/// The compaction result here is evidence-correct by construction, and the
/// second half of the test flips **only** `protected` and shows the very
/// same result validating. That is what makes this a proof that the
/// unprotected entry is the sole obstruction, rather than a test that merely
/// observes some rejection.
#[test]
fn compact_context_is_rejected_until_the_context_port_is_driven() {
    let retained = user_message(4, "hi");
    let draft = request_draft(vec![retained.clone()], Vec::new());
    let descriptor = compactor_descriptor("fixture.compactor");
    let input = assembled_before_model_input(&draft);
    assert!(
        input.source_entries.iter().all(|entry| !entry.protected),
        "the assembled input must carry no protected entry"
    );
    let result = evidence_correct_compaction(&input, std::slice::from_ref(&retained));

    let rejected = crate::middleware::validate_compaction_result(&descriptor, &input, &result)
        .expect_err("an unprotected trailing entry cannot satisfy the compaction contract");
    assert_eq!(
        rejected.code(),
        crate::middleware::COMPACTION_RESULT_INVALID
    );

    // Flip only `protected`, recompute the two digests that depend on it,
    // and the identical projection now validates.
    let protected_entries: Arc<[CompactionSourceEntry]> = input
        .source_entries
        .iter()
        .map(|entry| CompactionSourceEntry {
            protected: true,
            ..entry.clone()
        })
        .collect();
    let protected_input = BeforeModelInput {
        source_entries: protected_entries,
        ..input
    };
    let protected_result =
        evidence_correct_compaction(&protected_input, std::slice::from_ref(&retained));
    crate::middleware::validate_compaction_result(&descriptor, &protected_input, &protected_result)
        .expect("with a protected trailing user entry the same projection is valid");
}

/// The same limitation observed through the choke point rather than through
/// the validator: a registered compactor's `CompactContext` fails the run
/// with the validator's own stable code, and never reaches the kernel.
#[test]
fn a_compact_context_chain_fails_the_run_at_the_choke_point() {
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

    let error = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect_err("no source entry can be protected, so compaction cannot validate");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == crate::middleware::COMPACTION_RESULT_INVALID),
        "expected the compaction validator's own stable code, got {error:?}"
    );
    assert!(
        coordinator.state().pending_model_effect.is_none(),
        "a rejected fold must not have committed a model effect"
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
