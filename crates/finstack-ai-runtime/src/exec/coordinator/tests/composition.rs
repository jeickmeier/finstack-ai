#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the crash-recovery scenario is clearer as one chronological proof"
)]
fn child_retry_reconciles_ambiguous_reservation_before_invoke() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(FakeStore::with_composition_log(log.clone()));
    let mut commit = CommitCoordinator::new(store);
    block_on(commit.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("accept parent");
    create_compatible_lane(&mut commit, 40, 190, 191);

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
            lane_id: id(40),
            run_id: id(41),
        },
        remote: None,
    };
    let budget = BudgetRequest {
        input_tokens: Some(1_000),
        output_tokens: Some(250),
        cost: None,
        extension_counters: BoundedMap::default(),
    };
    let scope_id = id(42);
    let reservation_id = id(43);
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
    let receipt = BudgetReservationReceipt {
        scope_id,
        reservation_id,
        reserved: budget.clone(),
        remaining: BudgetRequest::default(),
        request_digest: reserve_digest,
        receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
    };
    let ledger = Arc::new(AmbiguousReserveLedger {
        log: log.clone(),
        receipt,
        charge_receipt: None,
        release_receipt: None,
        reserved: Mutex::new(false),
        fail_after_reserve_once: Mutex::new(true),
        reserve_calls: Mutex::new(0),
        charge_calls: Mutex::new(0),
        release_calls: Mutex::new(0),
    });
    let invoker = Arc::new(IdempotentChildInvoker {
        log: log.clone(),
        accepted: Mutex::new(BTreeMap::new()),
        physical_starts: Mutex::new(0),
    });
    let coordinator = ChildRunCoordinator::new(invoker.clone()).with_budget_ledger(ledger.clone());
    let context = ChildRunContext {
        parent: parent.clone(),
        parent_effect_id: id(44),
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
    let request = ChildRunRequest {
        agent: AgentRef {
            id: crate::AgentId::parse("finstack.agent.child").expect("agent id"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
        },
        input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("do the work").expect("text"),
        )]),
        placement: ChildPlacement::CompatibleLaneInParentSession,
        locator: child_locator,
        requested_deadline: Some(timestamp(5_000)),
        requested_budget: budget,
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    };
    let ids = ChildCoordinationIds {
        preparation_batch_id: id(201),
        preparation_record_id: id(202),
        reservation_request_record_id: Some(id(203)),
        reservation_settlement: Some(BudgetOperationIds {
            batch_id: id(204),
            record_id: id(205),
        }),
    };

    assert!(matches!(
        block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            Some(reserve.clone()),
            ids,
            timestamp(1_100),
        )),
        Err(CompositionError::Budget(BudgetError::Unavailable { .. }))
    ));
    assert!(!log.lock().expect("log").contains(&"invoke"));

    let first = block_on(coordinator.start_or_attach(
        &mut commit,
        context.clone(),
        request.clone(),
        Some(reserve.clone()),
        ids,
        timestamp(1_100),
    ))
    .expect("reconcile and invoke");
    let attached = block_on(coordinator.start_or_attach(
        &mut commit,
        context.clone(),
        request.clone(),
        Some(reserve.clone()),
        ids,
        timestamp(1_100),
    ))
    .expect("attach equal retry");
    assert_eq!(first, attached);
    assert_eq!(*ledger.reserve_calls.lock().expect("calls"), 1);
    assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
    assert_eq!(
        log.lock().expect("log").as_slice(),
        &[
            "prepare_committed",
            "reservation_requested",
            "reconcile",
            "reserve",
            "reconcile",
            "reservation_settled",
            "invoke",
            "invoke",
        ]
    );

    let mut conflicting = request;
    conflicting.request_digest = Digest::raw_json(br#"{"request":"child-b"}"#);
    assert!(matches!(
        block_on(coordinator.start_or_attach(
            &mut commit,
            context,
            conflicting,
            Some(reserve),
            ids,
            timestamp(1_100),
        )),
        Err(CompositionError::Commit(
            CommitCoordinatorError::SidecarConflict
        ))
    ));
    assert_eq!(*ledger.reserve_calls.lock().expect("calls"), 1);
    assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "all placement policies share one table-driven handshake proof"
)]
fn every_child_placement_converges_and_rejects_conflicting_digest() {
    let parent = OperationLocator {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
    };
    let remote = crate::RemoteRouteRef {
        service: crate::ComponentRef::new(
            crate::ComponentId::parse("finstack.remote.worker").expect("service"),
            Some(crate::Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        route: crate::ExternalHandleRef::try_new(
            crate::ComponentId::parse("finstack.remote.worker").expect("provider"),
            "route-a",
            RawJson::parse(r#"{"cluster":"a"}"#).expect("route metadata"),
        )
        .expect("route"),
    };
    let cases = [
        (
            ChildPlacement::CompatibleLaneInParentSession,
            ChildRunLocator {
                operation: OperationLocator {
                    tenant_scope: Arc::from("tenant-a"),
                    session_id: id(1),
                    lane_id: id(50),
                    run_id: id(51),
                },
                remote: None,
            },
        ),
        (
            ChildPlacement::IsolatedChildSession,
            ChildRunLocator {
                operation: OperationLocator {
                    tenant_scope: Arc::from("tenant-a"),
                    session_id: id(60),
                    lane_id: id(61),
                    run_id: id(62),
                },
                remote: None,
            },
        ),
        (
            ChildPlacement::RemoteChildSession,
            ChildRunLocator {
                operation: OperationLocator {
                    tenant_scope: Arc::from("tenant-a"),
                    session_id: id(70),
                    lane_id: id(71),
                    run_id: id(72),
                },
                remote: Some(remote),
            },
        ),
    ];

    for (index, (placement, locator)) in cases.into_iter().enumerate() {
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let mut commit = CommitCoordinator::new(store);
        block_on(commit.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        ))
        .expect("accept parent");
        if placement == ChildPlacement::CompatibleLaneInParentSession {
            create_compatible_lane(&mut commit, 50, 190, 191);
        }
        let invoker = Arc::new(IdempotentChildInvoker {
            log: Arc::new(Mutex::new(Vec::new())),
            accepted: Mutex::new(BTreeMap::new()),
            physical_starts: Mutex::new(0),
        });
        let coordinator = ChildRunCoordinator::new(invoker.clone());
        let context = ChildRunContext {
            parent: parent.clone(),
            parent_effect_id: id(80 + u64::try_from(index).expect("index")),
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
        let request = ChildRunRequest {
            agent: AgentRef {
                id: crate::AgentId::parse("finstack.agent.placement").expect("agent id"),
                bundle: None,
                spec_digest: Digest::raw_json(br#"{"agent":"placement"}"#),
            },
            input: Arc::from([]),
            placement,
            locator,
            requested_deadline: None,
            requested_budget: BudgetRequest::default(),
            delegation_id: None,
            metadata: Metadata::empty(),
            request_digest: Digest::raw_json(format!(r#"{{"placement":{index}}}"#).as_bytes()),
        };
        let ordinal = 500 + u64::try_from(index).expect("index") * 10;
        let ids = ChildCoordinationIds {
            preparation_batch_id: id(ordinal),
            preparation_record_id: id(ordinal + 1),
            reservation_request_record_id: None,
            reservation_settlement: None,
        };
        let first = block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            None,
            ids,
            timestamp(1_100),
        ))
        .expect("start child");
        let attached = block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            None,
            ids,
            timestamp(1_100),
        ))
        .expect("attach child");
        assert_eq!(first, attached);
        assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
        assert_eq!(
            commit
                .state()
                .child_preparations
                .get(&context.parent_effect_id)
                .map(|prepared| &prepared.child),
            Some(&request.locator)
        );

        let mut conflicting = request;
        conflicting.request_digest = Digest::raw_json(b"conflicting child request");
        assert!(matches!(
            block_on(coordinator.start_or_attach(
                &mut commit,
                context,
                conflicting,
                None,
                ids,
                timestamp(1_100),
            )),
            Err(CompositionError::Commit(
                CommitCoordinatorError::SidecarConflict
            ))
        ));
        assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
    }
}
