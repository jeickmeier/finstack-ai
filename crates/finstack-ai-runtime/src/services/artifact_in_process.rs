//! Bounded in-process [`ArtifactStore`] reference implementation.
//!
//! Lives beside the [`ArtifactStore`] contract so callers need not depend
//! on an extension crate. `store_id` and lock-poison messages stay the
//! `finstack-ai-memory` strings so existing descriptors and logs do not
//! change.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use bytes::Bytes;

use crate::ports::PortFuture;
use crate::services::artifact::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactPersistence,
    ArtifactRead, ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
    artifact_storage_key, build_artifact_ref, validate_artifact_scope, validate_retrieved_artifact,
};
use crate::{ArtifactRef, BlobRef, Digest, Timestamp};

/// In-process, non-durable [`ArtifactStore`] reference implementation.
#[derive(Debug)]
pub struct InProcessArtifactStore {
    state: Mutex<ArtifactState>,
    limits: ArtifactStoreLimits,
}

#[derive(Debug, Default)]
struct ArtifactState {
    entries: BTreeMap<Digest, StoredArtifact>,
    total_bytes: u64,
}

#[derive(Debug)]
struct StoredArtifact {
    scope: ArtifactScope,
    artifact: ArtifactRef,
    content: Bytes,
    owners: BTreeSet<ArtifactOwnerId>,
    unreferenced_since: Option<Timestamp>,
}

impl Default for InProcessArtifactStore {
    fn default() -> Self {
        Self {
            state: Mutex::new(ArtifactState::default()),
            limits: ArtifactStoreLimits::default(),
        }
    }
}

