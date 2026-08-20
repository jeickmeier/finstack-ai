//! Scoped non-kernel artifact storage contract and integrity checks.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ArtifactRef, Bytes, Digest, Metadata, PortFuture, PortObject, RunId, Sensitivity, SessionId,
};

/// Stable code for an unavailable artifact service.
pub const ARTIFACT_UNAVAILABLE: &str = "artifact_unavailable";
/// Stable code for a missing required artifact.
pub const ARTIFACT_NOT_FOUND: &str = "artifact_not_found";
/// Stable code for a scope-binding failure.
pub const ARTIFACT_SCOPE_MISMATCH: &str = "artifact_scope_mismatch";
/// Stable code for a content/reference integrity failure.
pub const ARTIFACT_INTEGRITY_FAILURE: &str = "artifact_integrity_failure";
/// Stable code for rejected rather than truncated oversized content.
pub const ARTIFACT_TOO_LARGE: &str = "artifact_too_large";
/// Stable code for malformed artifact metadata.
pub const ARTIFACT_INVALID_METADATA: &str = "artifact_invalid_metadata";
/// Default individual byte-string ceiling; stores may override via `limits()`.
pub const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;

/// Exact authorization and integrity scope for an artifact operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactScope {
    /// Authenticated tenant scope, never a bearer credential.
    pub tenant_scope: Arc<str>,
    /// Owning session.
    pub session_id: SessionId,
    /// Owning run when run-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Required sensitivity classification.
    pub sensitivity: Sensitivity,
}

impl ArtifactScope {
    /// Validate and hash the exact scope binding.
    ///
    /// # Errors
    ///
    /// Returns a metadata error for empty/NUL-bearing tenant scope or failed
    /// canonicalization.
    pub fn digest(&self) -> Result<Digest, ArtifactError> {
        if self.tenant_scope.is_empty() || self.tenant_scope.as_bytes().contains(&0) {
            return Err(ArtifactError::InvalidMetadata {
                message: Arc::from("invalid_tenant_scope"),
            });
        }
        let canonical = serde_json_canonicalizer::to_vec(self).map_err(|error| {
            ArtifactError::InvalidMetadata {
                message: Arc::from(error.to_string()),
            }
        })?;
        Digest::domain_separated("artifact-scope", 1, &canonical).map_err(|error| {
            ArtifactError::InvalidMetadata {
                message: Arc::from(error.to_string()),
            }
        })
    }
}

/// Exact metadata mapped to the returned `ArtifactRef` and `BlobRef`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactMetadata {
    /// Artifact kind label.
    pub kind: Arc<str>,
    /// Blob media type.
    pub media_type: Arc<str>,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Arc<str>>,
    /// Bounded non-secret, non-authoritative attributes.
    pub attributes: Metadata,
}

/// Per-store artifact size ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactStoreLimits {
    /// Reject staged content above this size; never truncate.
    pub max_artifact_bytes: usize,
}

impl Default for ArtifactStoreLimits {
    fn default() -> Self {
        Self {
            max_artifact_bytes: MAX_ARTIFACT_BYTES,
        }
    }
}

/// Scoped application/runtime artifact service.
pub trait ArtifactStore: PortObject {
    /// Durably stage exact bytes before the referencing journal append.
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>>;

    /// Read exact bytes through an already authorized scope.
    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>>;

    /// This store's size ceilings. Defaults to the v1 4 MiB constant.
    fn limits(&self) -> ArtifactStoreLimits {
        ArtifactStoreLimits::default()
    }
}

/// Stage a required artifact and verify the exact returned reference.
///
/// Callers receive no reference to journal until durable staging has
/// completed. The journal append remains the caller's next operation, which
/// is the deliberate narrow stage-before-journal exception for artifacts.
///
/// # Errors
///
/// Rejects invalid metadata or oversized content before the service call, and
/// rejects any returned reference that rewrites scope, content, or metadata.
pub async fn stage_required_artifact(
    store: &dyn ArtifactStore,
    scope: ArtifactScope,
    content: Bytes,
    metadata: ArtifactMetadata,
) -> Result<ArtifactRef, ArtifactError> {
    let limits = store.limits();
    validate_artifact_input(&scope, &content, &metadata, &limits)?;
    let artifact = store
        .stage_put(scope.clone(), content.clone(), metadata.clone())
        .await?;
    validate_staged_artifact(&scope, &content, &metadata, &artifact, &limits)?;
    Ok(artifact)
}

