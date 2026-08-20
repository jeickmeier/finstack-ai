//! Runs the shared `ObjectStore` contract suite against [`FakeObjectStore`].

use std::sync::Arc;

use finstack_ai_kernel::{Metadata, Sensitivity};
use finstack_ai_runtime::{Bytes, ObjectError, ObjectKey, ObjectMetadata, ObjectScope, ObjectStore, PutPayload};
use finstack_ai_test::object_store::{FakeObjectStore, run_object_store_contract_suite};

#[tokio::test]
async fn fake_object_store_satisfies_the_contract() {
    run_object_store_contract_suite(Arc::new(FakeObjectStore::default()), true).await;
}

#[tokio::test]
async fn fake_object_store_reports_injected_integrity_failure() {
    let store = FakeObjectStore::default();
    let scope = ObjectScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: None,
        run_id: None,
        sensitivity: Sensitivity::Internal,
    };
    let key = ObjectKey::try_new("docs/injected.bin").expect("key must be valid");
    let metadata =
        ObjectMetadata { media_type: Arc::from("application/octet-stream"), name: None, attributes: Metadata::empty() };

    store
        .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(vec![7_u8; 1024])), metadata)
        .await
        .expect("put must succeed before the injected failure");

    store.fail_next_get_with_integrity();
    let error = store.get(scope, key).await.expect_err("get must fail once poisoned");
    assert!(
        matches!(error, ObjectError::Integrity { .. }),
        "expected ObjectError::Integrity, got {error:?}"
    );
}
