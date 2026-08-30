use crate::record::*;
use crate::store::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use finstack_ai_embeddings::embedder::{HashEmbedder, TextEmbedder};
use finstack_ai_embeddings::vector::EmbeddingVector;
use finstack_ai_kernel::{Metadata, RunId, Sensitivity, SessionId, TIMESTAMP_MAX_MS, Timestamp};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{ArtifactMetadata, ArtifactScope, stage_required_artifact};

fn controlled_sqlite(now: Arc<AtomicI64>, limits: MemoryStoreLimits) -> SqliteMemoryStore {
    SqliteMemoryStore::try_open_in_memory_with(
        Arc::new(move || Timestamp::from_unix_ms(now.load(Ordering::SeqCst)).unwrap()),
        limits,
    )
    .unwrap()
}

#[tokio::test]
async fn put_is_idempotent_by_key() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
    let record = crate::tests::sample_record("m1", "t1");
    let first = store.put(Arc::from("k1"), record.clone()).await.unwrap();
    let second = store.put(Arc::from("k1"), record).await.unwrap();
    assert_eq!(first, PutOutcome::Inserted);
    assert_eq!(second, PutOutcome::AlreadyApplied);
}

#[tokio::test]
async fn sqlite_identical_ids_are_isolated_by_exact_scope() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();

    let other_scope = store
        .put(Arc::from("k2"), crate::tests::sample_record("m1", "t2"))
        .await
        .unwrap();
    assert_eq!(other_scope, PutOutcome::Inserted);

    let survivor = store
        .get(
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(survivor.scope.tenant(), "t1");
    assert!(
        store
            .get(
                MemoryScope::try_new("t2").unwrap(),
                MemoryId::parse("m1").unwrap(),
            )
            .await
            .unwrap()
            .is_some()
    );
}

#[test]
fn sqlite_store_identity_is_path_specific_and_persisted() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first.sqlite");
    let second_path = directory.path().join("second.sqlite");

    let first = SqliteMemoryStore::try_open(&first_path).unwrap();
    let first_id = first.descriptor().store_id;
    assert!(first.descriptor().manages_artifact_ownership);
    drop(first);

    let reopened_id = SqliteMemoryStore::try_open(&first_path)
        .unwrap()
        .descriptor()
        .store_id;
    let second_id = SqliteMemoryStore::try_open(&second_path)
        .unwrap()
        .descriptor()
        .store_id;
    assert_eq!(first_id, reopened_id);
    assert_ne!(first_id, second_id);
}

#[tokio::test]
async fn sqlite_and_in_process_full_text_normalization_match() {
    let sqlite = SqliteMemoryStore::try_open_in_memory().unwrap();
    let in_process = InProcessMemoryStore::new();
    let mut record = crate::tests::sample_record("m1", "t1");
    record.preview = Arc::from("Café-risk, portfolio");
    record.body = MemoryBody::Inline(Arc::from("Café-risk, portfolio"));
    sqlite
        .put(Arc::from("sqlite-put"), record.clone())
        .await
        .unwrap();
    in_process
        .put(Arc::from("in-process-put"), record)
        .await
        .unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    let query = MemoryQuery::FullText(Arc::from("CAFÉ-ris"));
    let sqlite_ids = sqlite
        .search(scope.clone(), query.clone(), 8)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.record.id)
        .collect::<Vec<_>>();
    let in_process_ids = in_process
        .search(scope, query, 8)
        .await
        .unwrap()
        .into_iter()
        .map(|hit| hit.record.id)
        .collect::<Vec<_>>();
    assert_eq!(sqlite_ids, in_process_ids);
}

#[tokio::test]
async fn put_rejects_same_scope_overwrite_under_a_new_key() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
    let record = crate::tests::sample_record("m1", "t1");
    store.put(Arc::from("k1"), record.clone()).await.unwrap();
    let replay = store.put(Arc::from("k1"), record).await.unwrap();
    assert_eq!(replay, PutOutcome::AlreadyApplied);
}

#[tokio::test]
async fn correct_rejects_self_supersession() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
        let store = SqliteMemoryStore::try_open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await
            .unwrap();
    }
    let store = SqliteMemoryStore::try_open(&path).unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
