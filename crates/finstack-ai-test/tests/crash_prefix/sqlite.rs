#[test]
fn journal_v1_family_count_stays_forty() {
    let bodies = all_activated_record_bodies().expect("bodies");
    assert_eq!(bodies.len(), 40);
}

#[tokio::test]
async fn sqlite_v1_opens_prunes_and_process_kill_stays_separate() {
    let path = unique_sqlite_path();
    let store = sqlite_store(path);
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut coordinator).await;
    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &coordinator).await;
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal(
        "sqlite-W2",
        recovered.state().phase,
        LegalRestore::Retryable,
    );
    store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: horizon(),
        })
        .await
        .expect("sqlite prune");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(recovered.state().pending_model_effect.is_some());
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions/stores/finstack-ai-store-sqlite/src/bin/sqlite_fault_helper.rs");
    assert!(
        helper.is_file(),
        "OS kill remains the separate sqlite_fault_helper row"
    );
}
