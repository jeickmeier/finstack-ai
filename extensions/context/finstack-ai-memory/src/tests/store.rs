use crate::record::*;
use crate::store::*;
use finstack_ai_embeddings::embedder::{
    EmbedError, HashEmbedder, TextEmbedder, TextEmbedderDescriptor,
};
use finstack_ai_embeddings::vector::EmbeddingVector;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use finstack_ai_kernel::{Metadata, RunId, Sensitivity, SessionId, Timestamp};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, ArtifactStoreLimits, stage_required_artifact,
};

fn controlled_store(now: Arc<AtomicI64>, limits: MemoryStoreLimits) -> InProcessMemoryStore {
    InProcessMemoryStore::new()
        .with_clock(Arc::new(move || {
            Timestamp::from_unix_ms(now.load(Ordering::SeqCst)).unwrap()
        }))
        .with_limits(limits)
}

#[tokio::test]
async fn put_is_idempotent_by_key() {
    let store = InProcessMemoryStore::new();
    let record = crate::tests::sample_record("m1", "t1");
    let first = store.put(Arc::from("k1"), record.clone()).await.unwrap();
    let second = store.put(Arc::from("k1"), record).await.unwrap();
    assert_eq!(first, PutOutcome::Inserted);
    assert_eq!(second, PutOutcome::AlreadyApplied);
}

