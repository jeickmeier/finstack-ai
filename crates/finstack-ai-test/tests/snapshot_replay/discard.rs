#[test]
fn deleting_snapshots_still_recovers_from_the_journal() {
    let store = store();
    populate_settled_session(&store);
    write_current_snapshot(&store);
    let expected = recover_hash(&store);
    store
        .discard_snapshot(id::<SessionTag>(1))
        .expect("discard");
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert!(loaded.snapshot.is_none());
    assert!(loaded.accelerated.is_none());
    let rebuilt = block_on(CommitCoordinator::recover(store, id::<SessionTag>(1)))
        .expect("recover without snapshot");
    assert_eq!(rebuilt.state().state_hash().expect("hash"), expected);
}
