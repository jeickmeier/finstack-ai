// ---- ContextProvider production collect --------------------------------

struct FixtureContextProvider {
    descriptor: ContextProviderDescriptor,
    calls: Arc<AtomicUsize>,
    item: ContextItem,
}

impl ContextProvider for FixtureContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        _request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let item = self.item.clone();
        Box::pin(async move { ContextContribution::try_new(vec![item], None::<&str>) })
    }
}

fn fixture_provider(text: &str, protected: bool) -> (Arc<dyn ContextProvider>, Arc<AtomicUsize>) {
    fixture_provider_named("fixture.context.repository", text, protected, 10)
}

fn fixture_provider_named(
    component: &str,
    text: &str,
    protected: bool,
    estimated_tokens: u64,
) -> (Arc<dyn ContextProvider>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let item = ContextItem::try_new(
        ContextItemKind::QuotedSource,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        ContextProvenance {
            source_id: Arc::from(component),
            source_ref: Some(Arc::from("AGENTS.md")),
            external: true,
        },
        ContextAuthority::Untrusted,
        10,
        estimated_tokens,
        Sensitivity::Internal,
        protected,
    )
    .expect("item");
    let provider: Arc<dyn ContextProvider> = Arc::new(FixtureContextProvider {
        descriptor: ContextProviderDescriptor {
            invocation: finstack_ai_kernel::ComponentInvocation {
                component: finstack_ai_kernel::ComponentId::parse(component).expect("component"),
                version: Version {
                    major: 0,
                    minor: 0,
                    patch: 4,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: finstack_ai_kernel::InvocationRecovery::RecomputeSafe,
            },
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        },
        calls: Arc::clone(&calls),
        item,
    });
    (provider, calls)
}

struct ErrorContextProvider {
    descriptor: ContextProviderDescriptor,
    calls: Arc<AtomicUsize>,
}

impl ContextProvider for ErrorContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        _request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Box::pin(async {
            Err(ContextError::try_new(
                crate::CONTEXT_CONTRIBUTION_INVALID,
                ErrorCategory::Validation,
                "fixture provider failed collect",
                Metadata::empty(),
            )
            .expect("stable context error"))
        })
    }
}

