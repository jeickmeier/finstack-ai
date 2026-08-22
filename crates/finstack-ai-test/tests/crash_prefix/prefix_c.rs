async fn recorded_stages(store: &Arc<dyn JournalStore>) -> Vec<Stage> {
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .filter_map(|envelope| match envelope.body() {
            RecordBody::StageOutcomeRecorded(outcome) => Some(outcome.cursor.stage),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn prefix_c1_through_c5() {
    let store = memory_store();
    let coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recovered.state().phase(),
        Some(RunPhase::BeforeRun),
        "C1 missing StageOutcomeRecorded leaves the BeforeRun cursor open"
    );
    assert!(
        recorded_stages(&(Arc::clone(&store) as Arc<dyn JournalStore>))
            .await
            .is_empty(),
        "C1 journal has no StageOutcomeRecorded; chain re-run is asserted in runtime recovery tests"
    );
    assert_legal("C1", recovered.state().phase(), LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recorded_stages(&(Arc::clone(&store) as Arc<dyn JournalStore>)).await,
        vec![Stage::BeforeRun],
        "C2 journal has a recorded BeforeRun"
    );
    assert_eq!(
        recovered.state().phase(),
        Some(RunPhase::PreparingContext),
        "C2 recover advances past the recorded stage"
    );
    assert_legal("C2", recovered.state().phase(), LegalRestore::Retryable);

    let content = b"required-summary";
    let scope = ArtifactScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        run_id: Some(id(3)),
        sensitivity: Sensitivity::Confidential,
    };
    let metadata = ArtifactMetadata {
        kind: Arc::from("compaction-summary"),
        media_type: Arc::from("application/octet-stream"),
        name: Some(Arc::from("summary.bin")),
        attributes: Metadata::empty(),
    };
    let digest = Digest::blob_content(content);
    let artifact = ArtifactRef::try_new(
        finstack_ai_kernel::ArtifactId::from_bytes([3; 16]),
        metadata.kind.as_ref(),
        BlobRef::try_new(
            "blob-1",
            metadata.media_type.as_ref(),
            u64::try_from(content.len()).expect("len"),
            Some(digest),
            metadata.name.as_deref(),
        )
        .expect("blob"),
        digest,
        scope.digest().expect("scope"),
        metadata.attributes.clone(),
    )
    .expect("artifact");
    let limits = ArtifactStoreLimits::default();
    validate_staged_artifact(&scope, content, &metadata, &artifact, &limits)
        .expect("C3 valid artifact");
    let err = validate_staged_artifact(&scope, b"corrupt", &metadata, &artifact, &limits)
        .expect_err("C4 corrupt");
    assert_eq!(
        match err {
            finstack_ai_runtime::artifact::ArtifactError::Integrity { .. } => err.code(),
            other => panic!("expected integrity, got {other:?}"),
        },
        ARTIFACT_INTEGRITY_FAILURE
    );

    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recorded_stages(&(Arc::clone(&store) as Arc<dyn JournalStore>)).await,
        vec![Stage::BeforeRun]
    );
    assert!(
        !recorded_stages(&(Arc::clone(&store) as Arc<dyn JournalStore>))
            .await
            .contains(&Stage::PrepareContext),
        "C5 journal has no PrepareContext StageOutcomeRecorded"
    );
    assert_eq!(
        recovered.state().phase(),
        Some(RunPhase::PreparingContext),
        "C5 missing PrepareContext record leaves the PrepareContext cursor open"
    );
    assert_legal("C5", recovered.state().phase(), LegalRestore::Retryable);
}