#[tokio::test]
async fn identical_ids_are_isolated_by_exact_scope() {
    let store = InProcessMemoryStore::new();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    let other_scope = store
        .put(Arc::from("k2"), crate::tests::sample_record("m1", "t2"))
        .await
        .unwrap();
    assert_eq!(other_scope, PutOutcome::Inserted);

    // Tenant 1's record is untouched.
    let survivor = store
        .get(
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(survivor.scope.tenant(), "t1");
    let other = store
        .get(
            MemoryScope::try_new("t2").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(other.scope.tenant(), "t2");
}

#[tokio::test]
async fn put_rejects_same_scope_overwrite_under_a_new_key() {
    let store = InProcessMemoryStore::new();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    let mut replacement = crate::tests::sample_record("m1", "t1");
    replacement.preview = Arc::from("clobbered");
    replacement.body = MemoryBody::Inline(Arc::from("clobbered"));
    let conflict = store.put(Arc::from("k2"), replacement).await;
    assert_eq!(conflict, Err(MemoryStoreError::IdConflict));

    let survivor = store
        .get(
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(survivor.preview.as_ref(), "body text");

    // The rejected write must not burn its idempotency key: the same key is
    // still usable for a record that does not collide.
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
        .await
        .unwrap();
}

#[tokio::test]
async fn put_replay_under_the_same_key_is_already_applied_not_a_conflict() {
    let store = InProcessMemoryStore::new();
    let record = crate::tests::sample_record("m1", "t1");
    store.put(Arc::from("k1"), record.clone()).await.unwrap();
    let replay = store.put(Arc::from("k1"), record).await.unwrap();
    assert_eq!(replay, PutOutcome::AlreadyApplied);
}

#[tokio::test]
async fn correct_rejects_self_supersession() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    let result = store
        .correct(
            Arc::from("k2"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            crate::tests::sample_record("m1", "t1"),
        )
        .await;
    assert_eq!(
        result,
        Err(MemoryStoreError::InvalidRecord {
            reason: "memory_self_supersession",
        })
    );

    // The record stays visible rather than superseding itself into oblivion.
    let hits = store
        .search(
            scope,
            MemoryQuery::ExactId(MemoryId::parse("m1").unwrap()),
            10,
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn search_excludes_tombstoned_and_superseded() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .forget(
            Arc::from("k2"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    let hits = store
        .search(
            scope,
            MemoryQuery::Keywords(Arc::from([Arc::<str>::from("alpha")])),
            10,
        )
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn correct_links_supersession_and_hides_old() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let replacement = crate::tests::sample_record("m2", "t1");
    store
        .correct(
            Arc::from("k2"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            replacement,
        )
        .await
        .unwrap();
    let old = store
        .get(scope.clone(), MemoryId::parse("m1").unwrap())
        .await
        .unwrap();
    assert!(old.is_none());
    let new = store
        .get(scope.clone(), MemoryId::parse("m2").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new.supersedes, Some(MemoryId::parse("m1").unwrap()));
    let hits = store
        .search(
            scope,
            MemoryQuery::ExactId(MemoryId::parse("m1").unwrap()),
            10,
        )
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn scope_filters_reads_and_search() {
    let store = InProcessMemoryStore::new();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let other = MemoryScope::try_new("t2").unwrap();
    assert!(
        store
            .get(other.clone(), MemoryId::parse("m1").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let hits = store
        .search(other, MemoryQuery::FullText(Arc::from("body")), 10)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn forget_missing_record_does_not_burn_the_idempotency_key() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    let key: Arc<str> = Arc::from("k1");

    let first = store
        .forget(
            Arc::clone(&key),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
        )
        .await;
    assert_eq!(first, Err(MemoryStoreError::NotFound));

    store
        .put(Arc::from("k-put"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    // Retrying the same idempotency key must actually tombstone the record
    // now that it exists, not silently no-op as "already applied".
    store
        .forget(key, scope.clone(), MemoryId::parse("m1").unwrap())
        .await
        .unwrap();
    let record = store
        .get(scope, MemoryId::parse("m1").unwrap())
        .await
        .unwrap();
    assert!(record.is_none());
}

#[tokio::test]
async fn correct_missing_old_record_does_not_burn_the_idempotency_key() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    let key: Arc<str> = Arc::from("k1");
    let replacement = crate::tests::sample_record("m2", "t1");

    let first = store
        .correct(
            Arc::clone(&key),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            replacement.clone(),
        )
        .await;
    assert_eq!(first, Err(MemoryStoreError::NotFound));

    store
        .put(Arc::from("k-put"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    // Retrying the same idempotency key must actually apply the correction
    // now that the old record exists, not silently no-op.
    store
        .correct(
            key,
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            replacement,
        )
        .await
        .unwrap();
    let old = store
        .get(scope.clone(), MemoryId::parse("m1").unwrap())
        .await
        .unwrap();
    assert!(old.is_none());
    let new = store
        .get(scope, MemoryId::parse("m2").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new.supersedes, Some(MemoryId::parse("m1").unwrap()));
}

#[tokio::test]
async fn full_text_matches_preview_substring() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let hits = store
        .search(scope, MemoryQuery::FullText(Arc::from("body text")), 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MatchEvidence::FullText);
}

#[tokio::test]
async fn forgotten_id_can_be_remembered_again_by_the_same_scope() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .forget(
            Arc::from("k2"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();

    // The tombstone must not strand the id: the same scope re-remembering
    // the same content is the recovery path, and refusing it would leave the
    // fact unstorable (supersession rejects a self-derived replacement id).
    let outcome = store
        .put(Arc::from("k3"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    assert_eq!(outcome, PutOutcome::Inserted);
    let revived = store
        .get(scope, MemoryId::parse("m1").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(!revived.tombstoned);
}

#[tokio::test]
async fn another_scope_can_use_the_same_id_as_a_tombstone() {
    let store = InProcessMemoryStore::new();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .forget(
            Arc::from("k2"),
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    let outcome = store
        .put(Arc::from("k3"), crate::tests::sample_record("m1", "t2"))
        .await
        .unwrap();
    assert_eq!(outcome, PutOutcome::Inserted);
}

#[tokio::test]
async fn correction_can_reuse_an_id_owned_by_another_scope() {
    let store = InProcessMemoryStore::new();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("victim", "t2"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("mine", "t1"))
        .await
        .unwrap();

    let mut replacement = crate::tests::sample_record("victim", "t1");
    replacement.body = MemoryBody::Inline(Arc::from("clobbered"));
    let result = store
        .correct(
            Arc::from("k3"),
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("mine").unwrap(),
            replacement,
        )
        .await;
    assert_eq!(result, Ok(()));

    // The other tenant's record is untouched.
    let victim = store
        .get(
            MemoryScope::try_new("t2").unwrap(),
            MemoryId::parse("victim").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(victim.scope.tenant(), "t2");
}

#[tokio::test]
async fn full_text_matches_any_query_token_and_never_matches_on_empty() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    // A whole user turn only overlaps the record on one token.
    let hits = store
        .search(
            scope.clone(),
            MemoryQuery::FullText(Arc::from("what did I say about body earlier?")),
            10,
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);

    for empty in ["", "   "] {
        let hits = store
            .search(scope.clone(), MemoryQuery::FullText(Arc::from(empty)), 10)
            .await
            .unwrap();
        assert!(hits.is_empty(), "empty query must not enumerate records");
    }
}

#[tokio::test]
async fn idempotency_key_is_bound_to_the_exact_operation_and_payload() {
    let store = InProcessMemoryStore::new();
    store
        .put(Arc::from("shared"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    let different_put = store
        .put(Arc::from("shared"), crate::tests::sample_record("m2", "t1"))
        .await;
    assert_eq!(different_put, Err(MemoryStoreError::IdempotencyConflict));

    let different_operation = store
        .forget(
            Arc::from("shared"),
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await;
    assert_eq!(
        different_operation,
        Err(MemoryStoreError::IdempotencyConflict)
    );
}

#[tokio::test]
async fn hard_expiry_hides_records_from_reads_and_releases_capacity_on_write() {
    let now = Arc::new(AtomicI64::new(0));
    let limits = MemoryStoreLimits {
        max_records: 1,
        max_inline_bytes: 9,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_store(Arc::clone(&now), limits);
    let mut expiring = crate::tests::sample_record("m1", "t1");
    expiring.body = MemoryBody::Inline(Arc::from("123456789"));
    expiring.preview = Arc::from("123456789");
    expiring.retention = RetentionPolicy::ExpireAfterMs(10);
    store.put(Arc::from("put-1"), expiring).await.unwrap();

    now.store(10, Ordering::SeqCst);
    let scope = MemoryScope::try_new("t1").unwrap();
    assert!(
        store
            .get(scope.clone(), MemoryId::parse("m1").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .search(scope.clone(), MemoryQuery::FullText(Arc::from("123")), 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .list(
                scope.clone(),
                MemoryPage {
                    offset: 0,
                    limit: 1,
                },
            )
            .await
            .unwrap()
            .total,
        0
    );
    // First write after expiry: the sweep has already dropped the row.
    assert_eq!(
        store
            .forget(
                Arc::from("forget-expired"),
                scope,
                MemoryId::parse("m1").unwrap(),
            )
            .await,
        Err(MemoryStoreError::NotFound)
    );

    let mut replacement = crate::tests::sample_record("m2", "t1");
    replacement.body = MemoryBody::Inline(Arc::from("123456789"));
    replacement.preview = Arc::from("123456789");
    assert_eq!(
        store.put(Arc::from("put-2"), replacement).await,
        Ok(PutOutcome::Inserted)
    );
}

#[tokio::test]
async fn reads_do_not_fail_when_expiry_sweep_would_exceed_artifact_action_capacity() {
    let now = Arc::new(AtomicI64::new(0));
    let limits = MemoryStoreLimits {
        max_artifact_actions: 1,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_store(Arc::clone(&now), limits);
    let mut record = crate::tests::blob_backed_record("m1", "t1").await;
    record.retention = RetentionPolicy::ExpireAfterMs(10);
    store.put(Arc::from("put"), record).await.unwrap();
    assert_eq!(store.pending_artifact_actions(1).await.unwrap().len(), 1);

    now.store(10, Ordering::SeqCst);
    let scope = MemoryScope::try_new("t1").unwrap();
    assert!(
        store
            .get(scope.clone(), MemoryId::parse("m1").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .search(
                scope.clone(),
                MemoryQuery::ExactId(MemoryId::parse("m1").unwrap()),
                1,
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .list(
                scope.clone(),
                MemoryPage {
                    offset: 0,
                    limit: 1,
                },
            )
            .await
            .unwrap()
            .total,
        0
    );
    // Write still sweeps, so the full outbox fails closed.
    assert_eq!(
        store
            .forget(
                Arc::from("forget-expired"),
                scope,
                MemoryId::parse("m1").unwrap(),
            )
            .await,
        Err(MemoryStoreError::CapacityExceeded {
            resource: "artifact_actions",
            limit: 1,
        })
    );
}

#[tokio::test]
async fn finite_store_limits_fail_before_mutating_state() {
    let limits = MemoryStoreLimits {
        max_records: 1,
        max_idempotency_keys: 1,
        max_inline_bytes: 64,
        max_search_results: 1,
        max_page_size: 1,
        max_artifact_actions: 1,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_store(Arc::new(AtomicI64::new(0)), limits);
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    assert_eq!(
        store
            .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
            .await,
        Err(MemoryStoreError::CapacityExceeded {
            resource: "idempotency_keys",
            limit: 1,
        })
    );
    let scope = MemoryScope::try_new("t1").unwrap();
    assert_eq!(
        store
            .search(scope.clone(), MemoryQuery::FullText(Arc::from("body")), 2)
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_search_limit_exceeded",
        })
    );
    assert_eq!(
        store
            .list(
                scope,
                MemoryPage {
                    offset: 0,
                    limit: 2,
                },
            )
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_page_limit_exceeded",
        })
    );
}

#[tokio::test]
async fn aged_receipts_are_pruned_so_capacity_is_not_a_lifetime_write_cap() {
    let now = Arc::new(AtomicI64::new(0));
    let limits = MemoryStoreLimits {
        max_idempotency_keys: 2,
        max_receipt_age_ms: 100,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_store(Arc::clone(&now), limits);
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
        .await
        .unwrap();
    assert_eq!(
        store
            .put(Arc::from("k3"), crate::tests::sample_record("m3", "t1"))
            .await,
        Err(MemoryStoreError::CapacityExceeded {
            resource: "idempotency_keys",
            limit: 2,
        })
    );
    now.store(100, Ordering::SeqCst);
    assert_eq!(
        store
            .put(Arc::from("k3"), crate::tests::sample_record("m3", "t1"))
            .await,
        Ok(PutOutcome::Inserted)
    );
    // Pruned key is a new operation; the live record surfaces as IdConflict.
    assert_eq!(
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await,
        Err(MemoryStoreError::IdConflict)
    );
}

#[tokio::test]
async fn correction_requires_exact_scope_and_unlinked_replacement() {
    let store = InProcessMemoryStore::new();
    let stored_scope = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_user("u1")
        .unwrap();
    let mut original = crate::tests::sample_record("m1", "t1");
    original.scope = stored_scope.clone();
    store.put(Arc::from("put"), original).await.unwrap();

    let mut replacement = crate::tests::sample_record("m2", "t1");
    replacement.scope = stored_scope.clone();
    let broad_scope = MemoryScope::try_new("t1").unwrap();
    assert_eq!(
        store
            .correct(
                Arc::from("broad"),
                broad_scope,
                MemoryId::parse("m1").unwrap(),
                replacement.clone(),
            )
            .await,
        Err(MemoryStoreError::NotFound)
    );

    replacement.superseded_by = Some(MemoryId::parse("m3").unwrap());
    assert_eq!(
        store
            .correct(
                Arc::from("linked"),
                stored_scope,
                MemoryId::parse("m1").unwrap(),
                replacement,
            )
            .await,
        Err(MemoryStoreError::InvalidRecord {
            reason: "memory_replacement_already_linked",
        })
    );
}

#[tokio::test]
async fn artifact_outbox_pins_then_unpins_memory_blobs() {
    let artifact_limits = ArtifactStoreLimits {
        orphan_grace_ms: 0,
        ..ArtifactStoreLimits::default()
    };
    let artifacts = InProcessArtifactStore::default().with_limits(artifact_limits);
    let artifact_scope = ArtifactScope {
        tenant_scope: Arc::from("t1"),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: Some(RunId::from_bytes([2; 16])),
        sensitivity: Sensitivity::Internal,
    };
    let artifact = stage_required_artifact(
        &artifacts,
        artifact_scope.clone(),
        Bytes::from_static(b"memory blob"),
        ArtifactMetadata {
            kind: Arc::from("memory-record"),
            media_type: Arc::from("text/plain"),
            name: Some(Arc::from("m1")),
            attributes: Metadata::empty(),
        },
    )
    .await
    .unwrap();
    let mut record = crate::tests::sample_record("m1", "t1");
    record.body = MemoryBody::Blob {
        scope: artifact_scope.clone(),
        artifact: artifact.clone(),
    };
    let memory = controlled_store(Arc::new(AtomicI64::new(10)), MemoryStoreLimits::default());
    memory.put(Arc::from("put"), record).await.unwrap();
    assert_eq!(memory.pending_artifact_actions(8).await.unwrap().len(), 1);
    assert_eq!(
        reconcile_memory_artifacts(&memory, &artifacts, 8).await,
        Ok(1)
    );
    assert!(memory.pending_artifact_actions(8).await.unwrap().is_empty());

    let now = Timestamp::from_unix_ms(10).unwrap();
    assert_eq!(
        artifacts
            .collect_orphans(artifact_scope.clone(), now, 8)
            .await
            .unwrap()
            .deleted,
        0
    );
    memory
        .forget(
            Arc::from("forget"),
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        reconcile_memory_artifacts(&memory, &artifacts, 8).await,
        Ok(1)
    );
    assert_eq!(
        artifacts
            .collect_orphans(artifact_scope.clone(), now, 8)
            .await
            .unwrap()
            .deleted,
        1
    );
    assert!(artifacts.get(artifact_scope, artifact).await.is_err());
}

fn embedding_query(embedder_id: &str, vector: EmbeddingVector) -> MemoryQuery {
    MemoryQuery::Embedding {
        embedder_id: Arc::from(embedder_id),
        vector,
    }
}

#[tokio::test]
async fn embedding_query_rejects_invalid_embedder_ids() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    let vector = EmbeddingVector::try_new(vec![1.0, 0.0]).unwrap();
    let overlong = "i".repeat(257);
    for bad_id in ["", overlong.as_str(), "id\0nul"] {
        assert_eq!(
            store
                .search(scope.clone(), embedding_query(bad_id, vector.clone()), 10)
                .await,
            Err(MemoryStoreError::InvalidRequest {
                reason: "memory_embedder_id_invalid",
            })
        );
    }
}

#[tokio::test]
async fn embedding_query_rejects_oversized_dimensions() {
    let narrow = InProcessMemoryStore::new().with_limits(MemoryStoreLimits {
        max_embedding_dimensions: 1,
        ..MemoryStoreLimits::default()
    });
    let scope = MemoryScope::try_new("t1").unwrap();
    let vector = EmbeddingVector::try_new(vec![1.0, 0.0]).unwrap();
    assert_eq!(
        narrow
            .search(scope, embedding_query("embed.hash-v1.2", vector), 10)
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedding_dimensions_exceeded",
        })
    );
}

#[tokio::test]
async fn embedding_query_for_an_unknown_space_returns_empty() {
    // A valid embedding query passes validation; a space no embedder ever
    // populated simply holds nothing.
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let vector = EmbeddingVector::try_new(vec![1.0, 0.0]).unwrap();
    assert_eq!(
        store
            .search(scope, embedding_query("embed.hash-v1.2", vector), 10)
            .await,
        Ok(Vec::new())
    );
}

#[test]
fn embedding_limits_default_to_documented_ceilings() {
    let limits = MemoryStoreLimits::default();
    assert_eq!(limits.max_embedding_dimensions, 4096);
    assert_eq!(limits.max_embedding_spaces, 4);
    assert_eq!(
        limits.max_embedding_dimensions,
        MAX_MEMORY_EMBEDDING_DIMENSIONS
    );
    assert_eq!(limits.max_embedding_spaces, MAX_MEMORY_EMBEDDING_SPACES);
}

#[test]
fn similarity_score_is_clamped_and_monotonic_with_fixed_endpoints() {
    assert_eq!(similarity_score(-1.0), 0);
    assert_eq!(similarity_score(0.0), 500_000);
    assert_eq!(similarity_score(1.0), 1_000_000);
    // Out-of-range dots clamp instead of wrapping.
    assert_eq!(similarity_score(-2.5), 0);
    assert_eq!(similarity_score(2.5), 1_000_000);
    assert_eq!(similarity_score(f32::NEG_INFINITY), 0);
    assert_eq!(similarity_score(f32::INFINITY), 1_000_000);
    // A degenerate NaN dot ranks last instead of poisoning the ordering.
    assert_eq!(similarity_score(f32::NAN), 0);

    let dots = [-1.0_f32, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0];
    for pair in dots.windows(2) {
        assert!(
            similarity_score(pair[0]) < similarity_score(pair[1]),
            "similarity_score must be strictly monotonic over {pair:?}"
        );
    }
}

const SPACE: &str = "embed.test-v1.2";

fn unit(components: Vec<f32>) -> EmbeddingVector {
    EmbeddingVector::try_new(components)
        .unwrap()
        .unit_normalized()
}

async fn source_digest_of(
    store: &InProcessMemoryStore,
    scope: &MemoryScope,
    id: &str,
) -> finstack_ai_kernel::Digest {
    let record = store
        .get(scope.clone(), MemoryId::parse(id).unwrap())
        .await
        .unwrap()
        .unwrap();
    embedding_source_digest(&embedding_source_text(&record)).unwrap()
}

async fn embed_record(
    store: &InProcessMemoryStore,
    scope: &MemoryScope,
    id: &str,
    space: &str,
    vector: EmbeddingVector,
) {
    let digest = source_digest_of(store, scope, id).await;
    store
        .store_embedding(
            Arc::from(space),
            scope.clone(),
            MemoryId::parse(id).unwrap(),
            digest,
            vector,
        )
        .await
        .unwrap();
}

fn pending_ids(sources: &[EmbeddingSource]) -> Vec<&str> {
    sources.iter().map(|source| source.id.as_str()).collect()
}

#[tokio::test]
async fn pending_embedding_sources_lists_only_live_unembedded_records_per_space() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
        .await
        .unwrap();

    let pending = store
        .pending_embedding_sources(Arc::from(SPACE), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m1", "m2"]);
    // The source carries the canonical text and its digest.
    assert_eq!(pending[0].text.as_ref(), "body text\nbody text\nalpha");
    assert_eq!(
        pending[0].source_digest,
        embedding_source_digest("body text\nbody text\nalpha").unwrap()
    );
    assert_eq!(pending[0].scope, scope);

    // Pending is derived per space: embedding m1 into one space leaves the
    // other space's anti-join untouched.
    embed_record(&store, &scope, "m1", SPACE, unit(vec![1.0, 0.0])).await;
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m2"]);
    let other_space = store
        .pending_embedding_sources(Arc::from("embed.other-v1.2"), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&other_space), ["m1", "m2"]);

    // Dead records are never pending; the limit bounds the batch.
    store
        .forget(
            Arc::from("k3"),
            scope.clone(),
            MemoryId::parse("m2").unwrap(),
        )
        .await
        .unwrap();
    assert!(
        store
            .pending_embedding_sources(Arc::from(SPACE), 8)
            .await
            .unwrap()
            .is_empty()
    );
    let limited = store
        .pending_embedding_sources(Arc::from("embed.other-v1.2"), 1)
        .await
        .unwrap();
    assert_eq!(pending_ids(&limited), ["m1"]);
}

#[tokio::test]
async fn store_embedding_digest_guard_silently_skips_rewritten_records() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let stale_digest = source_digest_of(&store, &scope, "m1").await;

    // The record is rewritten mid-reconcile: forgotten, then re-remembered
    // with different content under the same id.
    store
        .forget(
            Arc::from("k2"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    let mut rewritten = crate::tests::sample_record("m1", "t1");
    rewritten.body = MemoryBody::Inline(Arc::from("rewritten body"));
    rewritten.preview = Arc::from("rewritten body");
    store.put(Arc::from("k3"), rewritten).await.unwrap();

    // The stale write is a silent no-op, so the anti-join re-surfaces the
    // record instead of freezing the outdated vector.
    store
        .store_embedding(
            Arc::from(SPACE),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            stale_digest,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m1"]);

    // A tombstoned or missing record is skipped the same way.
    store
        .forget(
            Arc::from("k4"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    store
        .store_embedding(
            Arc::from(SPACE),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            stale_digest,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
    store
        .store_embedding(
            Arc::from(SPACE),
            scope.clone(),
            MemoryId::parse("missing").unwrap(),
            stale_digest,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
    let empty = store
        .search(scope, embedding_query(SPACE, unit(vec![1.0, 0.0])), 10)
        .await
        .unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn first_vector_fixes_a_space_dimensionality() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
        .await
        .unwrap();
    embed_record(&store, &scope, "m1", SPACE, unit(vec![1.0, 0.0])).await;

    let digest = source_digest_of(&store, &scope, "m2").await;
    assert_eq!(
        store
            .store_embedding(
                Arc::from(SPACE),
                scope.clone(),
                MemoryId::parse("m2").unwrap(),
                digest,
                unit(vec![1.0, 0.0, 0.0]),
            )
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedding_dimensions_mismatch",
        })
    );
    // Queries against the space enforce the same dimensionality.
    assert_eq!(
        store
            .search(scope, embedding_query(SPACE, unit(vec![1.0, 0.0, 0.0])), 10,)
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedding_dimensions_mismatch",
        })
    );
}

#[tokio::test]
async fn embedding_space_capacity_is_enforced() {
    let store = InProcessMemoryStore::new().with_limits(MemoryStoreLimits {
        max_embedding_spaces: 1,
        ..MemoryStoreLimits::default()
    });
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    embed_record(&store, &scope, "m1", SPACE, unit(vec![1.0, 0.0])).await;

    let digest = source_digest_of(&store, &scope, "m1").await;
    assert_eq!(
        store
            .store_embedding(
                Arc::from("embed.other-v1.2"),
                scope.clone(),
                MemoryId::parse("m1").unwrap(),
                digest,
                unit(vec![1.0, 0.0]),
            )
            .await,
        Err(MemoryStoreError::CapacityExceeded {
            resource: "embedding_spaces",
            limit: 1,
        })
    );

    // Dropping the space releases its slot: the index is derived data.
    store
        .forget_embedding_space(Arc::from(SPACE))
        .await
        .unwrap();
    store
        .store_embedding(
            Arc::from("embed.other-v1.2"),
            scope,
            MemoryId::parse("m1").unwrap(),
            digest,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn semantic_search_ranks_by_similarity_with_deterministic_tie_break() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    for id in ["m-close", "m-far", "m-mid", "m-tie"] {
        store
            .put(
                Arc::from(format!("k-{id}")),
                crate::tests::sample_record(id, "t1"),
            )
            .await
            .unwrap();
    }
    embed_record(&store, &scope, "m-close", SPACE, unit(vec![1.0, 0.0])).await;
    embed_record(&store, &scope, "m-tie", SPACE, unit(vec![1.0, 0.0])).await;
    embed_record(&store, &scope, "m-mid", SPACE, unit(vec![1.0, 1.0])).await;
    embed_record(&store, &scope, "m-far", SPACE, unit(vec![-1.0, 0.0])).await;

    let hits = store
        .search(
            scope.clone(),
            embedding_query(SPACE, unit(vec![1.0, 0.0])),
            10,
        )
        .await
        .unwrap();
    let ids: Vec<&str> = hits.iter().map(|hit| hit.record.id.as_str()).collect();
    // Equal-similarity hits tie-break on ascending id.
    assert_eq!(ids, ["m-close", "m-tie", "m-mid", "m-far"]);
    assert!(
        hits.iter()
            .all(|hit| hit.matched == MatchEvidence::Semantic)
    );
    assert_eq!(hits[0].score, 1_000_000);
    assert_eq!(hits[0].score, hits[1].score);
    assert!(hits[1].score > hits[2].score);
    assert!(hits[2].score > hits[3].score);
    assert_eq!(hits[3].score, 0);

    // The caller's limit truncates after ranking.
    let limited = store
        .search(scope, embedding_query(SPACE, unit(vec![1.0, 0.0])), 2)
        .await
        .unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].record.id.as_str(), "m-close");
}

#[tokio::test]
async fn semantic_search_honors_scope_isolation_and_liveness() {
    let now = Arc::new(AtomicI64::new(0));
    let store = controlled_store(Arc::clone(&now), MemoryStoreLimits::default());
    let scope_a = MemoryScope::try_new("t1").unwrap();
    let scope_b = MemoryScope::try_new("t2").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m1", "t2"))
        .await
        .unwrap();
    let mut expiring = crate::tests::sample_record("m-expiring", "t1");
    expiring.retention = RetentionPolicy::ExpireAfterMs(10);
    store.put(Arc::from("k3"), expiring).await.unwrap();

    embed_record(&store, &scope_a, "m1", SPACE, unit(vec![1.0, 0.0])).await;
    embed_record(&store, &scope_b, "m1", SPACE, unit(vec![1.0, 0.0])).await;
    embed_record(&store, &scope_a, "m-expiring", SPACE, unit(vec![1.0, 0.0])).await;

    // Only tenant 1's records surface for tenant 1's scope.
    let hits = store
        .search(
            scope_a.clone(),
            embedding_query(SPACE, unit(vec![1.0, 0.0])),
            10,
        )
        .await
        .unwrap();
    let ids: Vec<&str> = hits.iter().map(|hit| hit.record.id.as_str()).collect();
    assert_eq!(ids, ["m-expiring", "m1"]);
    assert!(hits.iter().all(|hit| hit.record.scope.tenant() == "t1"));

    // Reads filter expiry without waiting for a write sweep to evict.
    now.store(10, Ordering::SeqCst);
    let hits = store
        .search(scope_a, embedding_query(SPACE, unit(vec![1.0, 0.0])), 10)
        .await
        .unwrap();
    let ids: Vec<&str> = hits.iter().map(|hit| hit.record.id.as_str()).collect();
    assert_eq!(ids, ["m1"]);
}

#[tokio::test]
async fn embeddings_are_evicted_on_forget_correct_and_expiry() {
    let now = Arc::new(AtomicI64::new(0));
    let store = controlled_store(Arc::clone(&now), MemoryStoreLimits::default());
    let scope = MemoryScope::try_new("t1").unwrap();

    // Forget evicts: re-remembering identical content must surface the id
    // as pending again instead of reusing the tombstoned row.
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    embed_record(&store, &scope, "m1", SPACE, unit(vec![1.0, 0.0])).await;
    store
        .forget(
            Arc::from("k2"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    store
        .put(Arc::from("k3"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m1"]);

    // Correct evicts the superseded record's rows; the replacement is
    // pending, the old record is neither pending nor searchable.
    embed_record(&store, &scope, "m1", SPACE, unit(vec![1.0, 0.0])).await;
    store
        .correct(
            Arc::from("k4"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            crate::tests::sample_record("m2", "t1"),
        )
        .await
        .unwrap();
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m2"]);
    assert!(
        store
            .search(
                scope.clone(),
                embedding_query(SPACE, unit(vec![1.0, 0.0])),
                10,
            )
            .await
            .unwrap()
            .is_empty()
    );

    // Expiry sweeps evict rows alongside the record.
    let mut expiring = crate::tests::sample_record("m3", "t1");
    expiring.retention = RetentionPolicy::ExpireAfterMs(10);
    store.put(Arc::from("k5"), expiring).await.unwrap();
    embed_record(&store, &scope, "m3", SPACE, unit(vec![1.0, 0.0])).await;
    now.store(10, Ordering::SeqCst);
    // Any write sweeps; re-remembering the id then finds no stale row.
    store
        .put(Arc::from("k6"), crate::tests::sample_record("m3", "t1"))
        .await
        .unwrap();
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m2", "m3"]);
}

struct FlakyEmbedder {
    inner: HashEmbedder,
    fail_next: std::sync::atomic::AtomicBool,
}

impl TextEmbedder for FlakyEmbedder {
    fn descriptor(&self) -> TextEmbedderDescriptor {
        self.inner.descriptor()
    }

    fn embed(
        &self,
        texts: Vec<Arc<str>>,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<Vec<EmbeddingVector>, EmbedError>> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Box::pin(async {
                Err(EmbedError::Unavailable {
                    message: Arc::from("embedder offline"),
                })
            });
        }
        self.inner.embed(texts)
    }
}

#[tokio::test]
async fn reconcile_memory_embeddings_drains_idempotently_and_resumes() {
    let store = InProcessMemoryStore::new();
    let scope = MemoryScope::try_new("t1").unwrap();
    for id in ["m1", "m2", "m3"] {
        store
            .put(
                Arc::from(format!("k-{id}")),
                crate::tests::sample_record(id, "t1"),
            )
            .await
            .unwrap();
    }
    let embedder = HashEmbedder::try_new(16).unwrap();
    let space = embedder.descriptor().embedder_id;

    // A bounded batch drains incrementally and resumes across calls.
    assert_eq!(
        reconcile_memory_embeddings(&store, &embedder, 2).await,
        Ok(2)
    );
    assert_eq!(
        reconcile_memory_embeddings(&store, &embedder, 2).await,
        Ok(1)
    );
    // Re-running against a drained index applies nothing.
    assert_eq!(
        reconcile_memory_embeddings(&store, &embedder, 8).await,
        Ok(0)
    );
    assert!(
        store
            .pending_embedding_sources(Arc::clone(&space), 8)
            .await
            .unwrap()
            .is_empty()
    );

    // The drained index answers semantic queries end to end.
    let query = embedder
        .embed(vec![Arc::from("alpha body text")])
        .await
        .unwrap()
        .remove(0);
    let hits = store
        .search(
            scope,
            MemoryQuery::Embedding {
                embedder_id: Arc::clone(&space),
                vector: query,
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 3);
    assert!(
        hits.iter()
            .all(|hit| hit.matched == MatchEvidence::Semantic)
    );
    assert!(
        hits[0].score > 500_000,
        "shared tokens must score above 0-dot"
    );
}

#[tokio::test]
async fn reconcile_memory_embeddings_surfaces_embedder_failure_and_recovers() {
    let store = InProcessMemoryStore::new();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let embedder = FlakyEmbedder {
        inner: HashEmbedder::try_new(16).unwrap(),
        fail_next: std::sync::atomic::AtomicBool::new(true),
    };

    assert_eq!(
        reconcile_memory_embeddings(&store, &embedder, 8).await,
        Err(MemoryStoreError::Unavailable {
            message: Arc::from("memory_embedder_failed"),
        })
    );
    // Nothing was applied, nothing was lost: the next run drains fully.
    assert_eq!(
        reconcile_memory_embeddings(&store, &embedder, 8).await,
        Ok(1)
    );
    assert_eq!(
        reconcile_memory_embeddings(&store, &embedder, 8).await,
        Ok(0)
    );
}

/// A store that opts out of everything optional: the embedding defaults
/// must report no pending work, reject writes honestly, and accept space
/// rotation as a no-op.
struct MinimalStore;

impl MemoryStore for MinimalStore {
    fn put(
        &self,
        _idempotency_key: Arc<str>,
        _record: MemoryRecord,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<PutOutcome, MemoryStoreError>> {
        Box::pin(async { Err(minimal_unavailable()) })
    }

    fn get(
        &self,
        _scope: MemoryScope,
        _id: MemoryId,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>>
    {
        Box::pin(async { Ok(None) })
    }

    fn search(
        &self,
        _scope: MemoryScope,
        _query: MemoryQuery,
        _limit: usize,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn forget(
        &self,
        _idempotency_key: Arc<str>,
        _scope: MemoryScope,
        _id: MemoryId,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<(), MemoryStoreError>> {
        Box::pin(async { Err(minimal_unavailable()) })
    }

    fn correct(
        &self,
        _idempotency_key: Arc<str>,
        _scope: MemoryScope,
        _old: MemoryId,
        _replacement: MemoryRecord,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<(), MemoryStoreError>> {
        Box::pin(async { Err(minimal_unavailable()) })
    }

    fn list(
        &self,
        _scope: MemoryScope,
        _page: MemoryPage,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<MemoryListing, MemoryStoreError>> {
        Box::pin(async {
            Ok(MemoryListing {
                records: Vec::new(),
                total: 0,
            })
        })
    }
}

fn minimal_unavailable() -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from("minimal store is read-only"),
    }
}

#[tokio::test]
async fn embedding_trait_defaults_behave_for_a_minimal_store() {
    let store = MinimalStore;
    assert_eq!(
        store.pending_embedding_sources(Arc::from(SPACE), 8).await,
        Ok(Vec::new())
    );
    assert_eq!(
        store
            .store_embedding(
                Arc::from(SPACE),
                MemoryScope::try_new("t1").unwrap(),
                MemoryId::parse("m1").unwrap(),
                embedding_source_digest("anything").unwrap(),
                unit(vec![1.0, 0.0]),
            )
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embeddings_unsupported",
        })
    );
    assert_eq!(store.forget_embedding_space(Arc::from(SPACE)).await, Ok(()));

    // The reconciler sees no pending work and applies nothing.
    let embedder = HashEmbedder::try_new(16).unwrap();
    assert_eq!(
        reconcile_memory_embeddings(&store, &embedder, 8).await,
        Ok(0)
    );
}