async fn sqlite_other_scope_can_use_the_same_id_as_a_tombstone() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
async fn sqlite_correction_can_reuse_an_id_owned_by_another_scope() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
    assert_eq!(result, Ok(()));

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
async fn sqlite_full_text_matches_a_partial_query() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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

#[tokio::test]
async fn sqlite_enforces_exact_idempotency_and_hard_expiry() {
    let now = Arc::new(AtomicI64::new(0));
    let store = controlled_sqlite(Arc::clone(&now), MemoryStoreLimits::default());
    let mut record = crate::tests::sample_record("m1", "t1");
    record.retention = RetentionPolicy::ExpireAfterMs(10);
    store
        .put(Arc::from("shared"), record.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .put(Arc::from("shared"), crate::tests::sample_record("m2", "t1"))
            .await,
        Err(MemoryStoreError::IdempotencyConflict)
    );

    now.store(10, Ordering::SeqCst);
    let scope = MemoryScope::try_new("t1").unwrap();
    assert!(
        store
            .get(scope.clone(), MemoryId::parse("m1").unwrap())
            .await
            .unwrap()
            .is_none()
    );
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
}

#[tokio::test]
async fn sqlite_enforces_finite_capacity_and_request_limits() {
    let limits = MemoryStoreLimits {
        max_records: 1,
        max_idempotency_keys: 1,
        max_inline_bytes: 64,
        max_search_results: 1,
        max_page_size: 1,
        max_artifact_actions: 1,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_sqlite(Arc::new(AtomicI64::new(0)), limits);
    assert!(store.descriptor().durable);
    assert_eq!(store.descriptor().limits, limits);
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
    assert_eq!(
        store
            .search(
                MemoryScope::try_new("t1").unwrap(),
                MemoryQuery::FullText(Arc::from("body")),
                2,
            )
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_search_limit_exceeded",
        })
    );
}