/// Validate exact staging output without granting authority from metadata.
///
/// # Errors
///
/// Fails on oversize content, wrong scope, mismatched digest/length, or any
/// metadata field rewrite/drop.
pub fn validate_staged_artifact(
    scope: &ArtifactScope,
    content: &[u8],
    metadata: &ArtifactMetadata,
    artifact: &ArtifactRef,
    limits: &ArtifactStoreLimits,
) -> Result<(), ArtifactError> {
    validate_artifact_input(scope, content, metadata, limits)?;
    let expected_scope = scope.digest()?;
    if artifact.scope_digest() != expected_scope {
        return Err(ArtifactError::ScopeMismatch {
            expected: expected_scope,
            actual: artifact.scope_digest(),
        });
    }
    let expected_content = Digest::blob_content(content);
    let blob = artifact.blob();
    if artifact.content_digest() != expected_content
        || blob.digest().copied() != Some(expected_content)
        || blob.length() != u64::try_from(content.len()).unwrap_or(u64::MAX)
    {
        return Err(ArtifactError::Integrity {
            message: Arc::from("content_reference_mismatch"),
        });
    }
    if artifact.kind() != metadata.kind.as_ref()
        || blob.media_type() != metadata.media_type.as_ref()
        || blob.name() != metadata.name.as_deref()
        || artifact.metadata() != &metadata.attributes
    {
        return Err(ArtifactError::InvalidMetadata {
            message: Arc::from("metadata_mapping_mismatch"),
        });
    }
    Ok(())
}

// `limits` is taken by reference (rather than by value, despite being
// `Copy`-sized) to match the public `validate_staged_artifact` signature
// mandated by the artifact-limits design, which callers use uniformly.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn validate_artifact_input(
    scope: &ArtifactScope,
    content: &[u8],
    metadata: &ArtifactMetadata,
    limits: &ArtifactStoreLimits,
) -> Result<(), ArtifactError> {
    scope.digest()?;
    if content.len() > limits.max_artifact_bytes {
        return Err(ArtifactError::TooLarge {
            len: content.len(),
            max: limits.max_artifact_bytes,
        });
    }
    if metadata.kind.is_empty()
        || metadata.kind.as_bytes().contains(&0)
        || metadata.media_type.is_empty()
        || metadata.media_type.as_bytes().contains(&0)
        || metadata
            .name
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.as_bytes().contains(&0))
    {
        return Err(ArtifactError::InvalidMetadata {
            message: Arc::from("metadata_field_invalid"),
        });
    }
    Ok(())
}

/// Artifact service or integrity failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArtifactError {
    /// Service unavailable.
    #[error("{}: {message}", ARTIFACT_UNAVAILABLE)]
    Unavailable {
        /// Bounded diagnostic.
        message: Arc<str>,
    },
    /// Required artifact is missing.
    #[error("{}: artifact is missing", ARTIFACT_NOT_FOUND)]
    NotFound,
    /// Requested scope differs from the artifact's frozen binding.
    #[error(
        "{}: expected scope {expected}, actual scope {actual}",
        ARTIFACT_SCOPE_MISMATCH
    )]
    ScopeMismatch {
        /// Requested scope digest.
        expected: Digest,
        /// Artifact scope digest.
        actual: Digest,
    },
    /// Required content/reference integrity check failed.
    #[error("{}: {message}", ARTIFACT_INTEGRITY_FAILURE)]
    Integrity {
        /// Stable diagnostic.
        message: Arc<str>,
    },
    /// Content exceeds the v1 byte-string ceiling.
    #[error("{}: artifact has {len} bytes; maximum is {max}", ARTIFACT_TOO_LARGE)]
    TooLarge {
        /// Submitted bytes.
        len: usize,
        /// Maximum bytes.
        max: usize,
    },
    /// Metadata is malformed or was rewritten/dropped.
    #[error("{}: {message}", ARTIFACT_INVALID_METADATA)]
    InvalidMetadata {
        /// Stable diagnostic.
        message: Arc<str>,
    },
}

