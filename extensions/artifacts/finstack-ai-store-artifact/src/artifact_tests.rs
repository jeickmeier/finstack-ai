use std::sync::Arc;

use crate::driver::{ObjectDriver, ObjectKey, PageToken};
use finstack_ai_kernel::{Metadata, Sensitivity, SessionId, Timestamp};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ARTIFACT_INTEGRITY_FAILURE, ArtifactMetadata, ArtifactOwnerId, ArtifactScope, ArtifactStore,
    stage_required_artifact,
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

#[tokio::test]
async fn bounded_gc_advances_past_pinned_objects_across_adapter_restarts() {
    bounded_gc(Arc::new(FakeObjectDriver::default())).await;
}

#[cfg(feature = "local")]
#[tokio::test]
async fn local_bounded_gc_keeps_scan_progress_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    bounded_gc(Arc::new(
        crate::local::LocalObjectStore::try_new(dir.path().to_path_buf()).unwrap(),
    ))
    .await;
}

async fn bounded_gc(objects: Arc<dyn ObjectDriver>) {
    let adapter = ObjectArtifactStore::new(objects.clone());
    let mut artifacts = Vec::new();
    for content in [b"one".as_slice(), b"two", b"three"] {
        artifacts.push(
            stage_required_artifact(
                &adapter,
                scope(),
                Bytes::copy_from_slice(content),
                metadata(),
            )
            .await
            .unwrap(),
        );
    }
    artifacts.sort_by_key(|a| {
        finstack_ai_runtime::artifact::artifact_storage_key(&scope(), a)
            .unwrap()
            .to_hex()
    });
    adapter
        .pin(
            scope(),
            artifacts[0].clone(),
            ArtifactOwnerId::try_new("journal:retained").unwrap(),
        )
        .await
        .unwrap();
    let mut deleted = 0;
    for tick in 1..=12 {
        // Recreate the adapter each time: scan progress belongs to the scope,
        // not an unbounded in-process tenant map or one worker's lifetime.
        let reopened = ObjectArtifactStore::new(objects.clone());
        let report = reopened
            .collect_orphans(scope(), Timestamp::from_unix_ms(tick * 400_000).unwrap(), 1)
            .await
            .unwrap();
        assert!(report.examined <= 1);
        deleted += report.deleted;
    }
    assert_eq!(deleted, 2);
    adapter.get(scope(), artifacts[0].clone()).await.unwrap();
    for artifact in &artifacts[1..] {
        assert!(adapter.get(scope(), artifact.clone()).await.is_err());
    }
}

// ---------------------------------------------------------------------------
// Ownership sidecar, blob index, and GC ordering
// ---------------------------------------------------------------------------

use crate::artifact::{
    ArtifactEnvelopeHeader, OwnerSidecar, artifact_key, by_blob_key, decode_blob_index,
    decode_sidecar, encode_envelope, encode_sidecar, owners_key, to_object_scope,
};
use crate::driver::{ObjectError, ObjectMetadata, ObjectScope};
use finstack_ai_kernel::{ArtifactRef, Digest};
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactStoreLimits, artifact_storage_key, build_artifact_ref,
};
use std::collections::BTreeSet;

const ORPHAN_GRACE_MS: i64 = 5 * 60 * 1_000;

fn object_scope() -> ObjectScope {
    to_object_scope(&scope())
}

fn envelope_key(artifact: &ArtifactRef) -> crate::driver::ObjectKey {
    artifact_key(&artifact_storage_key(&scope(), artifact).unwrap()).unwrap()
}

fn sidecar_key(artifact: &ArtifactRef) -> crate::driver::ObjectKey {
    owners_key(&artifact_storage_key(&scope(), artifact).unwrap()).unwrap()
}

fn index_key(artifact: &ArtifactRef) -> crate::driver::ObjectKey {
    by_blob_key(artifact.blob()).unwrap()
}

fn owner(name: &str) -> ArtifactOwnerId {
    ArtifactOwnerId::try_new(name).unwrap()
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).unwrap()
}