#[tokio::test]
async fn sqlite_reads_do_not_fail_when_expiry_sweep_would_exceed_artifact_action_capacity() {
    let now = Arc::new(AtomicI64::new(0));
    let limits = MemoryStoreLimits {
        max_artifact_actions: 1,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_sqlite(Arc::clone(&now), limits);
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
async fn sqlite_prunes_aged_receipts_so_capacity_is_not_a_lifetime_write_cap() {
    let now = Arc::new(AtomicI64::new(0));
    let limits = MemoryStoreLimits {
        max_idempotency_keys: 2,
        max_receipt_age_ms: 100,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_sqlite(Arc::clone(&now), limits);
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
    assert_eq!(
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await,
        Err(MemoryStoreError::IdConflict)
    );
}

#[tokio::test]
async fn sqlite_correction_requires_exact_scope_and_clean_links() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
    let exact = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_user("u1")
        .unwrap();
    let mut original = crate::tests::sample_record("m1", "t1");
    original.scope = exact.clone();
    store.put(Arc::from("put"), original).await.unwrap();

    let mut replacement = crate::tests::sample_record("m2", "t1");
    replacement.scope = exact.clone();
    assert_eq!(
        store
            .correct(
                Arc::from("broad"),
                MemoryScope::try_new("t1").unwrap(),
                MemoryId::parse("m1").unwrap(),
                replacement.clone(),
            )
            .await,
        Err(MemoryStoreError::NotFound)
    );

    replacement.supersedes = Some(MemoryId::parse("m0").unwrap());
    assert_eq!(
        store
            .correct(
                Arc::from("linked"),
                exact,
                MemoryId::parse("m1").unwrap(),
                replacement,
            )
            .await,
        Err(MemoryStoreError::InvalidRecord {
            reason: "memory_replacement_already_linked",
        })
    );
}

/// `PRAGMA user_version` of the database at `path`, via a raw connection.
fn schema_version(path: &std::path::Path) -> i32 {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

/// Row count of `memory_embeddings`; panics when the table is missing.
fn embeddings_row_count(path: &std::path::Path) -> i64 {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .query_row("SELECT COUNT(*) FROM memory_embeddings", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn quick_check(path: &std::path::Path) -> String {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap()
}

#[tokio::test]
async fn sqlite_v1_schema_migrates_to_v3() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-v1.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE memory_records (
               id TEXT PRIMARY KEY, tenant TEXT NOT NULL,
               user TEXT, agent TEXT, workspace TEXT,
               body_inline TEXT, blob_ref_json TEXT, preview TEXT NOT NULL,
               sensitivity TEXT NOT NULL, keywords_json TEXT NOT NULL,
               provenance_json TEXT NOT NULL, created_at INTEGER NOT NULL,
               last_confirmed_at INTEGER NOT NULL, supersedes TEXT,
               superseded_by TEXT, retention_json TEXT NOT NULL,
               tombstoned INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX memory_records_tenant ON memory_records(tenant);
             CREATE VIRTUAL TABLE memory_fts USING fts5(
               id UNINDEXED, preview, body, keywords
             );
             CREATE TABLE memory_idempotency (
               key TEXT PRIMARY KEY, applied_at INTEGER NOT NULL
             );
             PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);

    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    drop(store);
    assert_eq!(schema_version(&path), 3);
    assert_eq!(embeddings_row_count(&path), 0);
    assert_eq!(quick_check(&path), "ok");
}

/// Fabricate the exact v2 layout the previous release wrote, seeded with
/// `record` in the records table, FTS index, and keyword index.
fn fabricate_v2_database(path: &std::path::Path, record: &MemoryRecord) {
    let scope_digest = record.scope.digest().unwrap().to_hex();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE memory_records (
                   scope_digest TEXT NOT NULL, id TEXT NOT NULL,
                   tenant TEXT NOT NULL, user TEXT, agent TEXT, workspace TEXT,
                   body_inline TEXT, blob_ref_json TEXT, preview TEXT NOT NULL,
                   sensitivity TEXT NOT NULL, keywords_json TEXT NOT NULL,
                   provenance_json TEXT NOT NULL, created_at INTEGER NOT NULL,
                   last_confirmed_at INTEGER NOT NULL, supersedes TEXT,
                   superseded_by TEXT, retention_json TEXT NOT NULL,
                   expires_at INTEGER, tombstoned INTEGER NOT NULL DEFAULT 0
                     CHECK (tombstoned IN (0, 1)),
                   CHECK ((body_inline IS NULL) != (blob_ref_json IS NULL)),
                   PRIMARY KEY (scope_digest, id)
                 );
                 CREATE INDEX memory_records_scope ON memory_records(scope_digest, id);
                 CREATE INDEX memory_records_expiry
                   ON memory_records(expires_at) WHERE expires_at IS NOT NULL;
                 CREATE VIRTUAL TABLE memory_fts USING fts5(
                   scope_digest UNINDEXED, id UNINDEXED, preview, body, keywords
                 );
                 CREATE TABLE memory_keywords (
                   scope_digest TEXT NOT NULL, id TEXT NOT NULL,
                   ordinal INTEGER NOT NULL, keyword TEXT NOT NULL,
                   keyword_folded TEXT NOT NULL,
                   PRIMARY KEY (scope_digest, id, ordinal)
                 );
                 CREATE INDEX memory_keywords_lookup
                   ON memory_keywords(scope_digest, keyword_folded, id);
                 CREATE TABLE memory_idempotency (
                   scope_digest TEXT NOT NULL, key TEXT NOT NULL,
                   fingerprint TEXT NOT NULL, applied_at INTEGER NOT NULL,
                   PRIMARY KEY (scope_digest, key)
                 );
                 CREATE TABLE memory_artifact_outbox (
                   sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                   action_id TEXT NOT NULL UNIQUE,
                   action_json TEXT NOT NULL,
                   created_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 2;",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO memory_records
                 (scope_digest, id, tenant, user, agent, workspace, body_inline, blob_ref_json,
                  preview, sensitivity, keywords_json, provenance_json, created_at,
                  last_confirmed_at, supersedes, superseded_by, retention_json, expires_at,
                  tombstoned)
                 VALUES (?1, ?2, ?3, NULL, NULL, NULL, ?4, NULL, ?5, ?6, ?7, ?8, 0, 0,
                         NULL, NULL, ?9, NULL, 0)",
            rusqlite::params![
                scope_digest,
                record.id.as_str(),
                record.scope.tenant(),
                "body text",
                record.preview.as_ref(),
                serde_json::to_string(&record.sensitivity).unwrap(),
                serde_json::to_string(&record.keywords).unwrap(),
                serde_json::to_string(&record.provenance).unwrap(),
                serde_json::to_string(&record.retention).unwrap(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO memory_fts (scope_digest, id, preview, body, keywords)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                scope_digest,
                record.id.as_str(),
                record.preview.as_ref(),
                "body text",
                "alpha"
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO memory_keywords
                 (scope_digest, id, ordinal, keyword, keyword_folded)
                 VALUES (?1, ?2, 0, 'alpha', 'alpha')",
            rusqlite::params![scope_digest, record.id.as_str()],
        )
        .unwrap();
}

#[tokio::test]
async fn sqlite_v2_schema_migrates_to_v3_preserving_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-v2.sqlite");
    fabricate_v2_database(&path, &crate::tests::sample_record("m1", "t1"));

    let store = SqliteMemoryStore::try_open(&path).unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    let survivor = store
        .get(scope.clone(), MemoryId::parse("m1").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(survivor.preview.as_ref(), "body text");
    let full_text = store
        .search(scope.clone(), MemoryQuery::FullText(Arc::from("body")), 8)
        .await
        .unwrap();
    assert_eq!(full_text.len(), 1);
    let keywords = store
        .search(
            scope,
            MemoryQuery::Keywords(Arc::from([Arc::<str>::from("alpha")])),
            8,
        )
        .await
        .unwrap();
    assert_eq!(keywords.len(), 1);
    drop(store);

    assert_eq!(schema_version(&path), 3);
    assert_eq!(embeddings_row_count(&path), 0);
    assert_eq!(quick_check(&path), "ok");
}

#[test]
fn sqlite_fresh_open_creates_v3_directly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-fresh.sqlite");
    drop(SqliteMemoryStore::try_open(&path).unwrap());
    assert_eq!(schema_version(&path), 3);
    assert_eq!(embeddings_row_count(&path), 0);
    assert_eq!(quick_check(&path), "ok");
}

#[test]
fn sqlite_future_schema_version_is_unsupported() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-future.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 4;")
        .unwrap();
    drop(connection);
    let error = SqliteMemoryStore::try_open(&path).err().unwrap();
    assert_eq!(
        error,
        MemoryStoreError::Unavailable {
            message: Arc::from("memory_store_sqlite_schema_unsupported"),
        }
    );
}

const SPACE_A: &str = "embed.a-v1.2";
const SPACE_B: &str = "embed.b-v1.2";

/// Insert an embedding row directly, bypassing the store: eviction must hold
/// for arbitrary rows, not only rows `store_embedding` accepted.
fn seed_embedding_row(path: &std::path::Path, tenant: &str, id: &str, embedder_id: &str) {
    let scope_digest = MemoryScope::try_new(tenant)
        .unwrap()
        .digest()
        .unwrap()
        .to_hex();
    let vector: Vec<u8> = [1.0_f32, 0.0_f32]
        .iter()
        .flat_map(|component| component.to_le_bytes())
        .collect();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO memory_embeddings
             (scope_digest, id, embedder_id, dimensions, source_digest, vector, embedded_at)
             VALUES (?1, ?2, ?3, 2, 'seeded', ?4, 0)",
            rusqlite::params![scope_digest, id, embedder_id, vector],
        )
        .unwrap();
}

/// `(id, embedder_id)` pairs currently indexed, in stable order.
fn embedding_row_keys(path: &std::path::Path) -> Vec<(String, String)> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT id, embedder_id FROM memory_embeddings ORDER BY id, embedder_id")
        .unwrap();
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    rows.map(Result::unwrap).collect()
}

fn pair(id: &str, embedder_id: &str) -> (String, String) {
    (String::from(id), String::from(embedder_id))
}

#[tokio::test]
async fn sqlite_put_over_tombstone_evicts_stale_embedding_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-revive.sqlite");
    {
        let store = SqliteMemoryStore::try_open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await
            .unwrap();
        store
            .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
            .await
            .unwrap();
        store
            .forget(
                Arc::from("k3"),
                MemoryScope::try_new("t1").unwrap(),
                MemoryId::parse("m1").unwrap(),
            )
            .await
            .unwrap();
    }
    seed_embedding_row(&path, "t1", "m1", SPACE_A);
    seed_embedding_row(&path, "t1", "m1", SPACE_B);
    seed_embedding_row(&path, "t1", "m2", SPACE_A);

    // Re-remembering the forgotten id must drop the tombstone's stale rows
    // in every space; the unrelated record's rows survive.
    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store
        .put(Arc::from("k4"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    drop(store);
    assert_eq!(embedding_row_keys(&path), [pair("m2", SPACE_A)]);
}

#[tokio::test]
async fn sqlite_forget_evicts_embedding_rows_across_all_spaces() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-forget.sqlite");
    {
        let store = SqliteMemoryStore::try_open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await
            .unwrap();
        store
            .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
            .await
            .unwrap();
    }
    seed_embedding_row(&path, "t1", "m1", SPACE_A);
    seed_embedding_row(&path, "t1", "m1", SPACE_B);
    seed_embedding_row(&path, "t1", "m2", SPACE_A);

    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store
        .forget(
            Arc::from("k3"),
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    drop(store);
    assert_eq!(embedding_row_keys(&path), [pair("m2", SPACE_A)]);
}

#[tokio::test]
async fn sqlite_correct_evicts_superseded_and_replacement_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-correct.sqlite");
    {
        let store = SqliteMemoryStore::try_open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await
            .unwrap();
        store
            .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
            .await
            .unwrap();
    }
    seed_embedding_row(&path, "t1", "m1", SPACE_A);
    seed_embedding_row(&path, "t1", "m1", SPACE_B);
    seed_embedding_row(&path, "t1", "m2", SPACE_A);
    // A stale row under the replacement's id must go with the supersession.
    seed_embedding_row(&path, "t1", "m3", SPACE_A);

    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store
        .correct(
            Arc::from("k3"),
            MemoryScope::try_new("t1").unwrap(),
            MemoryId::parse("m1").unwrap(),
            crate::tests::sample_record("m3", "t1"),
        )
        .await
        .unwrap();
    drop(store);
    assert_eq!(embedding_row_keys(&path), [pair("m2", SPACE_A)]);
}

#[tokio::test]
async fn sqlite_expiry_sweep_evicts_embedding_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-expiry.sqlite");
    {
        let store = SqliteMemoryStore::try_open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m2", "t1"))
            .await
            .unwrap();
        // `sample_record` is created at the epoch, so a 10ms retention is
        // hard-expired under the system clock; put last so no earlier
        // write's sweep removes it before rows are seeded.
        let mut expiring = crate::tests::sample_record("m-old", "t1");
        expiring.retention = RetentionPolicy::ExpireAfterMs(10);
        store.put(Arc::from("k2"), expiring).await.unwrap();
    }
    seed_embedding_row(&path, "t1", "m-old", SPACE_A);
    seed_embedding_row(&path, "t1", "m-old", SPACE_B);
    seed_embedding_row(&path, "t1", "m2", SPACE_A);

    // Any write sweeps hard-expired records; their rows go too.
    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store
        .put(Arc::from("k3"), crate::tests::sample_record("m3", "t1"))
        .await
        .unwrap();
    drop(store);
    assert_eq!(embedding_row_keys(&path), [pair("m2", SPACE_A)]);
}

#[tokio::test]
async fn sqlite_decode_fails_closed_on_corrupt_timestamps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-corrupt.sqlite");
    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    drop(store);

    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE memory_records SET created_at = ?1 WHERE id = 'm1'",
            [TIMESTAMP_MAX_MS + 1],
        )
        .unwrap();
    drop(connection);

    let store = SqliteMemoryStore::try_open(&path).unwrap();
    assert_eq!(
        store
            .get(
                MemoryScope::try_new("t1").unwrap(),
                MemoryId::parse("m1").unwrap(),
            )
            .await,
        Err(MemoryStoreError::Unavailable {
            message: Arc::from("memory_store_sqlite_unavailable"),
        })
    );
}

