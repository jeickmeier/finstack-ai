use super::*;

use finstack_ai_kernel::{
    AppendBatchTag, ArtifactRef, EffectOutputContract, EffectOutputKind, LaneTag,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft, RunTag, Sensitivity,
};

use crate::Bytes;
use crate::artifact::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactScope,
    ArtifactStore, build_artifact_ref,
};
use crate::ports::PortFuture;

#[derive(Default)]
struct RecordingArtifactStore {
    pins: Mutex<Vec<(ArtifactScope, ArtifactRef, ArtifactOwnerId)>>,
}

impl ArtifactStore for RecordingArtifactStore {
    fn stage_put(
        &self,
        _: ArtifactScope,
        _: Bytes,
        _: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        Box::pin(async { Err(ArtifactError::NotFound) })
    }

    fn get(&self, _: ArtifactScope, _: ArtifactRef) -> PortFuture<Result<Bytes, ArtifactError>> {
        Box::pin(async { Err(ArtifactError::NotFound) })
    }

    fn pin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
    ) -> PortFuture<Result<(), ArtifactError>> {
        self.pins
            .lock()
            .expect("pins")
            .push((scope, artifact, owner));
        Box::pin(async { Ok(()) })
    }

    fn unpin(
        &self,
        _: ArtifactScope,
        _: ArtifactRef,
        _: ArtifactOwnerId,
        _: Timestamp,
    ) -> PortFuture<Result<(), ArtifactError>> {
        Box::pin(async { Ok(()) })
    }

    fn collect_orphans(
        &self,
        _: ArtifactScope,
        _: Timestamp,
        _: usize,
    ) -> PortFuture<Result<ArtifactGcReport, ArtifactError>> {
        Box::pin(async { Ok(ArtifactGcReport::default()) })
    }
}

fn artifact_locator() -> OperationLocator {
    OperationLocator::try_new(
        "tenant-a",
        fixed_id::<SessionTag>(801),
        fixed_id::<LaneTag>(802),
        fixed_id::<RunTag>(803),
    )
    .expect("locator")
}

fn owned_artifact(locator: &OperationLocator) -> (ArtifactScope, ArtifactRef) {
    let scope = ArtifactScope {
        tenant_scope: Arc::clone(&locator.tenant_scope),
        session_id: locator.session_id,
        run_id: Some(locator.run_id),
        sensitivity: Sensitivity::Confidential,
    };
    let artifact = build_artifact_ref(
        &scope,
        b"committed artifact",
        &ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from("application/octet-stream"),
            name: None,
            attributes: Metadata::empty(),
        },
        &crate::artifact::ArtifactStoreLimits::default(),
    )
    .expect("artifact");
    (scope, artifact)
}

#[test]
fn committed_artifact_pin_reconstructs_the_exact_scope() {
    let locator = artifact_locator();
    let (scope, artifact) = owned_artifact(&locator);
    let store = Arc::new(RecordingArtifactStore::default());
    let mut sources = SettlementSources::try_new(
        ExternalClock::new(fixed_timestamp(900)),
        CountingRandom(AtomicU64::new(1)),
    )
    .expect("sources");
    sources.attach_artifact_store(Arc::clone(&store) as Arc<_>, locator.clone());

    block_on(sources.pin_committed_artifacts(&locator, std::slice::from_ref(&artifact)))
        .expect("pin");
    let pins = store.pins.lock().expect("pins");
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].0, scope);
    assert_eq!(pins[0].1, artifact);
    assert_eq!(
        pins[0].2.as_str(),
        format!("journal:{}", locator.session_id)
    );
}

#[cfg(feature = "native-tokio")]
#[test]
fn recovery_replays_committed_artifact_ownership() {
    let locator = artifact_locator();
    let (_, artifact) = owned_artifact(&locator);
    let completion = EffectCompleted::try_new(
        fixed_id(804),
        EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"tool-result"),
        },
        RawJson::parse(br#"{"ok":true}"#).expect("output"),
        None,
        vec![artifact.clone()],
        ProviderIds::empty(),
        None::<&str>,
        None,
    )
    .expect("completion");
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        fixed_id(805),
        locator.session_id,
        locator.lane_id,
        Some(locator.run_id),
        fixed_timestamp(901),
        vec![fixed_id(806)],
        RecordBody::EffectCompleted(completion),
    )
    .expect("draft");
    let journal = Arc::new(MemoryStore::new());
    block_on(
        journal.append(
            AppendRequest::try_new(
                fixed_id::<AppendBatchTag>(807),
                locator.session_id,
                1,
                vec![draft],
            )
            .expect("append request"),
        ),
    )
    .expect("append");
    let coordinator = CommitCoordinator::new(Arc::clone(&journal) as Arc<_>);
    let store = Arc::new(RecordingArtifactStore::default());
    let mut sources = SettlementSources::try_new(
        ExternalClock::new(fixed_timestamp(902)),
        CountingRandom(AtomicU64::new(2)),
    )
    .expect("sources");
    sources.attach_artifact_store(Arc::clone(&store) as Arc<_>, locator);

    block_on(sources.reconcile_recovered_artifacts(&coordinator)).expect("reconcile");
    let pins = store.pins.lock().expect("pins");
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].1, artifact);
}