fn envelope_metadata() -> ObjectMetadata {
    ObjectMetadata {
        media_type: Arc::from("application/vnd.finstack.artifact-v2"),
        name: None,
        attributes: Metadata::empty(),
    }
}

async fn raw(store: &FakeObjectDriver, key: &crate::driver::ObjectKey) -> Option<Bytes> {
    match store.get(object_scope(), key.clone()).await {
        Ok(bytes) => Some(bytes),
        Err(ObjectError::NotFound) => None,
        Err(error) => panic!("unexpected driver error: {error:?}"),
    }
}

async fn sidecar(store: &FakeObjectDriver, artifact: &ArtifactRef) -> Option<OwnerSidecar> {
    raw(store, &sidecar_key(artifact))
        .await
        .map(|bytes| decode_sidecar(&bytes).unwrap())
}

/// Write an envelope exactly as the pre-sidecar version did: owners and the
/// unreferenced clock live in the header and nothing else exists.
async fn put_legacy_envelope(
    store: &FakeObjectDriver,
    content: &[u8],
    owners: &[&str],
) -> (ArtifactRef, Bytes) {
    let limits = ArtifactStoreLimits::default();
    let artifact = build_artifact_ref(&scope(), content, &metadata(), &limits).unwrap();
    let header = ArtifactEnvelopeHeader {
        artifact: artifact.clone(),
        owners: owners.iter().map(|name| owner(name)).collect(),
        unreferenced_since: None,
    };
    let envelope = encode_envelope(&header, content).unwrap();
    store
        .put(
            object_scope(),
            envelope_key(&artifact),
            envelope.clone(),
            envelope_metadata(),
        )
        .await
        .unwrap();
    (artifact, envelope)
}

#[tokio::test]
async fn pin_seeds_sidecar_from_v2_envelope_and_leaves_content_untouched() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let (artifact, envelope) =
        put_legacy_envelope(&object_store, b"legacy pinned bytes", &["journal:legacy"]).await;
    assert!(sidecar(&object_store, &artifact).await.is_none());

    adapter
        .pin(scope(), artifact.clone(), owner("journal:new"))
        .await
        .expect("pin seeds the sidecar");

    let seeded = sidecar(&object_store, &artifact).await.expect("sidecar");
    assert_eq!(
        seeded.owners,
        BTreeSet::from([owner("journal:legacy"), owner("journal:new")]),
        "legacy header owners are carried into the sidecar"
    );
    assert_eq!(seeded.schema, 1);
    assert_eq!(seeded.unreferenced_since, None);
    assert_eq!(seeded.collecting_since, None);
    assert_eq!(seeded.envelope_digest, Digest::blob_content(&envelope));
    assert_eq!(
        raw(&object_store, &envelope_key(&artifact)).await.unwrap(),
        envelope,
        "the envelope object is byte-identical after pin"
    );
    assert_eq!(
        adapter.get(scope(), artifact.clone()).await.unwrap(),
        Bytes::from_static(b"legacy pinned bytes")
    );

    // Idempotent re-pin writes nothing new.
    adapter
        .pin(scope(), artifact.clone(), owner("journal:new"))
        .await
        .unwrap();
    assert_eq!(sidecar(&object_store, &artifact).await.unwrap(), seeded);
}

