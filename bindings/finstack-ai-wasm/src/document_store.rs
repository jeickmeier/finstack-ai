//! Internal in-memory `ArtifactStore` dedicated to document-attachment staging.
//!
//! Not exposed to JavaScript, and distinct from [`crate::host_artifact::HostArtifactStore`]
//! (which on `wasm32` delegates persistence to a JS host adapter). Run
//! attachments never need cross-reload durability, so a plain in-memory map
//! is sufficient here and keeps document ingestion usable without requiring
//! every host to supply an artifact-store adapter.
//!
//! One instance is shared between attachment staging (`Agent::start` /
//! `Agent::run` / `Lane::run`), the registered `DocumentToolset`, and
//! `DocumentIngestMiddleware` so all three resolve the exact same staged
//! `ArtifactRef` (the single-instance invariant documented on
//! `DocumentIngestMiddleware`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use finstack_ai::runtime::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes, PortFuture,
};
use finstack_ai_kernel::ArtifactRef;

use crate::host_artifact::build_artifact;

type Entries = Arc<Mutex<BTreeMap<String, (ArtifactRef, Bytes)>>>;

/// Bounded in-memory artifact store used only to stage run attachments for
/// document ingestion.
#[derive(Default)]
pub struct DocumentArtifactStore {
    entries: Entries,
}

impl ArtifactStore for DocumentArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        let artifact = match build_artifact(&scope, &content, &metadata) {
            Ok(artifact) => artifact,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let entries = Arc::clone(&self.entries);
        let key = artifact.id().to_canonical_string();
        Box::pin(async move {
            entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key, (artifact.clone(), content));
            Ok(artifact)
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        let entries = Arc::clone(&self.entries);
        let key = artifact.id().to_canonical_string();
        Box::pin(async move {
            let entries = entries.lock().unwrap_or_else(PoisonError::into_inner);
            entries
                .get(&key)
                .map(|(_, bytes)| bytes.clone())
                .ok_or(ArtifactError::NotFound)
        })
    }
}