#[tokio::test]
async fn sqlite_artifact_outbox_survives_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-outbox.sqlite");
    let artifacts = InProcessArtifactStore::default();
    let artifact_scope = ArtifactScope {
        tenant_scope: Arc::from("t1"),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: Some(RunId::from_bytes([2; 16])),
        sensitivity: Sensitivity::Internal,
    };
    let artifact = stage_required_artifact(
        &artifacts,
        artifact_scope.clone(),
        Bytes::from_static(b"durable memory blob"),
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
        scope: artifact_scope,
        artifact,
    };
    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store.put(Arc::from("put"), record).await.unwrap();
    let before = store.pending_artifact_actions(8).await.unwrap();
    assert_eq!(before.len(), 1);
    drop(store);

    let reopened = SqliteMemoryStore::try_open(&path).unwrap();
    assert_eq!(reopened.pending_artifact_actions(8).await.unwrap(), before);
}

fn unit(components: Vec<f32>) -> EmbeddingVector {
    EmbeddingVector::try_new(components)
        .unwrap()
        .unit_normalized()
}

async fn source_digest_of(
    store: &dyn MemoryStore,
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
    store: &dyn MemoryStore,
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
async fn sqlite_pending_embedding_sources_anti_join_per_space() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
        .pending_embedding_sources(Arc::from(SPACE_A), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m1", "m2"]);
    assert_eq!(pending[0].text.as_ref(), "body text\nbody text\nalpha");
    assert_eq!(
        pending[0].source_digest,
        embedding_source_digest("body text\nbody text\nalpha").unwrap()
    );
    assert_eq!(pending[0].scope, scope);

    // Pending is derived per space: embedding m1 into one space leaves the
    // other space's anti-join untouched.
    embed_record(&store, &scope, "m1", SPACE_A, unit(vec![1.0, 0.0])).await;
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE_A), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m2"]);
    let other_space = store
        .pending_embedding_sources(Arc::from(SPACE_B), 8)
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
            .pending_embedding_sources(Arc::from(SPACE_A), 8)
            .await
            .unwrap()
            .is_empty()
    );
    let limited = store
        .pending_embedding_sources(Arc::from(SPACE_B), 1)
        .await
        .unwrap();
    assert_eq!(pending_ids(&limited), ["m1"]);

    // A rewritten record re-appears: the forget evicted m1's row, and the
    // revived id is pending again in every space.
    store
        .forget(
            Arc::from("k4"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
        )
        .await
        .unwrap();
    store
        .put(Arc::from("k5"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE_A), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m1"]);

    let invalid = store.pending_embedding_sources(Arc::from(""), 8).await;
    assert_eq!(
        invalid,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedder_id_invalid",
        })
    );
}