#[tokio::test]
async fn unpin_sets_unreferenced_since_on_sidecar() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let artifact =
        stage_required_artifact(&adapter, scope(), Bytes::from_static(b"owned"), metadata())
            .await
            .unwrap();
    let envelope = raw(&object_store, &envelope_key(&artifact)).await.unwrap();
    adapter
        .pin(scope(), artifact.clone(), owner("journal:a"))
        .await
        .unwrap();
    adapter
        .pin(scope(), artifact.clone(), owner("journal:b"))
        .await
        .unwrap();

    adapter
        .unpin(scope(), artifact.clone(), owner("journal:a"), at(1_000))
        .await
        .unwrap();
    let partial = sidecar(&object_store, &artifact).await.unwrap();
    assert_eq!(partial.owners, BTreeSet::from([owner("journal:b")]));
    assert_eq!(partial.unreferenced_since, None, "still referenced");

    adapter
        .unpin(scope(), artifact.clone(), owner("journal:b"), at(2_000))
        .await
        .unwrap();
    let empty = sidecar(&object_store, &artifact).await.unwrap();
    assert!(empty.owners.is_empty());
    assert_eq!(empty.unreferenced_since, Some(2_000));

    // Unknown owner: no-op, clock untouched.
    adapter
        .unpin(scope(), artifact.clone(), owner("journal:zzz"), at(3_000))
        .await
        .unwrap();
    assert_eq!(sidecar(&object_store, &artifact).await.unwrap(), empty);
    adapter
        .pin(scope(), artifact.clone(), owner("journal:c"))
        .await
        .unwrap();
    assert_eq!(
        sidecar(&object_store, &artifact)
            .await
            .unwrap()
            .unreferenced_since,
        None,
        "re-pin clears the clock"
    );
    assert_eq!(
        raw(&object_store, &envelope_key(&artifact)).await.unwrap(),
        envelope,
        "no lifecycle change rewrites the envelope"
    );
    let header: ArtifactEnvelopeHeader =
        serde_json::from_slice(&envelope[12..envelope.len() - b"owned".len()]).unwrap();
    assert!(header.owners.is_empty(), "the header never learns owners");
}

#[tokio::test]
async fn get_by_blob_uses_index_then_falls_back_to_scan() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let content = Bytes::from_static(b"indexed attachment");
    let artifact = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .unwrap();
    let index = raw(&object_store, &index_key(&artifact))
        .await
        .expect("stage_put writes the index");
    assert_eq!(decode_blob_index(&index).unwrap().artifact, artifact);

    let read = adapter
        .get_by_blob(scope(), artifact.blob().clone())
        .await
        .unwrap();
    assert_eq!(read.reference, artifact);
    assert_eq!(read.content, content);
    assert_eq!(
        object_store.list_calls(),
        0,
        "the index answers without a scan"
    );

    object_store
        .delete(object_scope(), index_key(&artifact))
        .await
        .unwrap();
    let read = adapter
        .get_by_blob(scope(), artifact.blob().clone())
        .await
        .expect("pre-index data resolves by scan");
    assert_eq!(read.reference, artifact);
    assert_eq!(object_store.list_calls(), 1);
    assert_eq!(
        raw(&object_store, &index_key(&artifact)).await.unwrap(),
        index,
        "the scan hit re-creates the index"
    );
    adapter
        .get_by_blob(scope(), artifact.blob().clone())
        .await
        .unwrap();
    assert_eq!(object_store.list_calls(), 1, "back on the index path");

    // A dangling index entry (envelope gone) is removed and the lookup
    // reports NotFound instead of looping on the stale entry.
    object_store
        .delete(object_scope(), envelope_key(&artifact))
        .await
        .unwrap();
    assert_eq!(
        adapter
            .get_by_blob(scope(), artifact.blob().clone())
            .await
            .unwrap_err(),
        ArtifactError::NotFound
    );
    assert!(raw(&object_store, &index_key(&artifact)).await.is_none());
}

#[tokio::test]
async fn collect_orphans_deletes_envelope_sidecar_and_index() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let content = Bytes::from_static(b"collectible bytes");
    let artifact = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .unwrap();
    adapter
        .pin(scope(), artifact.clone(), owner("journal:x"))
        .await
        .unwrap();
    adapter
        .unpin(scope(), artifact.clone(), owner("journal:x"), at(1_000_000))
        .await
        .unwrap();

    let early = adapter
        .collect_orphans(scope(), at(1_000_000 + ORPHAN_GRACE_MS - 1), 8)
        .await
        .unwrap();
    assert_eq!((early.examined, early.deleted), (1, 0));
    assert!(raw(&object_store, &envelope_key(&artifact)).await.is_some());

    let report = adapter
        .collect_orphans(scope(), at(1_000_000 + ORPHAN_GRACE_MS), 8)
        .await
        .unwrap();
    assert_eq!((report.examined, report.deleted), (1, 1));
    assert_eq!(report.bytes_deleted, content.len() as u64);
    assert!(raw(&object_store, &envelope_key(&artifact)).await.is_none());
    assert!(raw(&object_store, &sidecar_key(&artifact)).await.is_none());
    assert!(raw(&object_store, &index_key(&artifact)).await.is_none());
    assert_eq!(
        adapter.get(scope(), artifact.clone()).await.unwrap_err(),
        ArtifactError::NotFound
    );
    assert_eq!(
        adapter
            .get_by_blob(scope(), artifact.blob().clone())
            .await
            .unwrap_err(),
        ArtifactError::NotFound
    );
}

