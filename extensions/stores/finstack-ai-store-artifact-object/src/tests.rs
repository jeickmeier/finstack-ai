use std::sync::Arc;

use finstack_ai_kernel::{Metadata, Sensitivity, SessionId};
use finstack_ai_runtime::{
    ARTIFACT_INTEGRITY_FAILURE, ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes,
    stage_required_artifact,
};

const ARTIFACT_NOT_FOUND: &str = "artifact_not_found";
const ARTIFACT_SCOPE_MISMATCH: &str = "artifact_scope_mismatch";
const ARTIFACT_TOO_LARGE: &str = "artifact_too_large";
use finstack_ai_test::object_store::FakeObjectStore;

use super::ObjectArtifactStore;

fn scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

fn other_scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("tenant-b"),
        session_id: SessionId::from_bytes([2; 16]),
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

fn metadata() -> ArtifactMetadata {
    ArtifactMetadata {
        kind: Arc::from("tool-output"),
        media_type: Arc::from("application/octet-stream"),
        name: Some(Arc::from("result.bin")),
        attributes: Metadata::parse(br#"{"source":"test"}"#).expect("metadata"),
    }
}

#[tokio::test]
async fn stage_and_get_round_trip_through_the_object_store() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectStore::default()));
    let content = Bytes::from(vec![9_u8; 5 * 1024 * 1024]); // > old 4 MiB cap
    let artifact = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .expect("staged past the old ceiling");
    let read = adapter.get(scope(), artifact).await.expect("get");
    assert_eq!(read, content);
}

#[tokio::test]
async fn default_limit_is_64_mib_and_enforced() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectStore::default()));
    assert_eq!(adapter.limits().max_artifact_bytes, 64 * 1024 * 1024);
    let oversize = Bytes::from(vec![0_u8; 64 * 1024 * 1024 + 1]);
    let error = stage_required_artifact(&adapter, scope(), oversize, metadata())
        .await
        .expect_err("must reject");
    assert_eq!(error.code(), ARTIFACT_TOO_LARGE);
}

#[tokio::test]
async fn cross_scope_get_fails_closed() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectStore::default()));
    let content = Bytes::from(vec![3_u8; 1024]);
    let artifact = stage_required_artifact(&adapter, scope(), content, metadata())
        .await
        .expect("staged");

    let error = adapter
        .get(other_scope(), artifact)
        .await
        .expect_err("cross-scope get must fail");
    assert!(
        error.code() == ARTIFACT_SCOPE_MISMATCH || error.code() == ARTIFACT_NOT_FOUND,
        "cross-scope get must fail closed with ScopeMismatch or NotFound, got {error:?}"
    );
}

#[tokio::test]
async fn object_errors_map_to_artifact_codes() {
    let object_store = Arc::new(FakeObjectStore::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let content = Bytes::from(vec![5_u8; 1024]);
    let artifact = stage_required_artifact(&adapter, scope(), content, metadata())
        .await
        .expect("staged");

    object_store.fail_next_get_with_integrity();
    let error = adapter.get(scope(), artifact).await.expect_err("must fail");
    assert_eq!(error.code(), ARTIFACT_INTEGRITY_FAILURE);
}
