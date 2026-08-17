#[test]
fn ambiguous_ack_retries_identical_request_once() {
    let once = Arc::new(FakeStore::new(FakeMode::AmbiguousOnce));
    let mut coordinator = CommitCoordinator::new(once.clone());
    block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("ambiguous recovery");
    assert_eq!(once.append_calls(), 2);

    let repeated = Arc::new(FakeStore::new(FakeMode::AmbiguousAlways));
    let mut coordinator = CommitCoordinator::new(repeated);
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
}

#[test]
fn sidecar_composition_ambiguous_ack_faults_later_submit() {
    let store = Arc::new(FakeStore::new(FakeMode::AmbiguousAlways));
    let mut coordinator = CommitCoordinator::new(store);
    let prepared = finstack_ai_kernel::ChildRunPrepared {
        parent_run_id: id(1),
        parent_effect_id: id(2),
        child: ChildRunLocator {
            operation: OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator"),
            remote: None,
        },
        request_digest: Digest::raw_json(b"child"),
        placement: ChildPlacement::CompatibleLaneInParentSession,
        budget_reservation_id: None,
    };
    assert!(matches!(
        block_on(coordinator.commit_composition_records(
            id(394),
            vec![session_draft(
                    id(395),
                    id::<SessionTag>(1),
                    id::<LaneTag>(2),
                    timestamp(1_050),
                    RecordBody::ChildRunPrepared(prepared),
                )
                .expect("composition draft")],
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
    assert_eq!(
        coordinator.fault().map(|fault| fault.code),
        Some("continued_ambiguous_acknowledgement")
    );
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )),
        Err(CommitCoordinatorError::Faulted {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
}

#[test]
fn sidecar_ambiguous_ack_faults_later_submit() {
    let store = Arc::new(FakeStore::new(FakeMode::AmbiguousAlways));
    let mut coordinator = CommitCoordinator::new(store);
    assert!(matches!(
        block_on(coordinator.commit_session_records(
            id(390),
            vec![session_draft(
                    id(391),
                    id::<SessionTag>(1),
                    id::<LaneTag>(2),
                    timestamp(1_050),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane draft")],
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
    assert_eq!(
        coordinator.fault().map(|fault| fault.code),
        Some("continued_ambiguous_acknowledgement")
    );
    assert!(matches!(
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )),
        Err(CommitCoordinatorError::Faulted {
            code: "continued_ambiguous_acknowledgement"
        })
    ));
}

#[test]
fn store_integrity_keeps_public_code_and_retains_reason() {
    let store = Arc::new(FakeStore::new(FakeMode::IntegrityAlways));
    let mut coordinator = CommitCoordinator::new(store);
    assert!(matches!(
        block_on(coordinator.commit_session_records(
            id(392),
            vec![session_draft(
                    id(393),
                    id::<SessionTag>(1),
                    id::<LaneTag>(2),
                    timestamp(1_050),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane draft")],
        )),
        Err(CommitCoordinatorError::BoundaryFault {
            code: "store_integrity_uncertain"
        })
    ));
    assert_eq!(
        coordinator.fault().map(|fault| fault.code),
        Some("store_integrity_uncertain")
    );
    assert_eq!(
        coordinator.last_store_reason(),
        Some("store integrity failure: checksum_mismatch")
    );
}
