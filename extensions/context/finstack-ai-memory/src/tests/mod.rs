mod observer;
mod provider;
mod record;
mod scale;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
mod sqlite;
mod store;
mod toolset;

use crate::record::*;
use crate::store::InProcessArtifactStore;
use finstack_ai_kernel::{Metadata, RunId, Sensitivity, SessionId, UNIX_EPOCH};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{ArtifactMetadata, ArtifactScope, stage_required_artifact};
use std::sync::Arc;

pub(crate) fn sample_record(id: &str, tenant: &str) -> MemoryRecord {
    MemoryRecord {
        id: MemoryId::parse(id).unwrap(),
        scope: MemoryScope::try_new(tenant).unwrap(),
        keywords: Arc::from([Arc::<str>::from("alpha")]),
        body: MemoryBody::Inline(Arc::from("body text")),
        preview: Arc::from("body text"),
        sensitivity: Sensitivity::Internal,
        provenance: MemoryProvenance {
            source_session: None,
            source_run: None,
            source_ref: None,
            extraction: ExtractionMethod::Explicit,
            confidence: 80,
        },
        created_at: UNIX_EPOCH,
        last_confirmed_at: UNIX_EPOCH,
        supersedes: None,
        superseded_by: None,
        retention: RetentionPolicy::KeepUntilDeleted,
        tombstoned: false,
    }
}

pub(crate) async fn blob_backed_record(id: &str, tenant: &str) -> MemoryRecord {
    let artifacts = InProcessArtifactStore::default();
    let artifact_scope = ArtifactScope {
        tenant_scope: Arc::from(tenant),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: Some(RunId::from_bytes([2; 16])),
        sensitivity: Sensitivity::Internal,
    };
    let artifact = stage_required_artifact(
        &artifacts,
        artifact_scope.clone(),
        Bytes::from_static(b"memory blob"),
        ArtifactMetadata {
            kind: Arc::from("memory-record"),
            media_type: Arc::from("text/plain"),
            name: Some(Arc::from(id)),
            attributes: Metadata::empty(),
        },
    )
    .await
    .unwrap();
    let mut record = sample_record(id, tenant);
    record.body = MemoryBody::Blob {
        scope: artifact_scope,
        artifact,
    };
    record
}
