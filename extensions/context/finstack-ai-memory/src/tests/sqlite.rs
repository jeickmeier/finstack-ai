use crate::record::*;
use crate::store::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

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

#[tokio::test]
async fn sqlite_v1_schema_migrates_to_v2() {
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
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
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
