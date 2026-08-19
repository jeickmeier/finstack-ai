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

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use finstack_ai::runtime::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes, PortFuture,
};
use finstack_ai_kernel::ArtifactRef;

use crate::host_artifact::build_artifact;

/// Maximum total staged content bytes retained across all entries before
/// oldest-first (FIFO) eviction kicks in. Keeps memory bounded for
/// long-lived agents/hosts that stage many run attachments over their
/// lifetime; 64 MiB comfortably covers many attachments at the toolset's
/// per-attachment cap (`DocumentLimits::max_input_bytes`, 4 MiB by
/// default) without growing unboundedly.
const MAX_TOTAL_CONTENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default)]
struct StoreState {
    entries: BTreeMap<String, (ArtifactRef, Bytes)>,
    insertion_order: VecDeque<String>,
    total_bytes: u64,
}

type Entries = Arc<Mutex<StoreState>>;

/// Bounded in-memory artifact store used only to stage run attachments for
/// document ingestion.
///
/// Bounded by [`MAX_TOTAL_CONTENT_BYTES`] total staged content bytes: once
/// staging a new artifact would push the running total over the cap, the
/// oldest-staged entries (FIFO, by insertion order) are evicted first,
/// until the total is back at or under the cap. A `get()` for an evicted
/// artifact returns [`ArtifactError::NotFound`], which callers already
/// treat as fail-soft (see the ingest middleware's "could not be read"
/// note).
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
            let mut state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            let content_len = content.len() as u64;
            if let Some((_, previous)) = state.entries.get(&key) {
                state.total_bytes = state.total_bytes.saturating_sub(previous.len() as u64);
            } else {
                state.insertion_order.push_back(key.clone());
            }
            state.entries.insert(key, (artifact.clone(), content));
            state.total_bytes = state.total_bytes.saturating_add(content_len);
            while state.total_bytes > MAX_TOTAL_CONTENT_BYTES {
                let Some(oldest) = state.insertion_order.pop_front() else {
                    break;
                };
                if let Some((_, bytes)) = state.entries.remove(&oldest) {
                    state.total_bytes = state.total_bytes.saturating_sub(bytes.len() as u64);
                }
            }
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
            let state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            state
                .entries
                .get(&key)
                .map(|(_, bytes)| bytes.clone())
                .ok_or(ArtifactError::NotFound)
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{DocumentArtifactStore, MAX_TOTAL_CONTENT_BYTES};
    use crate::executor::block_on_ready;
    use finstack_ai::runtime::{
        ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes,
    };
    use finstack_ai_kernel::{Metadata, Sensitivity, SessionId};

    fn scope() -> ArtifactScope {
        ArtifactScope {
            tenant_scope: std::sync::Arc::from("tenant-a"),
            session_id: SessionId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("session"),
            run_id: None,
            sensitivity: Sensitivity::Public,
        }
    }

    fn metadata(name: &str) -> ArtifactMetadata {
        ArtifactMetadata {
            kind: std::sync::Arc::from("attachment"),
            media_type: std::sync::Arc::from("application/octet-stream"),
            name: Some(std::sync::Arc::from(name)),
            attributes: Metadata::empty(),
        }
    }

    #[test]
    fn round_trips_bytes_within_budget() {
        let store = DocumentArtifactStore::default();
        let artifact =
            block_on_ready(store.stage_put(scope(), Bytes::from_static(b"hello"), metadata("a")))
                .expect("stage");
        let got = block_on_ready(store.get(scope(), artifact)).expect("get");
        assert_eq!(&got[..], b"hello");
    }

    #[test]
    fn oldest_entry_is_evicted_once_total_bytes_exceeds_cap() {
        let store = DocumentArtifactStore::default();
        // Each chunk is over half the cap, so the second stage_put must
        // evict the first before it fits.
        let chunk_len = usize::try_from(MAX_TOTAL_CONTENT_BYTES / 2 + 1).expect("fits usize");
        let first = vec![1_u8; chunk_len];
        let second = vec![2_u8; chunk_len];

        let first_artifact =
            block_on_ready(store.stage_put(scope(), Bytes::from(first), metadata("first")))
                .expect("stage first");
        let second_artifact =
            block_on_ready(store.stage_put(scope(), Bytes::from(second), metadata("second")))
                .expect("stage second");

        let first_result = block_on_ready(store.get(scope(), first_artifact));
        assert!(
            matches!(first_result, Err(ArtifactError::NotFound)),
            "oldest artifact must be evicted once the byte budget is exceeded"
        );
        let second_result = block_on_ready(store.get(scope(), second_artifact));
        assert!(
            second_result.is_ok(),
            "most recently staged artifact must remain readable"
        );
    }
}