#[tokio::test]
async fn collect_orphans_tolerates_dangling_sidecar() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let gone = stage_required_artifact(&adapter, scope(), Bytes::from_static(b"gone"), metadata())
        .await
        .unwrap();
    adapter
        .pin(scope(), gone.clone(), owner("journal:g"))
        .await
        .unwrap();
    // Crash prefix: the envelope and index vanished, the sidecar did not.
    object_store
        .delete(object_scope(), envelope_key(&gone))
        .await
        .unwrap();
    let dangling = raw(&object_store, &sidecar_key(&gone)).await.unwrap();
    let live = stage_required_artifact(&adapter, scope(), Bytes::from_static(b"live"), metadata())
        .await
        .unwrap();

    let first = adapter
        .collect_orphans(scope(), at(10_000), 8)
        .await
        .unwrap();
    assert_eq!(
        (first.examined, first.deleted),
        (1, 0),
        "only envelopes are listed"
    );
    assert_eq!(
        sidecar(&object_store, &live)
            .await
            .unwrap()
            .unreferenced_since,
        Some(10_000),
        "the live orphan is stamped"
    );
    let second = adapter
        .collect_orphans(scope(), at(10_000 + ORPHAN_GRACE_MS), 8)
        .await
        .unwrap();
    assert_eq!((second.examined, second.deleted), (1, 1));
    assert_eq!(
        raw(&object_store, &sidecar_key(&gone)).await.unwrap(),
        dangling,
        "an unlisted sidecar is neither read nor removed"
    );
    assert_eq!(
        adapter
            .pin(scope(), gone.clone(), owner("journal:again"))
            .await
            .unwrap_err(),
        ArtifactError::NotFound,
        "a sidecar never resurrects a missing envelope"
    );
    assert!(
        raw(&object_store, &sidecar_key(&gone)).await.is_none(),
        "the failed pin removes the dangling sidecar"
    );
}

#[tokio::test]
async fn pin_fails_closed_on_fresh_tombstone_and_gc_resets_stale_one() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let artifact =
        stage_required_artifact(&adapter, scope(), Bytes::from_static(b"marked"), metadata())
            .await
            .unwrap();
    let envelope = raw(&object_store, &envelope_key(&artifact)).await.unwrap();
    // A pass committed to deletion at t=50_000 and died before finishing.
    let tombstone = OwnerSidecar {
        schema: 1,
        owners: BTreeSet::new(),
        unreferenced_since: Some(0),
        envelope_digest: Digest::blob_content(&envelope),
        collecting_since: Some(50_000),
    };
    object_store
        .put(
            object_scope(),
            sidecar_key(&artifact),
            encode_sidecar(&tombstone).unwrap(),
            envelope_metadata(),
        )
        .await
        .unwrap();

    assert_eq!(
        adapter
            .pin(scope(), artifact.clone(), owner("journal:late"))
            .await
            .unwrap_err(),
        ArtifactError::NotFound
    );
    // Within one grace the mark is honoured: nothing is deleted or reset.
    let fresh = adapter
        .collect_orphans(scope(), at(50_001), 8)
        .await
        .unwrap();
    assert_eq!(fresh.deleted, 0);
    assert_eq!(sidecar(&object_store, &artifact).await.unwrap(), tombstone);
    // Past it the mark is stale: lift it and restart the clock.
    let reset_at = 50_000 + ORPHAN_GRACE_MS;
    let stale = adapter
        .collect_orphans(scope(), at(reset_at), 8)
        .await
        .unwrap();
    assert_eq!(stale.deleted, 0);
    let reset = sidecar(&object_store, &artifact).await.unwrap();
    assert_eq!(reset.collecting_since, None);
    assert_eq!(reset.unreferenced_since, Some(reset_at));
    assert_eq!(
        raw(&object_store, &envelope_key(&artifact)).await.unwrap(),
        envelope
    );
    adapter
        .pin(scope(), artifact.clone(), owner("journal:late"))
        .await
        .expect("pinnable again");
    let kept = adapter
        .collect_orphans(scope(), at(reset_at + ORPHAN_GRACE_MS), 8)
        .await
        .unwrap();
    assert_eq!(kept.deleted, 0, "the new owner holds it");
}