impl InProcessArtifactStore {
    /// Override the artifact byte ceiling for this in-process store.
    #[must_use]
    pub fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
        self.limits.max_artifact_bytes = max_artifact_bytes;
        self
    }

    /// Override all finite capacity limits for this in-process store.
    #[must_use]
    pub fn with_limits(mut self, limits: ArtifactStoreLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl ArtifactStore for InProcessArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        let result = (|| {
            let artifact = build_artifact_ref(&scope, &content, &metadata, &self.limits)?;
            let key = artifact_storage_key(&scope, &artifact)?;
            let content_len =
                u64::try_from(content.len()).map_err(|_| ArtifactError::InvalidMetadata {
                    message: Arc::from("invalid_length"),
                })?;
            let mut state = self.state.lock().map_err(|_| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;

            if let Some(stored) = state.entries.get(&key) {
                if stored.scope != scope || stored.artifact != artifact || stored.content != content
                {
                    return Err(ArtifactError::Integrity {
                        message: Arc::from("artifact_identity_collision"),
                    });
                }
                return Ok(artifact);
            }
            if state.entries.len() >= self.limits.max_artifacts {
                return Err(ArtifactError::CapacityExceeded {
                    resource: "artifacts",
                    limit: u64::try_from(self.limits.max_artifacts).unwrap_or(u64::MAX),
                });
            }
            let total_bytes = state.total_bytes.checked_add(content_len).ok_or(
                ArtifactError::CapacityExceeded {
                    resource: "total_bytes",
                    limit: self.limits.max_total_bytes,
                },
            )?;
            if total_bytes > self.limits.max_total_bytes {
                return Err(ArtifactError::CapacityExceeded {
                    resource: "total_bytes",
                    limit: self.limits.max_total_bytes,
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
        })();
        Box::pin(async move { result })
    }

    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        let result = (|| {
            validate_artifact_scope(&scope, &artifact)?;
            let key = artifact_storage_key(&scope, &artifact)?;
            let state = self.state.lock().map_err(|_| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
            let stored = state.entries.get(&key).ok_or(ArtifactError::NotFound)?;
            if stored.scope != scope || stored.artifact != artifact {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("stored_reference_mismatch"),
                });
            }
            validate_retrieved_artifact(&scope, &artifact, &stored.content)?;
            Ok(stored.content.clone())
        })();
        Box::pin(async move { result })
    }

    fn get_by_blob(
        &self,
        scope: ArtifactScope,
        blob: BlobRef,
    ) -> PortFuture<Result<ArtifactRead, ArtifactError>> {
        let result = (|| {
            if blob.digest().is_none() {
                return Err(ArtifactError::InvalidMetadata {
                    message: Arc::from("blob_digest_required"),
                });
            }
            let state = self.state.lock().map_err(|_| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
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
        })();
        Box::pin(async move { result })
    }

    fn limits(&self) -> ArtifactStoreLimits {
        self.limits
    }

    fn descriptor(&self) -> ArtifactStoreDescriptor {
        ArtifactStoreDescriptor {
            store_id: Arc::from("memory.in-process-artifacts"),
            persistence: ArtifactPersistence::Ephemeral,
            limits: self.limits,
        }
    }

    fn pin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let result = (|| {
            validate_artifact_scope(&scope, &artifact)?;
            let key = artifact_storage_key(&scope, &artifact)?;
            let mut state = self.state.lock().map_err(|_| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
            let stored = state.entries.get_mut(&key).ok_or(ArtifactError::NotFound)?;
            if stored.scope != scope || stored.artifact != artifact {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("stored_reference_mismatch"),
                });
            }
            if !stored.owners.contains(&owner)
                && stored.owners.len() >= self.limits.max_owners_per_artifact
            {
                return Err(ArtifactError::CapacityExceeded {
                    resource: "owners",
                    limit: u64::try_from(self.limits.max_owners_per_artifact).unwrap_or(u64::MAX),
                });
            }
            stored.owners.insert(owner);
            stored.unreferenced_since = None;
            Ok(())
        })();
        Box::pin(async move { result })
    }

    fn unpin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
        now: Timestamp,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let result = (|| {
            validate_artifact_scope(&scope, &artifact)?;
            let key = artifact_storage_key(&scope, &artifact)?;
            let mut state = self.state.lock().map_err(|_| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
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
        })();
        Box::pin(async move { result })
    }

    fn collect_orphans(
        &self,
        scope: ArtifactScope,
        now: Timestamp,
        limit: usize,
    ) -> PortFuture<Result<ArtifactGcReport, ArtifactError>> {
        let result = (|| {
            scope.digest()?;
            let bounded_limit = limit.min(self.limits.max_gc_batch);
            let mut state = self.state.lock().map_err(|_| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
            let mut examined = 0_usize;
            let mut delete = Vec::new();
            for (key, stored) in &mut state.entries {
                if examined >= bounded_limit || stored.scope != scope {
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
                    >= Some(self.limits.orphan_grace_ms)
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
                    let content_len = u64::try_from(stored.content.len()).unwrap_or(u64::MAX);
                    state.total_bytes = state.total_bytes.saturating_sub(content_len);
                    report.deleted += 1;
                    report.bytes_deleted = report.bytes_deleted.saturating_add(content_len);
                }
            }
            Ok(report)
        })();
        Box::pin(async move { result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::artifact::{get_required_artifact, stage_required_artifact};
    use crate::{Metadata, RunId, Sensitivity, SessionId};

    fn scope(tenant: &str) -> ArtifactScope {
        ArtifactScope {
            tenant_scope: Arc::from(tenant),
            session_id: SessionId::from_bytes([1; 16]),
            run_id: Some(RunId::from_bytes([2; 16])),
            sensitivity: Sensitivity::Internal,
        }
    }

    fn metadata(name: &str) -> ArtifactMetadata {
        ArtifactMetadata {
            kind: Arc::from("test"),
            media_type: Arc::from("application/octet-stream"),
            name: Some(Arc::from(name)),
            attributes: Metadata::empty(),
        }
    }

    #[tokio::test]
    async fn exact_scope_and_reference_are_required() {
        let store = InProcessArtifactStore::default();
        let content = Bytes::from_static(b"same bytes");
        let first =
            stage_required_artifact(&store, scope("tenant-a"), content.clone(), metadata("a"))
                .await
                .expect("first");
        let second =
            stage_required_artifact(&store, scope("tenant-b"), content.clone(), metadata("b"))
                .await
                .expect("second");
        assert_eq!(first.id(), second.id(), "wire ids remain content-derived");
        assert_ne!(first.scope_digest(), second.scope_digest());
        assert_eq!(
            get_required_artifact(&store, scope("tenant-a"), first)
                .await
                .expect("first bytes"),
            content
        );
        assert!(matches!(
            store.get(scope("tenant-a"), second).await,
            Err(ArtifactError::ScopeMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn capacity_rejects_without_overwriting_existing_content() {
        let limits = ArtifactStoreLimits {
            max_artifact_bytes: 8,
            max_artifacts: 1,
            max_total_bytes: 8,
            ..ArtifactStoreLimits::default()
        };
        let store = InProcessArtifactStore::default().with_limits(limits);
        let first = stage_required_artifact(
            &store,
            scope("tenant-a"),
            Bytes::from_static(b"one"),
            metadata("one"),
        )
        .await
        .expect("first");
        assert!(matches!(
            stage_required_artifact(
                &store,
                scope("tenant-a"),
                Bytes::from_static(b"two"),
                metadata("two")
            )
            .await,
            Err(ArtifactError::CapacityExceeded { .. })
        ));
        assert_eq!(
            store
                .get(scope("tenant-a"), first)
                .await
                .expect("first retained"),
            Bytes::from_static(b"one")
        );
    }

    #[tokio::test]
    async fn garbage_collection_never_deletes_pinned_content() {
        let limits = ArtifactStoreLimits {
            orphan_grace_ms: 10,
            max_gc_batch: 8,
            ..ArtifactStoreLimits::default()
        };
        let store = InProcessArtifactStore::default().with_limits(limits);
        let artifact = stage_required_artifact(
            &store,
            scope("tenant-a"),
            Bytes::from_static(b"owned"),
            metadata("owned"),
        )
        .await
        .expect("stage");
        let owner = ArtifactOwnerId::try_new("journal:test").expect("owner");
        store
            .pin(scope("tenant-a"), artifact.clone(), owner.clone())
            .await
            .expect("pin");
        let late = Timestamp::from_unix_ms(100).expect("time");
        assert_eq!(
            store
                .collect_orphans(scope("tenant-a"), late, 8)
                .await
                .expect("gc")
                .deleted,
            0
        );
        store
            .unpin(scope("tenant-a"), artifact.clone(), owner, late)
            .await
            .expect("unpin");
        let before_grace = Timestamp::from_unix_ms(109).expect("time");
        assert_eq!(
            store
                .collect_orphans(scope("tenant-a"), before_grace, 8)
                .await
                .expect("gc")
                .deleted,
            0
        );
        let after_grace = Timestamp::from_unix_ms(110).expect("time");
        assert_eq!(
            store
                .collect_orphans(scope("tenant-a"), after_grace, 8)
                .await
                .expect("gc")
                .deleted,
            1
        );
        assert!(matches!(
            store.get(scope("tenant-a"), artifact).await,
            Err(ArtifactError::NotFound)
        ));
    }
}
