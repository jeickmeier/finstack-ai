#[tokio::test]
async fn prefix_w1_through_w6_snapshot_and_prune() {
    let store = memory_store();
    let accepted = acceptance(3);
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(1),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        Some(id::<RunTag>(3)),
        timestamp(1_000),
        vec![id(1)],
        RecordBody::RunAccepted(accepted),
    )
    .expect("draft");
    store
        .append(
            AppendRequest::try_new(id(101), id::<SessionTag>(1), 1, vec![draft]).expect("append"),
        )
        .await
        .expect("ack");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal("W1", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut coordinator).await;
    let before = coordinator.state().state_hash().expect("hash");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().state_hash().expect("hash"), before);
    assert_legal("W2", recovered.state().phase, LegalRestore::Retryable);

    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    let snapshot = loaded.snapshot.expect("snapshot");
    let mut corrupt = snapshot.bytes().to_vec();
    corrupt[0] ^= 0xff;
    store
        .write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: OpaqueSnapshot::try_new(
                snapshot.sequence(),
                snapshot.digest(),
                corrupt,
                256 * 1024,
            )
            .expect("replacement"),
        })
        .await
        .expect("corrupt write");
    let ignored = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load corrupt");
    assert!(
        ignored.accelerated.is_none(),
        "W3 discards corrupt snapshot"
    );
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().state_hash().expect("hash"), before);
    assert_legal("W3", recovered.state().phase, LegalRestore::Retryable);

    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let accelerated = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    store
        .discard_snapshot(id::<SessionTag>(1))
        .expect("discard");
    let full = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        accelerated.state().state_hash().expect("hash"),
        full.state().state_hash().expect("hash"),
        "W4 snapshot-plus-tail equals full replay"
    );
    assert_eq!(
        accelerated.state().completion_identities,
        full.state().completion_identities
    );

    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    store
        .write_metadata(WriteMetadataRequest {
            session_id: id::<SessionTag>(1),
            expected_head_checksum: loaded.head_checksum,
            metadata: Metadata::parse(br#"{"note":"w5"}"#).expect("metadata"),
        })
        .await
        .expect("metadata");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recovered.state().phase,
        full.state().phase,
        "W5 no authority"
    );
    assert_legal("W5", recovered.state().phase, LegalRestore::Retryable);

    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let receipt = store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: horizon(),
        })
        .await
        .expect("prune");
    assert!(receipt.pruned_through_sequence >= 1);
    assert!(receipt.retained_outstanding >= 1, "W6 outstanding retained");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().state_hash().expect("hash"), before);
    assert!(recovered.state().pending_model_effect.is_some());
    assert_legal("W6", recovered.state().phase, LegalRestore::Retryable);
}
