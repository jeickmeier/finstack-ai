//! Runs the shared `ObjectStore` contract suite against [`FakeObjectStore`].

use std::sync::Arc;

use finstack_ai_kernel::{Metadata, Sensitivity};
use finstack_ai_runtime::{
    Bytes, ObjectError, ObjectKey, ObjectMetadata, ObjectScope, ObjectStore, PageToken, PutPayload,
};
use finstack_ai_test::object_store::{FakeObjectStore, run_object_store_contract_suite};

fn test_scope(tenant: &str) -> ObjectScope {
    ObjectScope {
        tenant_scope: Arc::from(tenant),
        session_id: None,
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

fn test_metadata() -> ObjectMetadata {
    ObjectMetadata { media_type: Arc::from("application/octet-stream"), name: None, attributes: Metadata::empty() }
}

#[tokio::test]
async fn fake_object_store_satisfies_the_contract() {
    run_object_store_contract_suite(Arc::new(FakeObjectStore::default()), true).await;
}

#[tokio::test]
async fn fake_object_store_reports_injected_integrity_failure() {
    let store = FakeObjectStore::default();
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/injected.bin").expect("key must be valid");

    store
        .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(vec![7_u8; 1024])), test_metadata())
        .await
        .expect("put must succeed before the injected failure");

    store.fail_next_get_with_integrity();
    let error = store.get(scope, key).await.expect_err("get must fail once poisoned");
    assert!(
        matches!(error, ObjectError::Integrity { .. }),
        "expected ObjectError::Integrity, got {error:?}"
    );
}

/// Fake-only pagination coverage: the shared contract suite must not assert
/// on page size or page count (real backends may return all results in one
/// page), but `FakeObjectStore` genuinely pages at a fixed, small size, so
/// its multi-page cursor path deserves direct coverage here.
#[tokio::test]
async fn fake_object_store_pages_list_results_at_a_small_fixed_size() {
    const FAKE_LIST_PAGE_SIZE: usize = 2;

    let store = FakeObjectStore::default();
    let scope = test_scope("tenant-a");
    let mut expected_keys = Vec::new();
    for index in 0_u8..5 {
        let key = ObjectKey::try_new(format!("paged/{index}.bin")).expect("key must be valid");
        store
            .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(vec![7_u8; 1024])), test_metadata())
            .await
            .expect("put must succeed");
        expected_keys.push(key);
    }

    let prefix = Some(ObjectKey::try_new("paged").expect("prefix must be valid"));
    let mut collected = Vec::new();
    let mut page = PageToken::first();
    let mut page_count = 0_u32;
    loop {
        let result = store.list(scope.clone(), prefix.clone(), page).await.expect("list must succeed");
        page_count += 1;
        assert!(
            result.entries.len() <= FAKE_LIST_PAGE_SIZE,
            "the fake pages at a fixed size of {FAKE_LIST_PAGE_SIZE}"
        );
        collected.extend(result.entries.into_iter().map(|entry| entry.key));
        match result.next {
            Some(next) => page = next,
            None => break,
        }
    }

    assert!(
        page_count >= 3,
        "5 entries at {FAKE_LIST_PAGE_SIZE} per page must force at least 3 pages, got {page_count}"
    );
    collected.sort();
    let mut expected_sorted = expected_keys;
    expected_sorted.sort();
    assert_eq!(collected, expected_sorted, "the full walk must return exactly the 5 stored keys");
}
