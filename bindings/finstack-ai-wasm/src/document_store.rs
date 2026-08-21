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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use finstack_ai::runtime::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactPersistence,
    ArtifactRead, ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
    Bytes, PortFuture, artifact_storage_key, build_artifact_ref, validate_artifact_scope,
    validate_retrieved_artifact,
};
use finstack_ai_kernel::{ArtifactRef, BlobRef, Digest, Timestamp};

/// Maximum total staged content bytes retained across all entries before
/// oldest-first (FIFO) eviction kicks in. Keeps memory bounded for
/// long-lived agents/hosts that stage many run attachments over their
/// lifetime; 64 MiB comfortably covers many attachments at the toolset's
/// per-attachment cap (`DocumentLimits::max_input_bytes`, 4 MiB by
/// default) without growing unboundedly.
const MAX_TOTAL_CONTENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default)]
struct StoreState {
    entries: BTreeMap<Digest, StoredArtifact>,
    total_bytes: u64,
}

struct StoredArtifact {
    scope: ArtifactScope,
    artifact: ArtifactRef,
    content: Bytes,
    owners: BTreeSet<ArtifactOwnerId>,
    unreferenced_since: Option<Timestamp>,
}

type Entries = Arc<Mutex<StoreState>>;

/// Bounded in-memory artifact store used only to stage run attachments for
/// document ingestion.
///
/// Bounded by [`MAX_TOTAL_CONTENT_BYTES`] total staged content bytes. New
/// writes are rejected at capacity; existing referenced content is never
/// evicted implicitly.
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
        let limits = self.limits();
        let artifact = match build_artifact_ref(&scope, &content, &metadata, &limits) {
            Ok(artifact) => artifact,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            let mut state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(stored) = state.entries.get(&key) {
                if stored.scope == scope && stored.artifact == artifact && stored.content == content
                {
                    return Ok(artifact);
                }
                return Err(ArtifactError::Integrity {
                    message: Arc::from("artifact_identity_collision"),
                });
            }
            if state.entries.len() >= limits.max_artifacts {
                return Err(ArtifactError::CapacityExceeded {
                    resource: "artifacts",
                    limit: u64::try_from(limits.max_artifacts).unwrap_or(u64::MAX),
                });
            }
            let content_len = u64::try_from(content.len()).unwrap_or(u64::MAX);
            let total_bytes = state.total_bytes.checked_add(content_len).ok_or(
                ArtifactError::CapacityExceeded {
                    resource: "total_bytes",
                    limit: limits.max_total_bytes,
                },
            )?;
            if total_bytes > limits.max_total_bytes {
                return Err(ArtifactError::CapacityExceeded {
                    resource: "total_bytes",
                    limit: limits.max_total_bytes,
                });
            }
            state.entries.insert(
                key,
                StoredArtifact {
                    scope,
                    artifact: artifact.clone(),
                    content,
                    owners: BTreeSet::new(),
                    unreferenced_since: None,
                },
            );
            state.total_bytes = total_bytes;
            Ok(artifact)
        })
    }

    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        let key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            validate_artifact_scope(&scope, &artifact)?;
            let state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            let stored = state.entries.get(&key).ok_or(ArtifactError::NotFound)?;
            if stored.scope != scope || stored.artifact != artifact {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("stored_reference_mismatch"),
                });
            }
            validate_retrieved_artifact(&scope, &artifact, &stored.content)?;
            Ok(stored.content.clone())
        })
    }

    fn get_by_blob(
        &self,
        scope: ArtifactScope,
        blob: BlobRef,
    ) -> PortFuture<Result<ArtifactRead, ArtifactError>> {
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            if blob.digest().is_none() {
                return Err(ArtifactError::InvalidMetadata {
                    message: Arc::from("blob_digest_required"),
                });
            }
            let state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            let stored = state
                .entries
                .values()
                .find(|stored| stored.scope == scope && stored.artifact.blob() == &blob)
                .ok_or(ArtifactError::NotFound)?;
            validate_retrieved_artifact(&scope, &stored.artifact, &stored.content)?;
            Ok(ArtifactRead {
                reference: stored.artifact.clone(),
                content: stored.content.clone(),
            })
        })
    }

    fn limits(&self) -> ArtifactStoreLimits {
        ArtifactStoreLimits {
            max_artifact_bytes: usize::try_from(MAX_TOTAL_CONTENT_BYTES).unwrap_or(usize::MAX),
            max_total_bytes: MAX_TOTAL_CONTENT_BYTES,
            ..ArtifactStoreLimits::default()
        }
    }

    fn descriptor(&self) -> ArtifactStoreDescriptor {
        ArtifactStoreDescriptor {
            store_id: Arc::from("wasm.document-artifacts"),
            persistence: ArtifactPersistence::Ephemeral,
            limits: self.limits(),
        }
    }

    fn pin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let entries = Arc::clone(&self.entries);
        let limits = self.limits();
        Box::pin(async move {
            let mut state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            let stored = state.entries.get_mut(&key).ok_or(ArtifactError::NotFound)?;
            if stored.scope != scope || stored.artifact != artifact {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("stored_reference_mismatch"),
                });
            }
            if !stored.owners.contains(&owner)
                && stored.owners.len() >= limits.max_owners_per_artifact
            {
                return Err(ArtifactError::CapacityExceeded {
                    resource: "owners",
                    limit: u64::try_from(limits.max_owners_per_artifact).unwrap_or(u64::MAX),
                });
            }
            stored.owners.insert(owner);
            stored.unreferenced_since = None;
            Ok(())
        })
    }

    fn unpin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
        now: Timestamp,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let key = match artifact_storage_key(&scope, &artifact) {
            Ok(key) => key,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            let mut state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            let stored = state.entries.get_mut(&key).ok_or(ArtifactError::NotFound)?;
            if stored.scope != scope || stored.artifact != artifact {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("stored_reference_mismatch"),
                });
            }
            if stored.owners.remove(&owner) && stored.owners.is_empty() {
                stored.unreferenced_since = Some(now);
            }
            Ok(())
        })
    }

    fn collect_orphans(
        &self,
        scope: ArtifactScope,
        now: Timestamp,
        limit: usize,
    ) -> PortFuture<Result<ArtifactGcReport, ArtifactError>> {
        let entries = Arc::clone(&self.entries);
        let limits = self.limits();
        Box::pin(async move {
            scope.digest()?;
            let mut state = entries.lock().unwrap_or_else(PoisonError::into_inner);
            let mut examined = 0_usize;
            let mut delete = Vec::new();
            for (key, stored) in &mut state.entries {
                if examined >= limit.min(limits.max_gc_batch) || stored.scope != scope {
                    continue;
                }
                examined += 1;
                if !stored.owners.is_empty() {
                    continue;
                }
                let Some(since) = stored.unreferenced_since else {
                    stored.unreferenced_since = Some(now);
                    continue;
                };
                let elapsed = now.as_unix_ms().checked_sub(since.as_unix_ms());
                if elapsed.and_then(|value| u64::try_from(value).ok())
                    >= Some(limits.orphan_grace_ms)
                {
                    delete.push(*key);
                }
            }
            let mut report = ArtifactGcReport {
                examined,
                ..ArtifactGcReport::default()
            };
            for key in delete {
                if let Some(stored) = state.entries.remove(&key) {
                    let bytes = u64::try_from(stored.content.len()).unwrap_or(u64::MAX);
                    state.total_bytes = state.total_bytes.saturating_sub(bytes);
                    report.deleted += 1;
                    report.bytes_deleted = report.bytes_deleted.saturating_add(bytes);
                }
            }
            Ok(report)
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
    fn capacity_rejects_new_content_without_evicting_existing_content() {
        let store = DocumentArtifactStore::default();
        // Each chunk is over half the cap, so the second stage_put must
        // evict the first before it fits.
        let chunk_len = usize::try_from(MAX_TOTAL_CONTENT_BYTES / 2 + 1).expect("fits usize");
        let first = vec![1_u8; chunk_len];
        let second = vec![2_u8; chunk_len];

        let first_artifact =
            block_on_ready(store.stage_put(scope(), Bytes::from(first), metadata("first")))
                .expect("stage first");
        let second_error =
            block_on_ready(store.stage_put(scope(), Bytes::from(second), metadata("second")))
                .expect_err("second artifact exceeds aggregate capacity");

        assert!(
            matches!(second_error, ArtifactError::CapacityExceeded { .. }),
            "capacity must reject rather than evict"
        );
        let first_result = block_on_ready(store.get(scope(), first_artifact));
        assert!(
            first_result.is_ok(),
            "existing artifact must remain readable"
        );
    }
}