#[tokio::test]
async fn gc_heals_sidecar_whose_envelope_digest_is_stale() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let artifact = stage_required_artifact(
        &adapter,
        scope(),
        Bytes::from_static(b"rewritten"),
        metadata(),
    )
    .await
    .unwrap();
    let envelope = raw(&object_store, &envelope_key(&artifact)).await.unwrap();
    // A pre-sidecar process rewrote the envelope after the sidecar was
    // seeded: the recorded digest no longer matches the stored bytes.
    let stale = OwnerSidecar {
        schema: 1,
        owners: BTreeSet::new(),
        unreferenced_since: Some(0),
        envelope_digest: Digest::blob_content(b"what the envelope used to be"),
        collecting_since: None,
    };
    object_store
        .put(
            object_scope(),
            sidecar_key(&artifact),
            encode_sidecar(&stale).unwrap(),
            envelope_metadata(),
        )
        .await
        .unwrap();

    let first = adapter
        .collect_orphans(scope(), at(ORPHAN_GRACE_MS), 8)
        .await
        .unwrap();
    assert_eq!(first.deleted, 0, "a mismatched digest never deletes");
    assert_eq!(
        raw(&object_store, &envelope_key(&artifact)).await.unwrap(),
        envelope
    );
    let healed = sidecar(&object_store, &artifact).await.unwrap();
    assert_eq!(healed.envelope_digest, Digest::blob_content(&envelope));
    assert_eq!(healed.collecting_since, None);
    assert_eq!(healed.unreferenced_since, Some(ORPHAN_GRACE_MS));
    adapter
        .pin(scope(), artifact.clone(), owner("journal:probe"))
        .await
        .expect("pinnable while it waits out the restarted grace");
    adapter
        .unpin(
            scope(),
            artifact.clone(),
            owner("journal:probe"),
            at(ORPHAN_GRACE_MS),
        )
        .await
        .unwrap();
    let second = adapter
        .collect_orphans(scope(), at(2 * ORPHAN_GRACE_MS), 8)
        .await
        .unwrap();
    assert_eq!(
        second.deleted, 1,
        "collected on the next pass with the real digest"
    );
    assert!(raw(&object_store, &sidecar_key(&artifact)).await.is_none());
}