#[tokio::test]
async fn sqlite_pending_includes_records_with_stale_index_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-stale.sqlite");
    {
        let store = SqliteMemoryStore::try_open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await
            .unwrap();
    }
    // A row whose stored digest no longer matches the record's content must
    // re-surface the record even though the space holds a row for it.
    seed_embedding_row(&path, "t1", "m1", SPACE_A);

    let store = SqliteMemoryStore::try_open(&path).unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE_A), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m1"]);

    // Re-embedding under the current digest replaces the stale row.
    embed_record(&store, &scope, "m1", SPACE_A, unit(vec![1.0, 0.0])).await;
    assert!(
        store
            .pending_embedding_sources(Arc::from(SPACE_A), 8)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn sqlite_pending_batch_is_capped_by_max_search_results() {
    let limits = MemoryStoreLimits {
        max_search_results: 1,
        ..MemoryStoreLimits::default()
    };
    let store = controlled_sqlite(Arc::new(AtomicI64::new(0)), limits);
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
        .await
        .unwrap();
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE_A), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m1"]);
}

#[tokio::test]
async fn sqlite_store_embedding_digest_guard_skips_rewritten_and_dead_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-guard.sqlite");
    let store = SqliteMemoryStore::try_open(&path).unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    let stale = source_digest_of(&store, &scope, "m1").await;

    // The record is corrected mid-reconcile: the superseded id, a missing
    // id, and the live replacement under a mismatched digest are all silent
    // no-ops.
    store
        .correct(
            Arc::from("k2"),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            crate::tests::sample_record("m2", "t1"),
        )
        .await
        .unwrap();
    store
        .store_embedding(
            Arc::from(SPACE_A),
            scope.clone(),
            MemoryId::parse("m1").unwrap(),
            stale,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
    store
        .store_embedding(
            Arc::from(SPACE_A),
            scope.clone(),
            MemoryId::parse("missing").unwrap(),
            stale,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
    let mismatched = embedding_source_digest("something else entirely").unwrap();
    store
        .store_embedding(
            Arc::from(SPACE_A),
            scope.clone(),
            MemoryId::parse("m2").unwrap(),
            mismatched,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
    // The replacement stays pending until a current-digest write lands.
    let pending = store
        .pending_embedding_sources(Arc::from(SPACE_A), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending), ["m2"]);
    embed_record(&store, &scope, "m2", SPACE_A, unit(vec![1.0, 0.0])).await;
    drop(store);

    assert_eq!(embedding_row_keys(&path), [pair("m2", SPACE_A)]);
}

#[tokio::test]
async fn sqlite_first_vector_fixes_space_dimensionality() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    store
        .put(Arc::from("k2"), crate::tests::sample_record("m2", "t1"))
        .await
        .unwrap();
    embed_record(&store, &scope, "m1", SPACE_A, unit(vec![1.0, 0.0])).await;

    let digest = source_digest_of(&store, &scope, "m2").await;
    assert_eq!(
        store
            .store_embedding(
                Arc::from(SPACE_A),
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

    let narrow = controlled_sqlite(
        Arc::new(AtomicI64::new(0)),
        MemoryStoreLimits {
            max_embedding_dimensions: 1,
            ..MemoryStoreLimits::default()
        },
    );
    assert_eq!(
        narrow
            .store_embedding(
                Arc::from(SPACE_A),
                scope,
                MemoryId::parse("m1").unwrap(),
                digest,
                unit(vec![1.0, 0.0]),
            )
            .await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedding_dimensions_exceeded",
        })
    );
}

