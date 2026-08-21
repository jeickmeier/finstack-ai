//! Runs the shared `ObjectStore` contract suite against `LocalObjectStore`.

use std::sync::Arc;

use finstack_ai_kernel::{Metadata, Sensitivity};
use finstack_ai_runtime::{
    Bytes, OBJECT_TOO_LARGE, ObjectError, ObjectKey, ObjectMetadata, ObjectScope, ObjectStore,
    ObjectStoreLimits, PutPayload,
};
use finstack_ai_store_object_local::LocalObjectStore;
use finstack_ai_test::object_store::run_object_store_contract_suite;

#[tokio::test]
async fn local_store_satisfies_the_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
    run_object_store_contract_suite(Arc::new(store), false).await;
}

fn scope(tenant: &str) -> ObjectScope {
    ObjectScope {
        tenant_scope: Arc::from(tenant),
        session_id: None,
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

fn test_metadata() -> ObjectMetadata {
    ObjectMetadata {
        media_type: Arc::from("application/octet-stream"),
        name: None,
        attributes: Metadata::empty(),
    }
}

/// The shared contract suite skips its oversize case whenever a store's
/// ceiling exceeds the proxy-max threshold, and every other invocation of
/// this store in the workspace uses the 5 GiB default ceiling — so the
/// `Bytes`-payload `TooLarge` path gets zero coverage otherwise. Deliberately
/// outside the shared suite: a small ceiling is a per-backend test concern.
#[tokio::test]
async fn local_store_rejects_an_oversize_bytes_put_over_a_small_ceiling() {
    const MAX_OBJECT_BYTES: u64 = 1024;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalObjectStore::try_new(dir.path().to_path_buf())
        .expect("store")
        .with_limits(ObjectStoreLimits {
            max_object_bytes: MAX_OBJECT_BYTES,
        });
    let scope = scope("tenant-a");
    let key = ObjectKey::try_new("docs/oversize-bytes.bin").expect("key");
    let oversized = vec![7_u8; usize::try_from(MAX_OBJECT_BYTES + 1).expect("fits usize")];

    let error = store
        .put(
            scope.clone(),
            key.clone(),
            PutPayload::Bytes(Bytes::from(oversized)),
            test_metadata(),
        )
        .await
        .expect_err("put over the ceiling must be rejected");

    assert_eq!(error.code(), OBJECT_TOO_LARGE);
    assert!(matches!(error, ObjectError::TooLarge { .. }));

    let get_error = store
        .get(scope, key)
        .await
        .expect_err("no object must have been published");
    assert_eq!(get_error.code(), finstack_ai_runtime::OBJECT_NOT_FOUND);
    assert!(
        walk_root_files(dir.path()).is_empty(),
        "no envelope or temporary file must have been left behind: {:?}",
        walk_root_files(dir.path())
    );
}

/// Same as above but through the mid-stream `PutPayload::File` branch, whose
/// oversize check happens chunk-by-chunk while streaming rather than
/// up-front on `bytes.len()`. Uses a real 2 KiB source file against a 1 KiB
/// ceiling so the streaming check must trip mid-copy.
#[tokio::test]
async fn local_store_rejects_an_oversize_file_put_over_a_small_ceiling() {
    const MAX_OBJECT_BYTES: u64 = 1024;
    const SOURCE_BYTES: usize = 2048;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalObjectStore::try_new(dir.path().to_path_buf())
        .expect("store")
        .with_limits(ObjectStoreLimits {
            max_object_bytes: MAX_OBJECT_BYTES,
        });

    let mut source_file = tempfile::NamedTempFile::new().expect("tempfile");
    std::io::Write::write_all(&mut source_file, &vec![9_u8; SOURCE_BYTES]).expect("write source");
    let source_path = source_file.path().to_path_buf();

    let scope = scope("tenant-a");
    let key = ObjectKey::try_new("docs/oversize-file.bin").expect("key");

    let error = store
        .put(
            scope.clone(),
            key.clone(),
            PutPayload::File(source_path),
            test_metadata(),
        )
        .await
        .expect_err("put over the ceiling must be rejected");

    assert_eq!(error.code(), OBJECT_TOO_LARGE);
    assert!(matches!(error, ObjectError::TooLarge { .. }));

    let get_error = store
        .get(scope, key)
        .await
        .expect_err("no object must have been published");
    assert_eq!(get_error.code(), finstack_ai_runtime::OBJECT_NOT_FOUND);
    assert!(
        walk_root_files(dir.path()).is_empty(),
        "no envelope or temporary file must have been left behind: {:?}",
        walk_root_files(dir.path())
    );
}

/// List every regular file under `root`, recursively — used to assert a
/// rejected put left no envelope or temporary file behind.
fn walk_root_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
