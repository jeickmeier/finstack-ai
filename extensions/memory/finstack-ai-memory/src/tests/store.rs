use crate::record::*;
use crate::store::*;
use std::sync::Arc;

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
        .unwrap()
        .unwrap();
    assert!(record.tombstoned);
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
