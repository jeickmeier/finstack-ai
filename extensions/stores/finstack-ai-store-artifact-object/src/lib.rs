//! `ArtifactStore` adapter over a host-supplied [`ObjectStore`].
//!
//! Bridges the application-level [`ArtifactStore`] contract onto the
//! infrastructure-level [`ObjectStore`] port so hosts can back artifacts
//! with the same object storage backend (local filesystem, S3, ...) used
//! for other unstructured content, instead of the small in-process default.
//!
//! Artifacts are stored at the logical key `artifacts/{content-digest-hex}`
//! within an [`ObjectScope`] derived from the caller's [`ArtifactScope`]
//! (`session_id` is always `Some`). The artifact `kind` and its
//! non-authoritative `attributes` do not survive as fields of the object
//! store's own [`finstack_ai_runtime::ObjectRef`], so `stage_put` stores
//! them verbatim in the object's [`ObjectMetadata::attributes`] alongside
//! the content, keeping the object record self-describing even though this
//! adapter's `get` (bound to a caller-supplied [`ArtifactRef`]) does not
//! need to read them back itself.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::Arc;

use finstack_ai_kernel::{ArtifactId, ArtifactRef, BlobRef, Digest, Metadata};
use finstack_ai_runtime::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, ArtifactStoreLimits, Bytes,
    ObjectError, ObjectKey, ObjectMetadata, ObjectScope, ObjectStore, PortFuture, PutPayload,
};
use serde::{Deserialize, Serialize};

/// Default artifact byte ceiling: 64 MiB, clamped to the backing object
/// store's [`finstack_ai_runtime::ObjectStoreLimits::max_object_bytes`].
pub const DEFAULT_MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

/// `ArtifactStore` backed by a host-supplied [`ObjectStore`].
pub struct ObjectArtifactStore {
    store: Arc<dyn ObjectStore>,
    max_artifact_bytes: usize,
}

impl ObjectArtifactStore {
    /// Construct an adapter over `store` with the default 64 MiB ceiling,
    /// clamped to the store's own [`ObjectStore::limits`].
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        let max_artifact_bytes =
            clamp_to_object_limit(DEFAULT_MAX_ARTIFACT_BYTES, store.limits().max_object_bytes);
        Self {
            store,
            max_artifact_bytes,
        }
    }

    /// Override the artifact byte ceiling, still clamped to the backing
    /// object store's [`ObjectStore::limits`].
    #[must_use]
    pub fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
        self.max_artifact_bytes =
            clamp_to_object_limit(max_artifact_bytes, self.store.limits().max_object_bytes);
        self
    }
}

fn clamp_to_object_limit(requested: usize, object_max_bytes: u64) -> usize {
    let object_max = usize::try_from(object_max_bytes).unwrap_or(usize::MAX);
    requested.min(object_max)
}

/// Everything the object round-trip does not preserve on its own, stored
/// verbatim in [`ObjectMetadata::attributes`] so `get` can rebuild the exact
/// [`ArtifactRef`] `stage_put` returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredArtifactMeta {
    kind: Arc<str>,
    attributes: Metadata,
}

fn to_object_scope(scope: &ArtifactScope) -> ObjectScope {
    ObjectScope {
        tenant_scope: Arc::clone(&scope.tenant_scope),
        session_id: Some(scope.session_id),
        run_id: scope.run_id,
        sensitivity: scope.sensitivity,
    }
}

fn artifact_key(digest: &Digest) -> Result<ObjectKey, ArtifactError> {
    ObjectKey::try_new(format!("artifacts/{}", digest.to_hex())).map_err(map_object_error)
}

fn encode_stored_meta(metadata: &ArtifactMetadata) -> Result<Metadata, ArtifactError> {
    let stored = StoredArtifactMeta {
        kind: Arc::clone(&metadata.kind),
        attributes: metadata.attributes.clone(),
    };
    let bytes = serde_json::to_vec(&stored).map_err(|error| ArtifactError::InvalidMetadata {
        message: Arc::from(error.to_string()),
    })?;
    Metadata::parse(bytes).map_err(|error| ArtifactError::InvalidMetadata {
        message: Arc::from(error.to_string()),
    })
}

