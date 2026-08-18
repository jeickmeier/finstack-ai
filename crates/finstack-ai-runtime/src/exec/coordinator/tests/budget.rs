#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the post-commit charge and release proof intentionally covers one lifecycle"
)]
fn budget_charge_and_release_are_post_commit_and_idempotent() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(FakeStore::new(FakeMode::Normal));
    let dispatcher = Arc::new(RecordingDispatcher::default());
    let mut commit = CommitCoordinator::with_test_dispatcher(store, dispatcher);
    block_on(commit.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("accept parent");
    create_compatible_lane(&mut commit, 340, 390, 391);

    let parent = OperationLocator {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
    };
    let child_locator = ChildRunLocator {
        operation: OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(340),
            run_id: id(341),
        },
        remote: None,
    };
    let budget = BudgetRequest {
        input_tokens: Some(1_000),
        output_tokens: Some(250),
        cost: None,
        extension_counters: Default::default(),
    };
    let scope_id = id(342);
    let reservation_id = id(343);
    let reserve_digest = BudgetReserveRequest::compute_digest(
        scope_id,
        reservation_id,
        child_locator.operation.run_id,
        &budget,
    )
    .expect("reserve digest");
    let reserve = BudgetReserveRequest {
        scope_id,
        reservation_id,
        run_id: child_locator.operation.run_id,
        amount: budget.clone(),
        request_digest: reserve_digest,
    };
    let reserve_receipt = BudgetReservationReceipt {
        scope_id,
        reservation_id,
        reserved: budget.clone(),
        remaining: BudgetRequest::default(),
        request_digest: reserve_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
    };
    let usage = Usage::try_new(Some(20), Some(10), Some(30), None, BTreeMap::new()).expect("usage");
    let usage_digest = Digest::effect_output(&usage.canonical_bytes().expect("usage bytes"));
    let charge_request = BudgetChargeRequest {
        scope_id,
        reservation_id,
        effect_id: id(103),
        usage: usage.clone(),
        usage_digest,
    };
    let charge_receipt = BudgetChargeReceipt {
        scope_id,
        reservation_id,
        effect_id: id(103),
        charged_usage: usage.clone(),
        cumulative_usage: usage.clone(),
        usage_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"charge"}"#),
    };
    let release_digest = BudgetReleaseRequest::compute_digest(scope_id, reservation_id, id(3))
        .expect("release digest");
    let release_request = BudgetReleaseRequest {
        scope_id,
        reservation_id,
        terminal_run_id: id(3),
        request_digest: release_digest,
    };
    let release_receipt = BudgetReleaseReceipt {
        scope_id,
        reservation_id,
        terminal_run_id: id(3),
        released_unused: BudgetRequest::default(),
        request_digest: release_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"release"}"#),
    };
    let ledger = Arc::new(AmbiguousReserveLedger {
        log: log.clone(),
        receipt: reserve_receipt,
        charge_receipt: Some(charge_receipt.clone()),
        release_receipt: Some(release_receipt.clone()),
        reserved: Mutex::new(false),
        fail_after_reserve_once: Mutex::new(false),
        reserve_calls: Mutex::new(0),
        charge_calls: Mutex::new(0),
        release_calls: Mutex::new(0),
    });
    let invoker = Arc::new(IdempotentChildInvoker {
        log,
        accepted: Mutex::new(BTreeMap::new()),
        physical_starts: Mutex::new(0),
    });
    let child_coordinator = ChildRunCoordinator::new(invoker).with_budget_ledger(ledger.clone());
    let child_context = ChildRunContext {
        parent: parent.clone(),
        parent_effect_id: id(344),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authentication_method: Arc::from("oidc"),
            assurance_level: Arc::from("high"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("policy-v1"),
            decision_id: Arc::from("decision-v1"),
        },
    };
    let child_request = ChildRunRequest {
        agent: AgentRef {
            id: crate::AgentId::parse("finstack.agent.budget-child").expect("agent id"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"budget-child"}"#),
        },
        input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("budgeted work").expect("text"),
        )]),
        placement: ChildPlacement::CompatibleLaneInParentSession,
        locator: child_locator,
        requested_deadline: None,
        requested_budget: budget,
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"budget-child"}"#),
    };
    block_on(child_coordinator.start_or_attach(
        &mut commit,
        child_context,
        child_request,
        Some(reserve),
        ChildCoordinationIds {
            preparation_batch_id: id(401),
            preparation_record_id: id(402),
            reservation_request_record_id: Some(id(403)),
            reservation_settlement: Some(BudgetOperationIds {
                batch_id: id(404),
                record_id: id(405),
            }),
        },
        timestamp(1_050),
    ))
    .expect("prepare budgeted child");

    let budget_coordinator = BudgetCoordinator::new(ledger.clone());
    assert!(matches!(
        block_on(budget_coordinator.charge_committed(
            &mut commit,
            &parent,
            charge_request.clone(),
            BudgetOperationIds {
                batch_id: id(406),
                record_id: id(407),
            },
            timestamp(1_350),
        )),
        Err(CompositionError::InvalidRequest {
            code: "effect_usage_not_committed"
        })
    ));
    assert_eq!(*ledger.charge_calls.lock().expect("calls"), 0);

    block_on(drive_accepted_to_model_request(&mut commit));
    let completion = EffectCompleted::try_new(
        id(103),
        output_contract(),
        RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
        Some(usage),
        vec![],
        ProviderIds::empty(),
        Some("budget-completion"),
        None,
    )
    .expect("completion");
    let assistant_message = Message::try_new(
        id(504),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant message");
    block_on(commit.submit(
        env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[504], 105),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: id(101),
            model_request_id: id(102),
            outcome: ModelSettlement::Completed {
                completion,
                assistant_message,
            },
        }),
    ))
    .expect("settle model");

    let charge_ids = BudgetOperationIds {
        batch_id: id(406),
        record_id: id(407),
    };
    let charged = block_on(budget_coordinator.charge_committed(
        &mut commit,
        &parent,
        charge_request.clone(),
        charge_ids,
        timestamp(1_450),
    ))
    .expect("charge committed usage");
    assert_eq!(charged, charge_receipt);
    assert_eq!(
        block_on(budget_coordinator.charge_committed(
            &mut commit,
            &parent,
            charge_request,
            charge_ids,
            timestamp(1_450),
        ))
        .expect("equal charge retry"),
        charge_receipt
    );
    assert_eq!(*ledger.charge_calls.lock().expect("calls"), 1);

    block_on(commit.submit(
        env(1_500, &[9], &[], &[], &[], &[], &[], 106),
        stage(Stage::AfterModel, ReducerStageOutcome::Continue),
    ))
    .expect("after model");
    block_on(commit.submit(
        env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 107),
        stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
    ))
    .expect("terminal commit");

    let release_ids = BudgetOperationIds {
        batch_id: id(408),
        record_id: id(409),
    };
    let released = block_on(budget_coordinator.release_committed(
        &mut commit,
        &parent,
        release_request.clone(),
        release_ids,
        timestamp(1_650),
    ))
    .expect("release after terminal");
    assert_eq!(released, release_receipt);
    assert_eq!(
        block_on(budget_coordinator.release_committed(
            &mut commit,
            &parent,
            release_request,
            release_ids,
            timestamp(1_650),
        ))
        .expect("equal release retry"),
        release_receipt
    );
    assert_eq!(*ledger.release_calls.lock().expect("calls"), 1);
}
