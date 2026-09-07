use crate::{record::*, store::*};
use finstack_ai_embeddings::vector::EmbeddingVector;
use finstack_ai_kernel::Timestamp;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

async fn embed(
    store: &dyn MemoryStore,
    record: &MemoryRecord,
    space: &str,
) -> Result<(), MemoryStoreError> {
    store
        .store_embedding(
            space.into(),
            record.scope.clone(),
            record.id.clone(),
            embedding_source_digest(&embedding_source_text(record)).unwrap(),
            EmbeddingVector::try_new(vec![1.0, 0.0]).unwrap(),
        )
        .await
}

async fn byte_capacity_contract(store: &dyn MemoryStore, now: &AtomicI64) {
    let first = super::sample_record("first", "tenant");
    let other = super::sample_record("other", "other-tenant");
    for record in [&first, &other] {
        store
            .put(record.id.as_str().into(), record.clone())
            .await
            .unwrap();
    }
    embed(store, &first, "space").await.unwrap();
    embed(store, &first, "space").await.unwrap(); // replacement credits existing bytes
    for (record, space) in [(&first, "second-space"), (&other, "space")] {
        assert_eq!(
            embed(store, record, space).await,
            Err(MemoryStoreError::CapacityExceeded {
                resource: "embedding_bytes",
                limit: 8
            })
        );
    }
    assert_eq!(
        store
            .embedding_coverage(first.scope.clone(), "space".into())
            .await
            .unwrap()
            .unwrap()
            .indexed_records,
        1
    );
    store
        .forget("forget".into(), first.scope.clone(), first.id.clone())
        .await
        .unwrap();
    embed(store, &other, "space").await.unwrap();
    let replacement = super::sample_record("replacement", "other-tenant");
    store
        .correct(
            "correct".into(),
            other.scope.clone(),
            other.id.clone(),
            replacement.clone(),
        )
        .await
        .unwrap();
    embed(store, &replacement, "space").await.unwrap();
    store.forget_embedding_space("space".into()).await.unwrap();
    let mut expiring = super::sample_record("expiring", "tenant");
    expiring.retention = RetentionPolicy::ExpireAfterMs(10);
    store.put("expiry".into(), expiring.clone()).await.unwrap();
    embed(store, &expiring, "space").await.unwrap();
    now.store(10, Ordering::SeqCst);
    store
        .put("sweep".into(), super::sample_record("sweep", "tenant"))
        .await
        .unwrap();
    embed(store, &replacement, "space").await.unwrap();
    let page = store
        .list(
            replacement.scope.clone(),
            MemoryPage {
                offset: 0,
                limit: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    assert!(page.records.is_empty());
}

#[tokio::test]
async fn in_process_embedding_bytes_are_atomic_and_reclaimed() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let store = InProcessMemoryStore::new()
        .with_limits(MemoryStoreLimits {
            max_embedding_bytes: 8,
            ..Default::default()
        })
        .with_clock(Arc::new(move || {
            Timestamp::from_unix_ms(clock.load(Ordering::SeqCst)).unwrap()
        }));
    byte_capacity_contract(&store, &now).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_embedding_bytes_are_atomic_and_reclaimed() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let store = SqliteMemoryStore::try_open_in_memory_with(
        Arc::new(move || Timestamp::from_unix_ms(clock.load(Ordering::SeqCst)).unwrap()),
        MemoryStoreLimits {
            max_embedding_bytes: 8,
            ..Default::default()
        },
    )
    .unwrap();
    byte_capacity_contract(&store, &now).await;
}
