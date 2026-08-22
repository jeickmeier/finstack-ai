use std::sync::Arc;

use crate::driver::{ObjectDriver, ObjectKey, PageToken};
use finstack_ai_kernel::{Metadata, Sensitivity, SessionId, Timestamp};
use finstack_ai_runtime::{
    ARTIFACT_INTEGRITY_FAILURE, ArtifactMetadata, ArtifactOwnerId, ArtifactScope, ArtifactStore,
    Bytes, stage_required_artifact,
};

const ARTIFACT_NOT_FOUND: &str = "artifact_not_found";
const ARTIFACT_SCOPE_MISMATCH: &str = "artifact_scope_mismatch";
const ARTIFACT_TOO_LARGE: &str = "artifact_too_large";
use crate::driver_fake::FakeObjectDriver;

use crate::artifact::ObjectArtifactStore;

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
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectDriver::default()));
    let content = Bytes::from(vec![9_u8; 5 * 1024 * 1024]); // > old 4 MiB cap
    let artifact = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .expect("staged past the old ceiling");
    let read = adapter.get(scope(), artifact).await.expect("get");
    assert_eq!(read, content);
}

#[tokio::test]
async fn digest_bearing_blob_resolves_within_scope() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectDriver::default()));
    let content = Bytes::from_static(b"scoped attachment");
    let artifact = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .expect("staged");

    let read = adapter
        .get_by_blob(scope(), artifact.blob().clone())
        .await
        .expect("resolved");

    assert_eq!(read.reference, artifact);
    assert_eq!(read.content, content);
}

#[tokio::test]
async fn default_limit_is_64_mib_and_enforced() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectDriver::default()));
    assert_eq!(adapter.limits().max_artifact_bytes, 64 * 1024 * 1024);
    let oversize = Bytes::from(vec![0_u8; 64 * 1024 * 1024 + 1]);
    let error = stage_required_artifact(&adapter, scope(), oversize, metadata())
        .await
        .expect_err("must reject");
    assert_eq!(error.code(), ARTIFACT_TOO_LARGE);
}

#[tokio::test]
async fn cross_scope_get_fails_closed() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectDriver::default()));
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
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let content = Bytes::from(vec![5_u8; 1024]);
    let artifact = stage_required_artifact(&adapter, scope(), content, metadata())
        .await
        .expect("staged");

    object_store.fail_next_get_with_integrity();
    let error = adapter.get(scope(), artifact).await.expect_err("must fail");
    assert_eq!(error.code(), ARTIFACT_INTEGRITY_FAILURE);
}

#[tokio::test]
async fn exact_replay_is_idempotent_and_metadata_variants_do_not_alias() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let content = Bytes::from_static(b"same bytes");
    let first = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .expect("first stage");
    let replay = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .expect("idempotent replay");
    assert_eq!(first, replay);

    let mut renamed = metadata();
    renamed.name = Some(Arc::from("renamed.bin"));
    let second = stage_required_artifact(&adapter, scope(), content, renamed)
        .await
        .expect("metadata variant");
    assert_eq!(first.id(), second.id(), "wire ids remain content-derived");
    assert_ne!(first, second, "exact references remain independent");
}

#[tokio::test]
async fn pinned_objects_survive_gc_and_unpinned_objects_observe_grace() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let artifact =
        stage_required_artifact(&adapter, scope(), Bytes::from_static(b"owned"), metadata())
            .await
            .expect("stage");
    let owner = ArtifactOwnerId::try_new("journal:test").expect("owner");
    adapter
        .pin(scope(), artifact.clone(), owner.clone())
        .await
        .expect("pin");
    let much_later = Timestamp::from_unix_ms(1_000_000).expect("time");
    assert_eq!(
        adapter
            .collect_orphans(scope(), much_later, 8)
            .await
            .expect("pinned gc")
            .deleted,
        0
    );

    adapter
        .unpin(scope(), artifact.clone(), owner, much_later)
        .await
        .expect("unpin");
    let before_grace = Timestamp::from_unix_ms(1_299_999).expect("time");
    assert_eq!(
        adapter
            .collect_orphans(scope(), before_grace, 8)
            .await
            .expect("before grace")
            .deleted,
        0
    );
    adapter
        .get(scope(), artifact.clone())
        .await
        .expect("artifact must remain before grace");
    let listed = object_store
        .list(
            crate::artifact::to_object_scope(&scope()),
            Some(ObjectKey::try_new("artifacts/v2").expect("prefix")),
            PageToken::first(),
        )
        .await
        .expect("list");
    assert_eq!(listed.entries.len(), 1, "artifact must remain discoverable");
    let after_grace = Timestamp::from_unix_ms(1_300_000).expect("time");
    let report = adapter
        .collect_orphans(scope(), after_grace, 8)
        .await
        .expect("after grace");
    assert_eq!(
        report.deleted, 1,
        "expected eligible artifact deletion, got {report:?}"
    );
    assert!(adapter.get(scope(), artifact).await.is_err());
}
