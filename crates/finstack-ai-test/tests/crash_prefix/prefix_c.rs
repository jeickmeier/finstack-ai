#[test]
fn prefix_c1_through_c5() {
    let requested = middleware_request(InvocationRecovery::NonRepeatable);
    assert_eq!(
        middleware_resume_action(&requested, None),
        InvocationResumeAction::SuspendUncertain,
        "C1 missing durable summary is uncertain"
    );

    let completed = EffectCompleted::try_new(
        requested.effect_id(),
        middleware_contract(),
        RawJson::parse(r#"{"ok":true}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("mw-1"),
        None,
    )
    .expect("completed");
    assert_eq!(
        middleware_resume_action(&requested, Some(&completed)),
        InvocationResumeAction::UseRecorded,
        "C2 recorded inline outcome replays"
    );

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
    validate_staged_artifact(&scope, content, &metadata, &artifact).expect("C3 valid artifact");
    let err =
        validate_staged_artifact(&scope, b"corrupt", &metadata, &artifact).expect_err("C4 corrupt");
    assert_eq!(
        match err {
            finstack_ai_runtime::ArtifactError::Integrity { .. } => err.code(),
            other => panic!("expected integrity, got {other:?}"),
        },
        ARTIFACT_INTEGRITY_FAILURE
    );

    let recompute = middleware_request(InvocationRecovery::RecomputeSafe);
    assert_eq!(
        middleware_resume_action(&recompute, None),
        InvocationResumeAction::Recompute,
        "C5 disposable checkpoint rebuilds"
    );
}