/// Map an [`ObjectError`] onto the matching [`ArtifactError`] per the
/// adapter's fixed error table.
fn map_object_error(error: ObjectError) -> ArtifactError {
    match error {
        ObjectError::NotFound => ArtifactError::NotFound,
        ObjectError::TooLarge { len, max } => ArtifactError::TooLarge {
            len: usize::try_from(len).unwrap_or(usize::MAX),
            max: usize::try_from(max).unwrap_or(usize::MAX),
        },
        ObjectError::ScopeMismatch { expected, actual } => {
            ArtifactError::ScopeMismatch { expected, actual }
        }
        ObjectError::Integrity { message } => ArtifactError::Integrity { message },
        ObjectError::InvalidKey { message } | ObjectError::InvalidMetadata { message } => {
            ArtifactError::InvalidMetadata { message }
        }
        ObjectError::Unavailable { message } | ObjectError::Io { message } => {
            ArtifactError::Unavailable { message }
        }
        ObjectError::Unsupported { operation } => ArtifactError::Unavailable { message: operation },
    }
}

impl ArtifactStore for ObjectArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { stage_put_impl(store.as_ref(), scope, content, metadata).await })
    }

    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { get_impl(store.as_ref(), scope, artifact).await })
    }

    fn limits(&self) -> ArtifactStoreLimits {
        ArtifactStoreLimits {
            max_artifact_bytes: self.max_artifact_bytes,
        }
    }
}

async fn stage_put_impl(
    store: &dyn ObjectStore,
    scope: ArtifactScope,
    content: Bytes,
    metadata: ArtifactMetadata,
) -> Result<ArtifactRef, ArtifactError> {
    let digest = Digest::blob_content(&content);
    let mut artifact_id_bytes = [0_u8; 16];
    artifact_id_bytes.copy_from_slice(&digest.as_bytes()[..16]);
    let artifact_id = ArtifactId::from_bytes(artifact_id_bytes);

    let key = artifact_key(&digest)?;
    let object_attributes = encode_stored_meta(&metadata)?;
    let object_metadata = ObjectMetadata {
        media_type: Arc::clone(&metadata.media_type),
        name: metadata.name.clone(),
        attributes: object_attributes,
    };

    let object_scope = to_object_scope(&scope);
    store
        .put(
            object_scope,
            key,
            PutPayload::Bytes(content.clone()),
            object_metadata,
        )
        .await
        .map_err(map_object_error)?;

    let blob = BlobRef::try_new(
        digest.to_hex(),
        metadata.media_type.as_ref(),
        u64::try_from(content.len()).unwrap_or(0),
        Some(digest),
        metadata.name.as_deref(),
    )
    .map_err(|error| ArtifactError::InvalidMetadata {
        message: Arc::from(error.to_string()),
    })?;

    ArtifactRef::try_new(
        artifact_id,
        metadata.kind.as_ref(),
        blob,
        digest,
        scope.digest()?,
        metadata.attributes,
    )
    .map_err(|error| ArtifactError::InvalidMetadata {
        message: Arc::from(error.to_string()),
    })
}

async fn get_impl(
    store: &dyn ObjectStore,
    scope: ArtifactScope,
    artifact: ArtifactRef,
) -> Result<Bytes, ArtifactError> {
    let digest = artifact.content_digest();
    let key = artifact_key(&digest)?;
    let object_scope = to_object_scope(&scope);
    let bytes = store
        .get(object_scope, key)
        .await
        .map_err(map_object_error)?;

    if Digest::blob_content(&bytes) != digest {
        return Err(ArtifactError::Integrity {
            message: Arc::from("content_digest_mismatch"),
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