#[tokio::test]
async fn sqlite_embedding_space_capacity_is_enforced() {
    let store = controlled_sqlite(
        Arc::new(AtomicI64::new(0)),
        MemoryStoreLimits {
            max_embedding_spaces: 1,
            ..MemoryStoreLimits::default()
        },
    );
    let scope = MemoryScope::try_new("t1").unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    embed_record(&store, &scope, "m1", SPACE_A, unit(vec![1.0, 0.0])).await;

    let digest = source_digest_of(&store, &scope, "m1").await;
    assert_eq!(
        store
            .store_embedding(
                Arc::from(SPACE_B),
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
        .forget_embedding_space(Arc::from(SPACE_A))
        .await
        .unwrap();
    store
        .store_embedding(
            Arc::from(SPACE_B),
            scope,
            MemoryId::parse("m1").unwrap(),
            digest,
            unit(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn sqlite_embedding_rows_persist_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-durable.sqlite");
    let scope = MemoryScope::try_new("t1").unwrap();
    {
        let store = SqliteMemoryStore::try_open(&path).unwrap();
        store
            .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
            .await
            .unwrap();
        embed_record(&store, &scope, "m1", SPACE_A, unit(vec![1.0, 0.0])).await;
    }

    let reopened = SqliteMemoryStore::try_open(&path).unwrap();
    assert!(
        reopened
            .pending_embedding_sources(Arc::from(SPACE_A), 8)
            .await
            .unwrap()
            .is_empty()
    );
    drop(reopened);
    assert_eq!(embedding_row_keys(&path), [pair("m1", SPACE_A)]);
}

#[tokio::test]
async fn sqlite_forget_embedding_space_empties_only_that_space() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory-rotate.sqlite");
    let scope = MemoryScope::try_new("t1").unwrap();
    let store = SqliteMemoryStore::try_open(&path).unwrap();
    store
        .put(Arc::from("k1"), crate::tests::sample_record("m1", "t1"))
        .await
        .unwrap();
    embed_record(&store, &scope, "m1", SPACE_A, unit(vec![1.0, 0.0])).await;
    embed_record(&store, &scope, "m1", SPACE_B, unit(vec![0.0, 1.0])).await;

    store
        .forget_embedding_space(Arc::from(SPACE_A))
        .await
        .unwrap();
    let pending_a = store
        .pending_embedding_sources(Arc::from(SPACE_A), 8)
        .await
        .unwrap();
    assert_eq!(pending_ids(&pending_a), ["m1"]);
    assert!(
        store
            .pending_embedding_sources(Arc::from(SPACE_B), 8)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.forget_embedding_space(Arc::from("")).await,
        Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedder_id_invalid",
        })
    );
    drop(store);
    assert_eq!(embedding_row_keys(&path), [pair("m1", SPACE_B)]);
}

#[tokio::test]
async fn sqlite_reconcile_memory_embeddings_drains_and_is_idempotent() {
    let store = SqliteMemoryStore::try_open_in_memory().unwrap();
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
}