fn error_provider() -> (Arc<dyn ContextProvider>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn ContextProvider> = Arc::new(ErrorContextProvider {
        descriptor: ContextProviderDescriptor {
            invocation: finstack_ai_kernel::ComponentInvocation {
                component: finstack_ai_kernel::ComponentId::parse("fixture.context.error")
                    .expect("component"),
                version: Version {
                    major: 0,
                    minor: 0,
                    patch: 4,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: finstack_ai_kernel::InvocationRecovery::RecomputeSafe,
            },
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        },
        calls: Arc::clone(&calls),
    });
    (provider, calls)
}

#[test]
fn prepare_context_invokes_committed_providers_and_projects_protected() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    let (provider, calls) = fixture_provider("repository-leaf", true);
    coordinator.install_context_providers(Arc::from([provider]));
    let sources = test_sources();

    block_on(settle_facade_stage(
        &mut coordinator,
        None,
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

    assert_eq!(calls.load(Ordering::Acquire), 1, "CommittedContextCall must invoke the provider");
    let committed = coordinator
        .state()
        .current_turn
        .as_ref()
        .expect("current turn");
    let texts: Vec<String> = committed.context.messages.iter().map(message_text).collect();
    assert!(
        texts.iter().any(|text| text.contains("repository-leaf")),
        "provider items must reach the committed PrepareContext messages, got {texts:?}"
    );
    assert_eq!(
        texts.last().map(String::as_str),
        Some("hi"),
        "the current user must stay last so CompactContext can validate"
    );

    let draft = request_draft(committed.context.messages.to_vec(), Vec::new());
    let StageInput::BeforeModel(input) = stage_input(
        coordinator.state(),
        Stage::BeforeModel,
        &model_request_settled(&draft).outcome,
        &test_profile(),
        coordinator.context_projection(),
    )
    .expect("before model input") else {
        panic!("BeforeModel must be the typed stage input");
    };
    assert!(
        input.source_entries.iter().any(|entry| {
            message_text(&entry.message).contains("repository-leaf") && entry.protected
        }),
        "authoritative protected from the provider item must reach source_entries"
    );
    assert!(
        input.source_entries.last().is_some_and(|entry| {
            entry.protected && entry.message.role() == MessageRole::User && message_text(&entry.message) == "hi"
        }),
        "the trailing current user stays structurally protected"
    );
}

fn settle_prepare_context(
    coordinator: &mut CommitCoordinator,
) -> Result<crate::CommitOutcome, RunHandleError> {
    let sources = test_sources();
    block_on(settle_facade_stage(
        coordinator,
        None,
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
}

fn committed_context_texts(coordinator: &CommitCoordinator) -> Vec<String> {
    coordinator
        .state()
        .current_turn
        .as_ref()
        .expect("current turn")
        .context
        .messages
        .iter()
        .map(message_text)
        .collect()
}

#[test]
fn prepare_context_invokes_providers_in_locked_order() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    let (first, first_calls) = fixture_provider_named("fixture.context.first", "first", false, 10);
    let (second, second_calls) =
        fixture_provider_named("fixture.context.second", "second", false, 10);
    coordinator.install_context_providers(Arc::from([first, second]));
    settle_prepare_context(&mut coordinator).expect("settles");
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    let texts = committed_context_texts(&coordinator);
    let first_at = texts.iter().position(|text| text.contains("first"));
    let second_at = texts.iter().position(|text| text.contains("second"));
    assert!(
        first_at.is_some_and(|first| second_at.is_some_and(|second| first < second)),
        "assembled messages must contain first then second, got {texts:?}"
    );
}

#[test]
fn prepare_context_empty_chain_emits_context_prepared_without_provider_calls() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    coordinator.install_context_providers(Arc::from([]));
    settle_prepare_context(&mut coordinator).expect("empty chain settles");
    let texts = committed_context_texts(&coordinator);
    assert_eq!(texts, vec!["hi".to_owned()]);
}

#[test]
fn prepare_context_truncates_when_provider_exceeds_budget() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    let (provider, calls) = fixture_provider_named(
        "fixture.context.over-budget",
        "over-budget",
        false,
        20_000,
    );
    coordinator.install_context_providers(Arc::from([provider]));
    settle_prepare_context(&mut coordinator).expect("over-budget contribution truncates");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let texts = committed_context_texts(&coordinator);
    assert!(
        texts.iter().all(|text| !text.contains("over-budget")),
        "items that exceed the locked profile ceiling must be omitted, got {texts:?}"
    );
    assert_eq!(texts.last().map(String::as_str), Some("hi"));
}

#[test]
fn prepare_context_provider_error_fails_the_stage_with_stable_code() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    let (provider, calls) = error_provider();
    coordinator.install_context_providers(Arc::from([provider]));
    let error = settle_prepare_context(&mut coordinator).expect_err("provider error fails the stage");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(
            &error,
            RunHandleError::Middleware { code }
                if code.as_ref() == crate::CONTEXT_CONTRIBUTION_INVALID
        ),
        "expected the provider's stable code, got {error:?}"
    );
}

#[test]
fn prepare_context_driver_matches_direct_port_conformance_contribution() {
    let (provider, calls) = fixture_provider_named(
        "fixture.context.conformance",
        "conformance-leaf",
        true,
        10,
    );
    let descriptor = provider.descriptor();
    let request = ContextRequest {
        session_id: id::<finstack_ai_kernel::SessionTag>(1),
        lane_id: id::<finstack_ai_kernel::LaneTag>(2),
        run_id: id::<finstack_ai_kernel::RunTag>(3),
        user_input: Arc::from([]),
        recent_history: Arc::from([]),
        budget: crate::ContextBudget {
            max_items: 8,
            max_tokens: 1_024,
            max_bytes: 4_096,
            overflow: crate::ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    };
    let context = ContextCallContext {
        run: crate::RunCallContext {
            locator: finstack_ai_kernel::OperationLocator::try_new(
                "tenant-a",
                request.session_id,
                request.lane_id,
                request.run_id,
            )
            .expect("locator"),
            authorization: crate::AuthorizationContext {
                principal: finstack_ai_kernel::PrincipalRef::try_new(
                    "issuer",
                    "subject",
                    Some("tenant-a"),
                )
                .expect("principal"),
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: id::<finstack_ai_kernel::EffectTag>(4),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
            relation_depth: 0,
        },
        provider_index: 0,
        chain_digest: Digest::raw_json(b"context-chain"),
    };
    let contribution = block_on(provider.collect(context, request)).expect("direct collect");
    assert_eq!(
        provider.descriptor(),
        descriptor,
        "descriptor must stay stable after collect"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let expected_text = message_text(&Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(99),
        MessageRole::User,
        contribution.items[0].content.to_vec(),
        Timestamp::from_unix_ms(1).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message"));

    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_prepare_context(&mut coordinator);
    coordinator.install_context_providers(Arc::from([provider]));
    settle_prepare_context(&mut coordinator).expect("driver collect");
    let texts = committed_context_texts(&coordinator);
    assert!(
        texts.iter().any(|text| text == &expected_text),
        "driver-assembled messages must carry the same normalized contribution, got {texts:?}"
    );
}
