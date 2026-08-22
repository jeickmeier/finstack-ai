#[tokio::test]
async fn prefix_f1_through_f3() {
    let store = memory_store();
    let recovered = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase(), Some(RunPhase::AfterModel));
    assert_legal("F1", recovered.state().phase(), LegalRestore::Retryable);

    let mut recovered = recovered;
    recovered
        .submit(
            env(1_500, &[9], &[], &[], &[], &[], &[], 106),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase(), Some(RunPhase::BeforeFinalize));
    assert_legal("F2", recovered.state().phase(), LegalRestore::Retryable);

    let mut recovered = recovered;
    recovered
        .submit(
            env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 107),
            stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        )
        .await
        .expect("finalize");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase(), Some(RunPhase::Completed));
    assert_legal("F3", recovered.state().phase(), LegalRestore::Completed);
    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let identities = recovered.state().completion_identities().clone();
    store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: horizon(),
        })
        .await
        .expect("prune settled");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().completion_identities(), &identities);
    assert_eq!(recovered.state().phase(), Some(RunPhase::Completed));
}
