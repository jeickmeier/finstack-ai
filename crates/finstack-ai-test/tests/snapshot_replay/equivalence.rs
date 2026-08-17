#[test]
fn empty_session_replay_matches_default_state() {
    let store = store();
    let recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("empty recover");
    assert_eq!(
        recovered.state().state_hash().expect("hash"),
        KernelState::default().state_hash().expect("hash")
    );
    assert!(recovered.state().model_settlements.is_empty());
    assert!(recovered.state().completion_identities.is_empty());
}

#[test]
fn snapshot_plus_tail_matches_full_replay_hashes_and_settlements() {
    let store = store();
    populate_settled_session(&store);
    assert_snapshot_matches_full_replay(&store);
    assert!(!block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("v1")
    .state()
    .model_settlements
    .is_empty());
}

#[test]
fn tool_bearing_snapshot_plus_tail_matches_full_replay() {
    let store = store();
    populate_tool_session(&store);
    let recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("tool session");
    assert!(recovered.state().state_version >= 2);
    assert_snapshot_matches_full_replay(&store);
}

#[test]
fn snapshot_restores_model_continuation_sidecar() {
    let store = store();
    populate_settled_session(&store);
    let before_snapshot = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("recover before snapshot");
    let continuation = before_snapshot
        .last_model_continuation()
        .cloned()
        .expect("journal continuation");
    write_current_snapshot(&store);

    let recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("recover from snapshot");
    assert_eq!(recovered.last_model_continuation(), Some(&continuation));
}
