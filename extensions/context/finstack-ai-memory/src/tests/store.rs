use crate::record::*;
use crate::store::*;
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
