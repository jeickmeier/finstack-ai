use crate::record::*;
use crate::store::*;
use std::sync::Arc;

#[tokio::test]
async fn put_is_idempotent_by_key() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    let record = crate::tests::sample_record("m1", "t1");
    let first = store.put(Arc::from("k1"), record.clone()).await.unwrap();
    let second = store.put(Arc::from("k1"), record).await.unwrap();
    assert_eq!(first, PutOutcome::Inserted);
    assert_eq!(second, PutOutcome::AlreadyApplied);
}

#[tokio::test]
async fn put_rejects_cross_tenant_id_clobber() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    let intruder = store
        .put(Arc::from("k2"), crate::tests::sample_record("m1", "t2"))
        .await;
    assert_eq!(intruder, Err(MemoryStoreError::IdConflict));

    let survivor = store
        .get(
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(survivor.scope.tenant(), "t1");
}

#[tokio::test]
async fn put_rejects_same_scope_overwrite_under_a_new_key() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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

    // The rolled-back transaction must have released the key claim, leaving
    // it usable for a write that does not collide.
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
        .await
        .unwrap();
}

#[tokio::test]
async fn put_replay_under_the_same_key_is_already_applied_not_a_conflict() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    let record = crate::tests::sample_record("m1", "t1");
    store.put(Arc::from("k1"), record.clone()).await.unwrap();
    let replay = store.put(Arc::from("k1"), record).await.unwrap();
    assert_eq!(replay, PutOutcome::AlreadyApplied);
}

#[tokio::test]
async fn correct_rejects_self_supersession() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
        .unwrap()
        .unwrap();
    assert_eq!(old.superseded_by, Some(MemoryId::parse("m2").unwrap()));
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
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
        .unwrap()
        .unwrap();
    assert!(record.tombstoned);
}

#[tokio::test]
async fn correct_missing_old_record_does_not_burn_the_idempotency_key() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
        .unwrap()
        .unwrap();
    assert_eq!(old.superseded_by, Some(MemoryId::parse("m2").unwrap()));
    let new = store
        .get(scope, MemoryId::parse("m2").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new.supersedes, Some(MemoryId::parse("m1").unwrap()));
}

#[tokio::test]
async fn full_text_matches_preview_substring() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
async fn full_text_ranks_with_bm25() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    let mut a = crate::tests::sample_record("m-a", "t1");
    a.body = MemoryBody::Inline(Arc::from("rust memory extension design"));
    a.preview = Arc::from("rust memory extension design");
    let mut b = crate::tests::sample_record("m-b", "t1");
    b.body = MemoryBody::Inline(Arc::from("memory"));
    b.preview = Arc::from("memory");
    store.put(Arc::from("k1"), a).await.unwrap();
    store.put(Arc::from("k2"), b).await.unwrap();
    let hits = store
        .search(
            scope,
            MemoryQuery::FullText(Arc::from("memory extension")),
            10,
        )
        .await
        .unwrap();
    assert_eq!(hits[0].record.id.as_str(), "m-a"); // both terms match => ranks first
}

#[tokio::test]
async fn persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mem.sqlite");
    {
        let store = SqliteMemoryStore::open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await
            .unwrap();
    }
    let store = SqliteMemoryStore::open(&path).unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    assert!(
        store
            .get(scope, MemoryId::parse("m1").unwrap())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn tombstone_removes_from_fts() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
        .search(scope, MemoryQuery::FullText(Arc::from("body")), 10)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn sqlite_forgotten_id_can_be_remembered_again_by_the_same_scope() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
    let outcome = store
        .put(Arc::from("k3"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    assert_eq!(outcome, PutOutcome::Inserted);

    // The revived record is live again, and searchable.
    let hits = store
        .search(
            scope,
            MemoryQuery::Keywords(Arc::from([Arc::<str>::from("alpha")])),
            10,
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn sqlite_put_rejects_reviving_another_scopes_tombstone() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
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
        .await;
    assert_eq!(outcome, Err(MemoryStoreError::IdConflict));
}

#[tokio::test]
async fn sqlite_correct_rejects_a_replacement_id_owned_by_another_scope() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("victim", "t2"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("mine", "t1"))
        .await
        .unwrap();
    let result = store
        .correct(
            Arc::from("k3"),
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("mine").unwrap(),
            crate::tests::sample_record("victim", "t1"),
        )
        .await;
    assert_eq!(result, Err(MemoryStoreError::IdConflict));

    let victim = store
        .get(
            MemoryScope::try_new("t2").unwrap(),
            MemoryId::parse("victim").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(victim.scope.tenant.as_ref(), "t2");
}

#[tokio::test]
async fn sqlite_full_text_matches_a_partial_query() {
    let store = SqliteMemoryStore::open_in_memory().unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    let mut record = crate::tests::sample_record("m1", "t1");
    record.body = MemoryBody::Inline(Arc::from("the user prefers dark mode"));
    record.preview = Arc::from("the user prefers dark mode");
    store.put(Arc::from("k1"), record).await.unwrap();

    // A whole user turn shares only some tokens with the stored memory:
    // under FTS5's implicit AND this would not match at all.
    let hits = store
        .search(
            scope,
            MemoryQuery::FullText(Arc::from("what did I say about dark mode?")),
            10,
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
}