#[tokio::test]
async fn restaged_artifact_clears_stale_tombstone() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let content = Bytes::from_static(b"re-staged");
    let artifact = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .unwrap();
    adapter
        .pin(scope(), artifact.clone(), owner("journal:brief"))
        .await
        .unwrap();
    adapter
        .unpin(scope(), artifact.clone(), owner("journal:brief"), at(0))
        .await
        .unwrap();
    let collected = adapter
        .collect_orphans(scope(), at(ORPHAN_GRACE_MS), 8)
        .await
        .unwrap();
    assert_eq!(collected.deleted, 1);
    // Crash prefix: envelope deleted, tombstone left behind.
    assert!(raw(&object_store, &envelope_key(&artifact)).await.is_none());
    let mut tombstone = OwnerSidecar {
        schema: 1,
        owners: BTreeSet::new(),
        unreferenced_since: Some(0),
        envelope_digest: Digest::blob_content(b"irrelevant"),
        collecting_since: Some(ORPHAN_GRACE_MS),
    };
    object_store
        .put(
            object_scope(),
            sidecar_key(&artifact),
            encode_sidecar(&tombstone).unwrap(),
            envelope_metadata(),
        )
        .await
        .unwrap();

    let again = stage_required_artifact(&adapter, scope(), content, metadata())
        .await
        .unwrap();
    assert_eq!(again, artifact);
    assert!(
        sidecar(&object_store, &artifact).await.is_none(),
        "a fresh envelope clears the predecessor's tombstone"
    );
    adapter
        .pin(scope(), artifact.clone(), owner("journal:fresh"))
        .await
        .expect("pin works immediately after re-staging");
    tombstone.collecting_since = None;
    let live = sidecar(&object_store, &artifact).await.unwrap();
    assert_eq!(live.owners, BTreeSet::from([owner("journal:fresh")]));
}

#[tokio::test]
async fn stage_put_duplicate_blob_index_is_verified() {
    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    let content = Bytes::from_static(b"shared bytes");
    let first = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .unwrap();
    let index = raw(&object_store, &index_key(&first)).await.unwrap();

    // Same exact blob under another kind: the entry already names an
    // artifact with this blob, so staging succeeds and the entry is kept.
    let mut other_kind = metadata();
    other_kind.kind = Arc::from("report");
    let second = stage_required_artifact(&adapter, scope(), content.clone(), other_kind)
        .await
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(first.blob(), second.blob());
    assert_eq!(index_key(&first), index_key(&second));
    assert_eq!(raw(&object_store, &index_key(&first)).await.unwrap(), index);
    let read = adapter
        .get_by_blob(scope(), first.blob().clone())
        .await
        .unwrap();
    assert_eq!(read.reference, first);

    // A different name is a different blob and gets its own entry.
    let mut renamed = metadata();
    renamed.name = Some(Arc::from("renamed.bin"));
    let third = stage_required_artifact(&adapter, scope(), content.clone(), renamed)
        .await
        .unwrap();
    assert_ne!(index_key(&first), index_key(&third));
    assert_eq!(
        adapter
            .get_by_blob(scope(), third.blob().clone())
            .await
            .unwrap()
            .reference,
        third
    );
    assert_eq!(object_store.list_calls(), 0);

    // An entry naming an artifact with a different blob is corruption.
    let foreign =
        stage_required_artifact(&adapter, scope(), Bytes::from_static(b"other"), metadata())
            .await
            .unwrap();
    object_store
        .put(
            object_scope(),
            index_key(&first),
            crate::artifact::encode_blob_index(&foreign).unwrap(),
            envelope_metadata(),
        )
        .await
        .unwrap();
    let error = stage_required_artifact(&adapter, scope(), content, metadata())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ArtifactError::Integrity { message } if &**message == "artifact_identity_collision"),
        "got {error:?}"
    );
}

#[cfg(feature = "local")]
#[tokio::test]
async fn stage_put_under_held_lock_is_busy_not_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let driver = crate::local::LocalObjectStore::try_new(dir.path().to_path_buf()).unwrap();
    let adapter = ObjectArtifactStore::new(Arc::new(driver));
    let content = Bytes::from_static(b"locked bytes");
    let artifact = build_artifact_ref(&scope(), &content, &metadata(), &adapter.limits()).unwrap();
    let held =
        crate::local::acquire_mutation_lock(dir.path(), &object_scope(), &envelope_key(&artifact))
            .await
            .unwrap();

    let error = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .expect_err("a held lock is not a stored artifact");
    assert!(
        matches!(&error, ArtifactError::Unavailable { message } if &**message == "artifact_busy"),
        "got {error:?}"
    );
    assert_eq!(
        adapter.get(scope(), artifact.clone()).await.unwrap_err(),
        ArtifactError::NotFound,
        "nothing was stored"
    );

    drop(held);
    let staged = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .unwrap();
    assert_eq!(staged, artifact);
    assert_eq!(adapter.get(scope(), artifact).await.unwrap(), content);
}