impl ArtifactError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => ARTIFACT_UNAVAILABLE,
            Self::NotFound => ARTIFACT_NOT_FOUND,
            Self::ScopeMismatch { .. } => ARTIFACT_SCOPE_MISMATCH,
            Self::Integrity { .. } => ARTIFACT_INTEGRITY_FAILURE,
            Self::TooLarge { .. } => ARTIFACT_TOO_LARGE,
            Self::InvalidMetadata { .. } => ARTIFACT_INVALID_METADATA,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::future::Future;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use crate::{ArtifactId, BlobRef};

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn scope() -> ArtifactScope {
        ArtifactScope {
            tenant_scope: Arc::from("tenant-a"),
            session_id: SessionId::from_bytes([1; 16]),
            run_id: Some(RunId::from_bytes([2; 16])),
            sensitivity: Sensitivity::Confidential,
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

    fn artifact(content: &[u8], scope: &ArtifactScope, metadata: &ArtifactMetadata) -> ArtifactRef {
        let digest = Digest::blob_content(content);
        let blob = BlobRef::try_new(
            "blob-1",
            metadata.media_type.as_ref(),
            u64::try_from(content.len()).expect("length"),
            Some(digest),
            metadata.name.as_deref(),
        )
        .expect("blob");
        ArtifactRef::try_new(
            ArtifactId::from_bytes([3; 16]),
            metadata.kind.as_ref(),
            blob,
            digest,
            scope.digest().expect("scope digest"),
            metadata.attributes.clone(),
        )
        .expect("artifact")
    }

    struct StoredArtifact {
        scope: ArtifactScope,
        content: Bytes,
        artifact: ArtifactRef,
    }

    struct LifecycleStore {
        next_id: Mutex<u8>,
        entries: Mutex<BTreeMap<ArtifactId, StoredArtifact>>,
        pinned: Mutex<BTreeSet<ArtifactId>>,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl LifecycleStore {
        fn new(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                next_id: Mutex::new(10),
                entries: Mutex::new(BTreeMap::new()),
                pinned: Mutex::new(BTreeSet::new()),
                log,
            }
        }

        fn pin_from_committed_reference(&self, artifact: &ArtifactRef) {
            self.pinned.lock().expect("pinned").insert(artifact.id());
        }

        fn collect_unreferenced_after_grace(&self) {
            let pinned = self.pinned.lock().expect("pinned").clone();
            self.entries
                .lock()
                .expect("entries")
                .retain(|id, _| pinned.contains(id));
        }

        fn corrupt(&self, artifact_id: ArtifactId) {
            self.entries
                .lock()
                .expect("entries")
                .get_mut(&artifact_id)
                .expect("stored artifact")
                .content = Bytes::from_static(b"corrupt");
        }
    }

    impl ArtifactStore for LifecycleStore {
        fn stage_put(
            &self,
            scope: ArtifactScope,
            content: Bytes,
            metadata: ArtifactMetadata,
        ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
            self.log.lock().expect("log").push("stage_put");
            let ordinal = {
                let mut next = self.next_id.lock().expect("next id");
                let ordinal = *next;
                *next = next.saturating_add(1);
                ordinal
            };
            let digest = Digest::blob_content(&content);
            let blob = BlobRef::try_new(
                format!("blob-{ordinal}"),
                metadata.media_type.as_ref(),
                u64::try_from(content.len()).expect("length"),
                Some(digest),
                metadata.name.as_deref(),
            )
            .expect("blob");
            let artifact = ArtifactRef::try_new(
                ArtifactId::from_bytes([ordinal; 16]),
                metadata.kind.as_ref(),
                blob,
                digest,
                scope.digest().expect("scope"),
                metadata.attributes,
            )
            .expect("artifact");
            self.entries.lock().expect("entries").insert(
                artifact.id(),
                StoredArtifact {
                    scope,
                    content,
                    artifact: artifact.clone(),
                },
            );
            Box::pin(async move { Ok(artifact) })
        }

        fn get(
            &self,
            scope: ArtifactScope,
            artifact: ArtifactRef,
        ) -> PortFuture<Result<Bytes, ArtifactError>> {
            let result = self
                .entries
                .lock()
                .expect("entries")
                .get(&artifact.id())
                .map_or_else(
                    || Err(ArtifactError::NotFound),
                    |stored| {
                        let expected = stored.scope.digest()?;
                        let submitted = scope.digest()?;
                        if expected != submitted || artifact.scope_digest() != submitted {
                            return Err(ArtifactError::ScopeMismatch {
                                expected,
                                actual: submitted,
                            });
                        }
                        if stored.artifact != artifact
                            || Digest::blob_content(&stored.content) != artifact.content_digest()
                        {
                            return Err(ArtifactError::Integrity {
                                message: Arc::from("stored_reference_mismatch"),
                            });
                        }
                        Ok(stored.content.clone())
                    },
                );
            Box::pin(async move { result })
        }
    }

    #[test]
    fn validation_preserves_exact_scope_content_and_metadata() {
        let scope = scope();
        let metadata = metadata();
        let content = b"artifact bytes";
        let artifact = artifact(content, &scope, &metadata);
        let limits = ArtifactStoreLimits::default();
        validate_staged_artifact(&scope, content, &metadata, &artifact, &limits)
            .expect("valid artifact");

        assert!(
            validate_staged_artifact(&scope, b"corrupt", &metadata, &artifact, &limits).is_err(),
            "corrupt bytes must fail"
        );
        let other_scope = ArtifactScope {
            tenant_scope: Arc::from("tenant-b"),
            ..scope.clone()
        };
        assert!(
            validate_staged_artifact(&other_scope, content, &metadata, &artifact, &limits).is_err(),
            "cross-scope read must fail"
        );
    }

    #[test]
    fn oversized_content_is_rejected_not_truncated() {
        let scope = scope();
        let metadata = metadata();
        let content = vec![0_u8; MAX_ARTIFACT_BYTES + 1];
        let artifact = artifact(&content, &scope, &metadata);
        assert!(matches!(
            validate_staged_artifact(
                &scope,
                &content,
                &metadata,
                &artifact,
                &ArtifactStoreLimits::default()
            ),
            Err(ArtifactError::TooLarge { .. })
        ));
    }

    #[test]
    fn staging_precedes_reference_and_gc_retains_only_pinned_content() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let store = LifecycleStore::new(log.clone());
        let scope = scope();
        let metadata = metadata();
        let referenced = block_on(stage_required_artifact(
            &store,
            scope.clone(),
            Bytes::from_static(b"referenced"),
            metadata.clone(),
        ))
        .expect("staged reference");
        log.lock().expect("log").push("journal_append");
        store.pin_from_committed_reference(&referenced);

        let orphan = block_on(stage_required_artifact(
            &store,
            scope.clone(),
            Bytes::from_static(b"orphaned-before-append"),
            metadata,
        ))
        .expect("staged orphan");
        assert_eq!(
            log.lock().expect("log").as_slice(),
            &["stage_put", "journal_append", "stage_put"]
        );

        store.collect_unreferenced_after_grace();
        assert_eq!(
            block_on(store.get(scope.clone(), referenced)).expect("pinned content"),
            Bytes::from_static(b"referenced")
        );
        assert!(matches!(
            block_on(store.get(scope.clone(), orphan)),
            Err(ArtifactError::NotFound)
        ));

        let other_scope = ArtifactScope {
            tenant_scope: Arc::from("tenant-b"),
            ..scope
        };
        let new_reference = block_on(stage_required_artifact(
            &store,
            other_scope.clone(),
            Bytes::from_static(b"scoped"),
            ArtifactMetadata {
                kind: Arc::from("tool-output"),
                media_type: Arc::from("application/octet-stream"),
                name: None,
                attributes: Metadata::parse(
                    br#"{"tenant_scope":"tenant-a","sensitivity":"public"}"#,
                )
                .expect("non-authoritative attributes"),
            },
        ))
        .expect("scoped artifact");
        assert!(matches!(
            block_on(store.get(
                ArtifactScope {
                    tenant_scope: Arc::from("tenant-a"),
                    ..other_scope.clone()
                },
                new_reference.clone()
            )),
            Err(ArtifactError::ScopeMismatch { .. })
        ));
        store.corrupt(new_reference.id());
        assert!(matches!(
            block_on(store.get(other_scope, new_reference)),
            Err(ArtifactError::Integrity { .. })
        ));
    }

    #[test]
    fn default_limits_match_the_v1_ceiling() {
        struct DefaultStore;
        impl ArtifactStore for DefaultStore {
            fn stage_put(
                &self,
                _: ArtifactScope,
                _: Bytes,
                _: ArtifactMetadata,
            ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
                Box::pin(async { Err(ArtifactError::NotFound) })
            }
            fn get(
                &self,
                _: ArtifactScope,
                _: ArtifactRef,
            ) -> PortFuture<Result<Bytes, ArtifactError>> {
                Box::pin(async { Err(ArtifactError::NotFound) })
            }
        }
        assert_eq!(DefaultStore.limits().max_artifact_bytes, MAX_ARTIFACT_BYTES);
    }

    #[test]
    fn staging_respects_store_limits_not_the_constant() {
        // A store that raises its ceiling accepts content above MAX_ARTIFACT_BYTES.
        let content = Bytes::from(vec![0_u8; MAX_ARTIFACT_BYTES + 1]);
        let raised = ArtifactStoreLimits {
            max_artifact_bytes: 8 * 1024 * 1024,
        };
        assert!(validate_artifact_input(&scope(), &content, &metadata(), &raised).is_ok());
        let default = ArtifactStoreLimits::default();
        assert_eq!(
            validate_artifact_input(&scope(), &content, &metadata(), &default)
                .expect_err("must reject")
                .code(),
            ARTIFACT_TOO_LARGE
        );
    }
}
