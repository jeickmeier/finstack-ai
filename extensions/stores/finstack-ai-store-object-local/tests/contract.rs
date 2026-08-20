//! Runs the shared `ObjectStore` contract suite against `LocalObjectStore`.

use std::sync::Arc;

use finstack_ai_store_object_local::LocalObjectStore;
use finstack_ai_test::object_store::run_object_store_contract_suite;

#[tokio::test]
async fn local_store_satisfies_the_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
    run_object_store_contract_suite(Arc::new(store), false).await;
}