// ---------------------------------------------------------------------------
// Compatibility fixture: a v2 envelope written before the sidecar existed
// ---------------------------------------------------------------------------

const FIXTURE_RELATIVE: &str =
    "fixtures/compatibility/artifact/v2/envelope/valid--pinned-owner.bin";
const FIXTURE_CONTENT: &[u8] = b"finstack-ai artifact v2 fixture: pinned by journal:fixture\n";
const FIXTURE_OWNER: &str = "journal:fixture";

/// The checked-in fixture is exactly this: deterministic inputs through the
/// envelope encoder as it existed before the sidecar, owners in the header.
fn fixture_envelope() -> (ArtifactRef, Bytes) {
    let artifact = build_artifact_ref(
        &scope(),
        FIXTURE_CONTENT,
        &metadata(),
        &ArtifactStoreLimits::default(),
    )
    .unwrap();
    let header = ArtifactEnvelopeHeader {
        artifact: artifact.clone(),
        owners: BTreeSet::from([owner(FIXTURE_OWNER)]),
        unreferenced_since: None,
    };
    let envelope = encode_envelope(&header, FIXTURE_CONTENT).unwrap();
    (artifact, envelope)
}

#[tokio::test]
async fn v2_pinned_owner_fixture_pins_through_sidecar() {
    let path = finstack_ai_test::repo_root().join(FIXTURE_RELATIVE);
    let (artifact, expected) = fixture_envelope();
    if std::env::var_os("FINSTACK_WRITE_ARTIFACT_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &expected).unwrap();
    }
    let stored = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert_eq!(
        stored, expected,
        "fixture must be the deterministic v2 encoding; regenerate with FINSTACK_WRITE_ARTIFACT_FIXTURES=1"
    );
    assert_eq!(&stored[..8], b"FSAIART2");

    let object_store = Arc::new(FakeObjectDriver::default());
    let adapter = ObjectArtifactStore::new(Arc::clone(&object_store) as Arc<_>);
    object_store
        .put(
            object_scope(),
            envelope_key(&artifact),
            Bytes::from(stored.clone()),
            envelope_metadata(),
        )
        .await
        .unwrap();

    assert_eq!(
        adapter.get(scope(), artifact.clone()).await.unwrap(),
        Bytes::from_static(FIXTURE_CONTENT)
    );
    let read = adapter
        .get_by_blob(scope(), artifact.blob().clone())
        .await
        .expect("pre-index fixture resolves by scan");
    assert_eq!(read.reference, artifact);
    assert!(raw(&object_store, &index_key(&artifact)).await.is_some());

    adapter
        .pin(scope(), artifact.clone(), owner("journal:upgraded"))
        .await
        .unwrap();
    let seeded = sidecar(&object_store, &artifact).await.unwrap();
    assert_eq!(
        seeded.owners,
        BTreeSet::from([owner(FIXTURE_OWNER), owner("journal:upgraded")])
    );
    assert_eq!(seeded.envelope_digest, Digest::blob_content(&stored));
    assert_eq!(
        raw(&object_store, &envelope_key(&artifact)).await.unwrap(),
        Bytes::from(stored)
    );

    // The fixture owner keeps it alive through GC until released.
    adapter
        .unpin(scope(), artifact.clone(), owner("journal:upgraded"), at(1))
        .await
        .unwrap();
    assert_eq!(
        adapter
            .collect_orphans(scope(), at(ORPHAN_GRACE_MS * 2), 8)
            .await
            .unwrap()
            .deleted,
        0
    );
    adapter
        .unpin(
            scope(),
            artifact.clone(),
            owner(FIXTURE_OWNER),
            at(ORPHAN_GRACE_MS * 2),
        )
        .await
        .unwrap();
    assert_eq!(
        adapter
            .collect_orphans(scope(), at(ORPHAN_GRACE_MS * 3), 8)
            .await
            .unwrap()
            .deleted,
        1
    );
}
