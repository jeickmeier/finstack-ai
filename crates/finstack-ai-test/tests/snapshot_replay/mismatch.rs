#[test]
fn corrupt_and_mismatched_snapshots_are_ignored() {
    let store = store();
    populate_settled_session(&store);
    write_current_snapshot(&store);
    let expected = recover_hash(&store);

    let snapshot = load_session(&store).snapshot.expect("snapshot");
    let mut corrupt = snapshot.bytes().to_vec();
    corrupt[0] ^= 0xff;
    replace_snapshot_bytes(&store, corrupt, snapshot.digest());
    assert!(load_session(&store).snapshot.is_some());
    assert_snapshot_ignored(&store, expected);

    write_current_snapshot(&store);
    let snapshot = load_session(&store).snapshot.expect("snapshot");
    replace_snapshot_bytes(
        &store,
        snapshot.bytes().to_vec(),
        Digest::raw_json(b"wrong-digest"),
    );
    assert_snapshot_ignored(&store, expected);

    write_current_snapshot(&store);
    let snapshot = load_session(&store).snapshot.expect("snapshot");
    let mut value = decode_value(snapshot.bytes()).expect("value");
    if let CanonicalValue::Map(entries) = &mut value {
        for (key, item) in entries.iter_mut() {
            if *key == CanonicalValue::Text("format_version".into()) {
                *item = CanonicalValue::Unsigned(3);
            }
        }
    }
    replace_snapshot_bytes(
        &store,
        encode_value(&value).expect("tamper"),
        snapshot.digest(),
    );
    assert_snapshot_ignored(&store, expected);

    write_current_snapshot(&store);
    let recovered_state = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("state for fork")
    .state()
    .clone();
    let (forked_bytes, forked_digest) = encode_snapshot(
        &recovered_state,
        recovered_state.last_applied_sequence,
        Digest::raw_json(b"forked-head"),
        None,
        None,
    )
    .expect("forked envelope");
    replace_snapshot_bytes(&store, forked_bytes, forked_digest);
    assert!(load_session(&store).accelerated.is_some());
    assert_eq!(recover_hash(&store), expected);
}
