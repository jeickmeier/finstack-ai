//! `ArtifactStore` adapter over a host-supplied [`ObjectDriver`].
//!
//! Bridges the application-level [`ArtifactStore`] contract onto the
//! infrastructure-level [`ObjectDriver`] port so hosts can back artifacts
//! with the same object storage backend (local filesystem, S3, ...) used
//! for other unstructured content, instead of the small in-process default.
//!
//! Artifacts are stored at the logical key `artifacts/v2/{reference-digest}`
//! within an [`ObjectScope`] derived from the caller's [`ArtifactScope`]
//! (`session_id` is always `Some`). A compact versioned envelope preserves
//! the complete [`ArtifactRef`] next to the bytes so reads can verify exact
//! scope, metadata, length, and content identity.
//!
//! # Ownership sidecar and blob index
//!
//! Two small objects live beside each envelope, outside the `artifacts/v2/`
//! listing prefix so garbage collection never mistakes them for artifacts:
//!
//! - `artifacts/owners/v1/{reference-digest}`: the [`OwnerSidecar`]. Once it
//!   exists it is the only authority for owners and the unreferenced clock;
//!   the `owners`/`unreferenced_since` fields still present in the envelope
//!   header are then stale but harmless. It is seeded lazily from the header
//!   the first time an envelope written before the sidecar existed is
//!   pinned, unpinned, or examined by GC, so no migration pass is needed and
//!   envelope bytes are never rewritten.
//! - `artifacts/by-blob/v1/{blob-digest}`: the [`BlobIndex`], mapping the
//!   digest of the canonical [`BlobRef`] to the exact [`ArtifactRef`] so
//!   [`ArtifactStore::get_by_blob`] is one small read plus one envelope read.
//!   Envelopes staged before the index existed are found by the bounded
//!   listing scan, which writes the index on a hit.
//!
//! GC commits to a deletion by marking the sidecar `collecting_since` under
//! a digest precondition before touching the envelope; a pin racing that
//! decision either wins the precondition or observes the mark and fails
//! closed. A mark left behind by an interrupted pass is reset after one
//! orphan grace, and a fresh `stage_put` clears it, so no crash prefix can
//! strand or resurrect an artifact.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::driver::{ObjectDriver, ObjectError, ObjectKey, ObjectMetadata, ObjectScope, PageToken};
use finstack_ai_kernel::{ArtifactRef, BlobRef, Digest, Metadata, Timestamp};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactPersistence,
    ArtifactRead, ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
    artifact_storage_key, build_artifact_ref, validate_artifact_scope, validate_retrieved_artifact,
};
use finstack_ai_runtime::ports::PortFuture;
use serde::{Deserialize, Serialize};

use crate::DEFAULT_MAX_ARTIFACT_BYTES;

const ENVELOPE_MAGIC: &[u8; 8] = b"FSAIART2";
const ENVELOPE_HEADER_LEN_BYTES: usize = 4;
const MAX_CONDITIONAL_RETRIES: usize = 8;
const ENVELOPE_PREFIX: &str = "artifacts/v2";
const OWNERS_PREFIX: &str = "artifacts/owners/v1";
const BY_BLOB_PREFIX: &str = "artifacts/by-blob/v1";
const SIDECAR_SCHEMA: u32 = 1;
const ENVELOPE_MEDIA_TYPE: &str = "application/vnd.finstack.artifact-v2";
const OWNERS_MEDIA_TYPE: &str = "application/vnd.finstack.artifact-owners-v1";
const BY_BLOB_MEDIA_TYPE: &str = "application/vnd.finstack.artifact-by-blob-v1";

/// Bytes added on top of the artifact ceiling when clamping a driver's
/// single-object limit: the envelope magic, length prefix, and a canonical
/// JSON header bounded by the kernel's metadata and label limits plus up to
/// `MAX_ARTIFACT_OWNERS` legacy owner ids.
#[cfg(feature = "s3")]
pub(crate) const ENVELOPE_HEADER_SLACK_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArtifactEnvelopeHeader {
    pub(crate) artifact: ArtifactRef,
    pub(crate) owners: BTreeSet<ArtifactOwnerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unreferenced_since: Option<Timestamp>,
}

/// Authoritative lifecycle state for one envelope, stored at
/// `artifacts/owners/v1/{reference-digest}` as canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerSidecar {
    /// Always [`SIDECAR_SCHEMA`].
    pub(crate) schema: u32,
    /// Owners currently keeping the artifact reachable.
    pub(crate) owners: BTreeSet<ArtifactOwnerId>,
    /// Unix milliseconds when the owner set became empty, if it is.
    pub(crate) unreferenced_since: Option<i64>,
    /// Digest of the envelope bytes this sidecar describes.
    pub(crate) envelope_digest: Digest,
    /// Unix milliseconds when GC committed to deleting the envelope; a
    /// tombstone that fails every pin until the deletion completes or the
    /// mark expires.
    #[serde(default)]
    pub(crate) collecting_since: Option<i64>,
}

/// Blob-to-artifact index entry stored at `artifacts/by-blob/v1/{digest}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlobIndex {
    /// Always [`SIDECAR_SCHEMA`].
    pub(crate) schema: u32,
    /// The exact artifact whose blob hashes to the key.
    pub(crate) artifact: ArtifactRef,
}

/// A sidecar together with the digest of the stored bytes it was decoded
/// from, which every conditional write uses as its precondition.
struct LoadedSidecar {
    sidecar: OwnerSidecar,
    digest: Digest,
}

/// `ArtifactStore` backed by a host-supplied [`ObjectDriver`].
pub(crate) struct ObjectArtifactStore {
    store: Arc<dyn ObjectDriver>,
    max_artifact_bytes: usize,
}

impl ObjectArtifactStore {
    /// Construct an adapter over `store` with the default 64 MiB ceiling,
    /// clamped to the store's own [`ObjectDriver::limits`].
    #[must_use]
    pub(crate) fn new(store: Arc<dyn ObjectDriver>) -> Self {
        let max_artifact_bytes =
            clamp_to_object_limit(DEFAULT_MAX_ARTIFACT_BYTES, store.limits().max_object_bytes);
        Self {
            store,
            max_artifact_bytes,
        }
    }

    /// Override the artifact byte ceiling, still clamped to the backing
    /// object store's [`ObjectDriver::limits`].
    #[cfg(any(feature = "s3", feature = "local"))]
    #[must_use]
    pub(crate) fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
        self.max_artifact_bytes =
            clamp_to_object_limit(max_artifact_bytes, self.store.limits().max_object_bytes);
        self
    }

    /// The backing driver's ceilings, for clamp assertions.
    #[cfg(all(test, feature = "s3"))]
    pub(crate) fn object_limits(&self) -> crate::driver::ObjectStoreLimits {
        self.store.limits()
    }
}

fn clamp_to_object_limit(requested: usize, object_max_bytes: u64) -> usize {
    let object_max = usize::try_from(object_max_bytes).unwrap_or(usize::MAX);
    requested.min(object_max)
}

/// Single-object ceiling a driver needs for artifacts up to
/// `max_artifact_bytes`: the artifact ceiling plus header slack, never above
/// the driver's own `configured` limit.
#[cfg(feature = "s3")]
#[must_use]
pub(crate) fn clamp_object_ceiling(configured: u64, max_artifact_bytes: usize) -> u64 {
    let ceiling = u64::try_from(max_artifact_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(ENVELOPE_HEADER_SLACK_BYTES);
    configured.min(ceiling)
}

pub(crate) fn to_object_scope(scope: &ArtifactScope) -> ObjectScope {
    ObjectScope {
        tenant_scope: Arc::clone(&scope.tenant_scope),
        session_id: Some(scope.session_id),
        run_id: scope.run_id,
        sensitivity: scope.sensitivity,
    }
}

pub(crate) fn artifact_key(storage_key: &Digest) -> Result<ObjectKey, ArtifactError> {
    ObjectKey::try_new(format!("{ENVELOPE_PREFIX}/{}", storage_key.to_hex()))
        .map_err(map_object_error)
}

pub(crate) fn owners_key(storage_key: &Digest) -> Result<ObjectKey, ArtifactError> {
    ObjectKey::try_new(format!("{OWNERS_PREFIX}/{}", storage_key.to_hex()))
        .map_err(map_object_error)
}

/// Index key for one exact blob: the domain-separated digest of its
/// canonical JSON, so blobs that share bytes but differ in name or media
/// type get distinct entries, matching [`ArtifactStore::get_by_blob`]'s
/// exact-equality contract.
pub(crate) fn by_blob_key(blob: &BlobRef) -> Result<ObjectKey, ArtifactError> {
    let invalid = || ArtifactError::InvalidMetadata {
        message: Arc::from("artifact_reference_invalid"),
    };
    let canonical = serde_json_canonicalizer::to_vec(blob).map_err(|_| invalid())?;
    let digest =
        Digest::domain_separated("artifact-by-blob", 1, &canonical).map_err(|_| invalid())?;
    ObjectKey::try_new(format!("{BY_BLOB_PREFIX}/{}", digest.to_hex())).map_err(map_object_error)
}

pub(crate) fn encode_envelope(
    header: &ArtifactEnvelopeHeader,
    content: &[u8],
) -> Result<Bytes, ArtifactError> {
    let header =
        serde_json_canonicalizer::to_vec(header).map_err(|_| ArtifactError::InvalidMetadata {
            message: Arc::from("artifact_reference_invalid"),
        })?;
    let header_len = u32::try_from(header.len()).map_err(|_| ArtifactError::InvalidMetadata {
        message: Arc::from("artifact_reference_too_large"),
    })?;
    let capacity = ENVELOPE_MAGIC
        .len()
        .checked_add(ENVELOPE_HEADER_LEN_BYTES)
        .and_then(|value| value.checked_add(header.len()))
        .and_then(|value| value.checked_add(content.len()))
        .ok_or(ArtifactError::InvalidMetadata {
            message: Arc::from("artifact_envelope_too_large"),
        })?;
    let mut envelope = Vec::with_capacity(capacity);
    envelope.extend_from_slice(ENVELOPE_MAGIC);
    envelope.extend_from_slice(&header_len.to_be_bytes());
    envelope.extend_from_slice(&header);
    envelope.extend_from_slice(content);
    Ok(Bytes::from(envelope))
}

pub(crate) fn decode_envelope(
    envelope: &[u8],
) -> Result<(ArtifactEnvelopeHeader, Bytes), ArtifactError> {
    let header_start = ENVELOPE_MAGIC.len() + ENVELOPE_HEADER_LEN_BYTES;
    if envelope.len() < header_start || envelope.get(..ENVELOPE_MAGIC.len()) != Some(ENVELOPE_MAGIC)
    {
        return Err(ArtifactError::Integrity {
            message: Arc::from("artifact_envelope_invalid"),
        });
    }
    let length_bytes: [u8; ENVELOPE_HEADER_LEN_BYTES] = envelope
        .get(ENVELOPE_MAGIC.len()..header_start)
        .and_then(|value| value.try_into().ok())
        .ok_or(ArtifactError::Integrity {
            message: Arc::from("artifact_envelope_invalid"),
        })?;
    let header_len = usize::try_from(u32::from_be_bytes(length_bytes)).map_err(|_| {
        ArtifactError::Integrity {
            message: Arc::from("artifact_envelope_invalid"),
        }
    })?;
    let content_start = header_start
        .checked_add(header_len)
        .filter(|value| *value <= envelope.len())
        .ok_or(ArtifactError::Integrity {
            message: Arc::from("artifact_envelope_invalid"),
        })?;
    let header = serde_json::from_slice(envelope.get(header_start..content_start).ok_or(
        ArtifactError::Integrity {
            message: Arc::from("artifact_envelope_invalid"),
        },
    )?)
    .map_err(|_| ArtifactError::Integrity {
        message: Arc::from("artifact_reference_invalid"),
    })?;
    let content = envelope
        .get(content_start..)
        .ok_or(ArtifactError::Integrity {
            message: Arc::from("artifact_envelope_invalid"),
        })?;
    Ok((header, Bytes::copy_from_slice(content)))
}

pub(crate) fn encode_sidecar(sidecar: &OwnerSidecar) -> Result<Bytes, ArtifactError> {
    serde_json_canonicalizer::to_vec(sidecar)
        .map(Bytes::from)
        .map_err(|_| ArtifactError::InvalidMetadata {
            message: Arc::from("artifact_sidecar_invalid"),
        })
}

pub(crate) fn decode_sidecar(bytes: &[u8]) -> Result<OwnerSidecar, ArtifactError> {
    let sidecar: OwnerSidecar =
        serde_json::from_slice(bytes).map_err(|_| ArtifactError::Integrity {
            message: Arc::from("artifact_sidecar_invalid"),
        })?;
    if sidecar.schema != SIDECAR_SCHEMA {
        return Err(ArtifactError::Integrity {
            message: Arc::from("artifact_sidecar_schema_unsupported"),
        });
    }
    Ok(sidecar)
}

pub(crate) fn encode_blob_index(artifact: &ArtifactRef) -> Result<Bytes, ArtifactError> {
    serde_json_canonicalizer::to_vec(&BlobIndex {
        schema: SIDECAR_SCHEMA,
        artifact: artifact.clone(),
    })
    .map(Bytes::from)
    .map_err(|_| ArtifactError::InvalidMetadata {
        message: Arc::from("artifact_reference_invalid"),
    })
}

pub(crate) fn decode_blob_index(bytes: &[u8]) -> Result<BlobIndex, ArtifactError> {
    let index: BlobIndex = serde_json::from_slice(bytes).map_err(|_| ArtifactError::Integrity {
        message: Arc::from("artifact_blob_index_invalid"),
    })?;
    if index.schema != SIDECAR_SCHEMA {
        return Err(ArtifactError::Integrity {
            message: Arc::from("artifact_blob_index_schema_unsupported"),
        });
    }
    Ok(index)
}

/// Map an [`ObjectError`] onto the matching [`ArtifactError`] per the
/// adapter's fixed error table.
pub(crate) fn map_object_error(error: ObjectError) -> ArtifactError {
    match error {
        ObjectError::NotFound => ArtifactError::NotFound,
        ObjectError::Conflict => ArtifactError::Unavailable {
            message: Arc::from("artifact_concurrent_mutation"),
        },
        ObjectError::Busy => ArtifactError::Unavailable {
            message: Arc::from("artifact_busy"),
        },
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

fn concurrent_mutation() -> ArtifactError {
    ArtifactError::Unavailable {
        message: Arc::from("artifact_concurrent_mutation"),
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
        let limits = self.limits();
        Box::pin(
            async move { stage_put_impl(store.as_ref(), scope, content, metadata, limits).await },
        )
    }

    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { get_impl(store.as_ref(), scope, artifact).await })
    }

    fn get_by_blob(
        &self,
        scope: ArtifactScope,
        blob: BlobRef,
    ) -> PortFuture<Result<ArtifactRead, ArtifactError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { get_by_blob_impl(store.as_ref(), scope, blob).await })
    }

    fn limits(&self) -> ArtifactStoreLimits {
        ArtifactStoreLimits {
            max_artifact_bytes: self.max_artifact_bytes,
            // Aggregate capacity belongs to the external object service and
            // is not knowable or atomically enforceable through ObjectDriver.
            // Do not advertise the bounded in-process defaults as guarantees.
            max_artifacts: usize::MAX,
            max_total_bytes: u64::MAX,
            ..ArtifactStoreLimits::default()
        }
    }

    fn descriptor(&self) -> ArtifactStoreDescriptor {
        ArtifactStoreDescriptor {
            store_id: Arc::from("object.artifacts-v2"),
            persistence: ArtifactPersistence::Durable,
            limits: self.limits(),
        }
    }

    fn pin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let store = Arc::clone(&self.store);
        let limits = self.limits();
        Box::pin(async move { pin_impl(store.as_ref(), scope, artifact, owner, limits).await })
    }

    fn unpin(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
        owner: ArtifactOwnerId,
        now: Timestamp,
    ) -> PortFuture<Result<(), ArtifactError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { unpin_impl(store.as_ref(), scope, artifact, owner, now).await })
    }

    fn collect_orphans(
        &self,
        scope: ArtifactScope,
        now: Timestamp,
        limit: usize,
    ) -> PortFuture<Result<ArtifactGcReport, ArtifactError>> {
        let store = Arc::clone(&self.store);
        let limits = self.limits();
        Box::pin(async move {
            collect_orphans_impl(
                store.as_ref(),
                scope,
                now,
                limit.min(limits.max_gc_batch),
                limits,
            )
            .await
        })
    }
}

fn object_metadata(name: Option<Arc<str>>) -> ObjectMetadata {
    ObjectMetadata {
        media_type: Arc::from(ENVELOPE_MEDIA_TYPE),
        name,
        attributes: Metadata::empty(),
    }
}

fn small_object_metadata(media_type: &'static str) -> ObjectMetadata {
    ObjectMetadata {
        media_type: Arc::from(media_type),
        name: None,
        attributes: Metadata::empty(),
    }
}

fn validate_object_ref(
    object_ref: &crate::driver::ObjectRef,
    object_scope: &ObjectScope,
    key: &ObjectKey,
    envelope: &[u8],
) -> Result<(), ArtifactError> {
    let expected_scope = object_scope.digest().map_err(map_object_error)?;
    let expected_digest = Digest::blob_content(envelope);
    let expected_length = u64::try_from(envelope.len()).unwrap_or(u64::MAX);
    if object_ref.scope_digest != expected_scope {
        return Err(ArtifactError::ScopeMismatch {
            expected: expected_scope,
            actual: object_ref.scope_digest,
        });
    }
    if object_ref.key != *key
        || object_ref.content_digest != expected_digest
        || object_ref.length != expected_length
        || object_ref.media_type.as_ref() != ENVELOPE_MEDIA_TYPE
    {
        return Err(ArtifactError::Integrity {
            message: Arc::from("object_reference_mismatch"),
        });
    }
    Ok(())
}

async fn stage_put_impl(
    store: &dyn ObjectDriver,
    scope: ArtifactScope,
    content: Bytes,
    metadata: ArtifactMetadata,
    limits: ArtifactStoreLimits,
) -> Result<ArtifactRef, ArtifactError> {
    let artifact = build_artifact_ref(&scope, &content, &metadata, &limits)?;
    let storage_key = artifact_storage_key(&scope, &artifact)?;
    let key = artifact_key(&storage_key)?;
    let header = ArtifactEnvelopeHeader {
        artifact: artifact.clone(),
        owners: BTreeSet::new(),
        unreferenced_since: None,
    };
    let envelope = encode_envelope(&header, &content)?;
    let object_metadata = object_metadata(metadata.name.clone());

    let object_scope = to_object_scope(&scope);
    let result = store
        .put_if_absent(
            object_scope.clone(),
            key.clone(),
            envelope.clone(),
            object_metadata,
        )
        .await;
    match result {
        Ok(object_ref) => {
            validate_object_ref(&object_ref, &object_scope, &key, &envelope)?;
            // The envelope was absent a moment ago, so any GC tombstone at
            // this key belongs to a collected predecessor with identical
            // bytes and must not fail the pins that follow.
            clear_stale_tombstone(store, &object_scope, &owners_key(&storage_key)?).await?;
        }
        Err(ObjectError::Conflict) => {
            let (_, stored_header, stored_content) =
                load_envelope(store, &scope, &artifact).await?;
            if stored_header.artifact != artifact || stored_content != content {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("artifact_identity_collision"),
                });
            }
        }
        Err(error) => return Err(map_object_error(error)),
    }
    write_blob_index(store, &object_scope, &artifact).await?;
    Ok(artifact)
}

/// Remove a GC tombstone left at `key` by an interrupted collection of a
/// previous envelope generation. Only a tombstone is removed, and only the
/// exact bytes observed: a live sidecar seeded by a concurrent pin is
/// never touched.
async fn clear_stale_tombstone(
    store: &dyn ObjectDriver,
    object_scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<(), ArtifactError> {
    let Some(loaded) = read_sidecar(store, object_scope, key).await? else {
        return Ok(());
    };
    if loaded.sidecar.collecting_since.is_none() {
        return Ok(());
    }
    match store
        .delete_if_digest(object_scope.clone(), key.clone(), loaded.digest)
        .await
    {
        Ok(()) | Err(ObjectError::Conflict | ObjectError::NotFound) => Ok(()),
        Err(error) => Err(map_object_error(error)),
    }
}

/// Publish the blob index entry for `artifact`, tolerating an entry that
/// already names an artifact with the same exact blob.
async fn write_blob_index(
    store: &dyn ObjectDriver,
    object_scope: &ObjectScope,
    artifact: &ArtifactRef,
) -> Result<(), ArtifactError> {
    let key = by_blob_key(artifact.blob())?;
    let bytes = encode_blob_index(artifact)?;
    match store
        .put_if_absent(
            object_scope.clone(),
            key.clone(),
            bytes,
            small_object_metadata(BY_BLOB_MEDIA_TYPE),
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(ObjectError::Conflict) => {
            let existing = match store.get(object_scope.clone(), key).await {
                Ok(existing) => existing,
                // Removed between the conflict and the read; a lookup
                // recreates it from the listing scan.
                Err(ObjectError::NotFound) => return Ok(()),
                Err(error) => return Err(map_object_error(error)),
            };
            let index = decode_blob_index(&existing)?;
            if index.artifact.blob() != artifact.blob() {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("artifact_identity_collision"),
                });
            }
            Ok(())
        }
        Err(error) => Err(map_object_error(error)),
    }
}

async fn load_envelope(
    store: &dyn ObjectDriver,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
) -> Result<(Bytes, ArtifactEnvelopeHeader, Bytes), ArtifactError> {
    validate_artifact_scope(scope, artifact)?;
    let key = artifact_key(&artifact_storage_key(scope, artifact)?)?;
    let envelope = store
        .get(to_object_scope(scope), key)
        .await
        .map_err(map_object_error)?;
    let (header, content) = decode_envelope(&envelope)?;
    if header.artifact != *artifact {
        return Err(ArtifactError::Integrity {
            message: Arc::from("stored_reference_mismatch"),
        });
    }
    validate_retrieved_artifact(scope, artifact, &content)?;
    Ok((envelope, header, content))
}

async fn get_impl(
    store: &dyn ObjectDriver,
    scope: ArtifactScope,
    artifact: ArtifactRef,
) -> Result<Bytes, ArtifactError> {
    let (_, _, content) = load_envelope(store, &scope, &artifact).await?;
    Ok(content)
}

async fn get_by_blob_impl(
    store: &dyn ObjectDriver,
    scope: ArtifactScope,
    blob: BlobRef,
) -> Result<ArtifactRead, ArtifactError> {
    if blob.digest().is_none() {
        return Err(ArtifactError::InvalidMetadata {
            message: Arc::from("artifact_digest_required"),
        });
    }
    scope.digest()?;
    let object_scope = to_object_scope(&scope);
    let index_key = by_blob_key(&blob)?;
    match store.get(object_scope.clone(), index_key.clone()).await {
        Ok(bytes) => {
            let index = decode_blob_index(&bytes)?;
            if index.artifact.blob() != &blob {
                return Err(ArtifactError::Integrity {
                    message: Arc::from("artifact_blob_index_mismatch"),
                });
            }
            match load_envelope(store, &scope, &index.artifact).await {
                Ok((_, header, content)) => {
                    return Ok(ArtifactRead {
                        reference: header.artifact,
                        content,
                    });
                }
                // The envelope is gone (collected, or a crash between the
                // envelope and index deletions): drop the dangling entry
                // and fall back to the scan.
                Err(ArtifactError::NotFound) => {
                    match store
                        .delete_if_digest(
                            object_scope.clone(),
                            index_key,
                            Digest::blob_content(&bytes),
                        )
                        .await
                    {
                        Ok(()) | Err(ObjectError::Conflict | ObjectError::NotFound) => {}
                        Err(error) => return Err(map_object_error(error)),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(ObjectError::NotFound) => {}
        Err(error) => return Err(map_object_error(error)),
    }

    // Envelopes staged before the index existed: bounded listing scan,
    // indexing the hit so the next lookup is direct.
    let prefix = ObjectKey::try_new(ENVELOPE_PREFIX).map_err(map_object_error)?;
    let mut page = PageToken::first();
    loop {
        let listed = store
            .list(object_scope.clone(), Some(prefix.clone()), page)
            .await
            .map_err(map_object_error)?;
        for entry in listed.entries {
            let Some((_, header, content)) =
                listed_envelope(store, &scope, &object_scope, &entry.key).await?
            else {
                continue;
            };
            if header.artifact.blob() == &blob {
                write_blob_index(store, &object_scope, &header.artifact).await?;
                return Ok(ArtifactRead {
                    reference: header.artifact,
                    content,
                });
            }
        }
        let Some(next) = listed.next else {
            return Err(ArtifactError::NotFound);
        };
        page = next;
    }
}

async fn pin_impl(
    store: &dyn ObjectDriver,
    scope: ArtifactScope,
    artifact: ArtifactRef,
    owner: ArtifactOwnerId,
    limits: ArtifactStoreLimits,
) -> Result<(), ArtifactError> {
    update_sidecar(store, &scope, &artifact, |sidecar| {
        if sidecar.owners.contains(&owner) {
            return Ok(false);
        }
        if sidecar.owners.len() >= limits.max_owners_per_artifact {
            return Err(ArtifactError::CapacityExceeded {
                resource: "owners",
                limit: u64::try_from(limits.max_owners_per_artifact).unwrap_or(u64::MAX),
            });
        }
        sidecar.owners.insert(owner.clone());
        sidecar.unreferenced_since = None;
        Ok(true)
    })
    .await
}

async fn unpin_impl(
    store: &dyn ObjectDriver,
    scope: ArtifactScope,
    artifact: ArtifactRef,
    owner: ArtifactOwnerId,
    now: Timestamp,
) -> Result<(), ArtifactError> {
    update_sidecar(store, &scope, &artifact, |sidecar| {
        if !sidecar.owners.remove(&owner) {
            return Ok(false);
        }
        if sidecar.owners.is_empty() {
            sidecar.unreferenced_since = Some(now.as_unix_ms());
        }
        Ok(true)
    })
    .await
}

/// Read the sidecar at `key`, or `None` when it does not exist.
async fn read_sidecar(
    store: &dyn ObjectDriver,
    object_scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<Option<LoadedSidecar>, ArtifactError> {
    match store.get(object_scope.clone(), key.clone()).await {
        Ok(bytes) => Ok(Some(LoadedSidecar {
            sidecar: decode_sidecar(&bytes)?,
            digest: Digest::blob_content(&bytes),
        })),
        Err(ObjectError::NotFound) => Ok(None),
        Err(error) => Err(map_object_error(error)),
    }
}

/// Seed the sidecar at `key` from an envelope's header: its owners, its
/// unreferenced clock, and the digest of its exact bytes. Loses the race to
/// a concurrent seed gracefully by reading what landed.
async fn seed_sidecar(
    store: &dyn ObjectDriver,
    object_scope: &ObjectScope,
    key: &ObjectKey,
    envelope: &[u8],
    header: &ArtifactEnvelopeHeader,
) -> Result<LoadedSidecar, ArtifactError> {
    let seed = OwnerSidecar {
        schema: SIDECAR_SCHEMA,
        owners: header.owners.clone(),
        unreferenced_since: header.unreferenced_since.map(Timestamp::as_unix_ms),
        envelope_digest: Digest::blob_content(envelope),
        collecting_since: None,
    };
    let bytes = encode_sidecar(&seed)?;
    match store
        .put_if_absent(
            object_scope.clone(),
            key.clone(),
            bytes.clone(),
            small_object_metadata(OWNERS_MEDIA_TYPE),
        )
        .await
    {
        Ok(_) => Ok(LoadedSidecar {
            sidecar: seed,
            digest: Digest::blob_content(&bytes),
        }),
        Err(ObjectError::Conflict) => read_sidecar(store, object_scope, key)
            .await?
            .ok_or_else(concurrent_mutation),
        Err(error) => Err(map_object_error(error)),
    }
}

/// Load the authoritative sidecar for `artifact`, seeding it from the
/// envelope header on first touch.
async fn load_owner_sidecar(
    store: &dyn ObjectDriver,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
) -> Result<LoadedSidecar, ArtifactError> {
    validate_artifact_scope(scope, artifact)?;
    let storage_key = artifact_storage_key(scope, artifact)?;
    let key = owners_key(&storage_key)?;
    let object_scope = to_object_scope(scope);
    if let Some(loaded) = read_sidecar(store, &object_scope, &key).await? {
        // Authoritative for lifecycle, not for existence: confirm the
        // envelope is still there without downloading it, so an owner is
        // never recorded against an artifact removed out of band.
        return match store
            .head(object_scope.clone(), artifact_key(&storage_key)?)
            .await
        {
            Ok(_) => Ok(loaded),
            Err(ObjectError::NotFound) => {
                match store
                    .delete_if_digest(object_scope, key, loaded.digest)
                    .await
                {
                    Ok(()) | Err(ObjectError::Conflict | ObjectError::NotFound) => {}
                    Err(error) => return Err(map_object_error(error)),
                }
                Err(ArtifactError::NotFound)
            }
            Err(error) => Err(map_object_error(error)),
        };
    }
    let (envelope, header, _) = load_envelope(store, scope, artifact).await?;
    seed_sidecar(store, &object_scope, &key, &envelope, &header).await
}

/// Apply `update` to the sidecar and write it back under a digest
/// precondition, retrying from a fresh read on every conflict. The envelope
/// is never rewritten.
///
/// `update` returns `Ok(false)` when the sidecar already has the desired
/// shape, in which case nothing is written. A sidecar marked for collection
/// fails closed with [`ArtifactError::NotFound`].
async fn update_sidecar(
    store: &dyn ObjectDriver,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
    mut update: impl FnMut(&mut OwnerSidecar) -> Result<bool, ArtifactError>,
) -> Result<(), ArtifactError> {
    let key = owners_key(&artifact_storage_key(scope, artifact)?)?;
    let object_scope = to_object_scope(scope);
    for _ in 0..MAX_CONDITIONAL_RETRIES {
        let LoadedSidecar {
            mut sidecar,
            digest,
        } = load_owner_sidecar(store, scope, artifact).await?;
        if sidecar.collecting_since.is_some() {
            return Err(ArtifactError::NotFound);
        }
        if !update(&mut sidecar)? {
            return Ok(());
        }
        let replacement = encode_sidecar(&sidecar)?;
        match store
            .replace_if_digest(
                object_scope.clone(),
                key.clone(),
                digest,
                replacement,
                small_object_metadata(OWNERS_MEDIA_TYPE),
            )
            .await
        {
            Ok(_) => return Ok(()),
            // NotFound: GC removed the sidecar after the read; the retry
            // re-seeds it, which fails honestly if the envelope is gone.
            Err(ObjectError::Conflict | ObjectError::NotFound) => {}
            Err(error) => return Err(map_object_error(error)),
        }
    }
    Err(concurrent_mutation())
}

/// Fetch and validate one listed artifact envelope, or `None` when it was
/// deleted between the listing and the read.
async fn listed_envelope(
    store: &dyn ObjectDriver,
    scope: &ArtifactScope,
    object_scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<Option<(Bytes, ArtifactEnvelopeHeader, Bytes)>, ArtifactError> {
    let envelope = match store.get(object_scope.clone(), key.clone()).await {
        Ok(value) => value,
        Err(ObjectError::NotFound) => return Ok(None),
        Err(error) => return Err(map_object_error(error)),
    };
    let (header, content) = decode_envelope(&envelope)?;
    validate_artifact_scope(scope, &header.artifact)?;
    let expected_key = artifact_key(&artifact_storage_key(scope, &header.artifact)?)?;
    if expected_key != *key {
        return Err(ArtifactError::Integrity {
            message: Arc::from("stored_reference_mismatch"),
        });
    }
    validate_retrieved_artifact(scope, &header.artifact, &content)?;
    Ok(Some((envelope, header, content)))
}

/// Durable scan hint, outside the artifact prefix. Losing a concurrent cursor
/// update can repeat work but cannot authorize deletion or bypass the grace.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GcCursor {
    page: PageToken,
    offset: usize,
}

impl Default for GcCursor {
    fn default() -> Self {
        Self {
            page: PageToken::first(),
            offset: 0,
        }
    }
}

async fn load_gc_cursor(
    store: &dyn ObjectDriver,
    scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<GcCursor, ArtifactError> {
    match store.get(scope.clone(), key.clone()).await {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| ArtifactError::Integrity {
            message: Arc::from("artifact_gc_cursor_invalid"),
        }),
        Err(ObjectError::NotFound) => Ok(GcCursor::default()),
        Err(error) => Err(map_object_error(error)),
    }
}

/// Outcome of examining one listed envelope during GC.
enum GcOutcome {
    /// Retained, skipped, or merely stamped.
    Kept,
    /// Envelope deleted; carries the content bytes reclaimed.
    Deleted(u64),
}

/// Sidecar key for a listed envelope key, or `None` for a key that is not
/// an envelope (nothing else should list under the prefix, so this fails
/// closed by skipping rather than deleting).
fn sidecar_key_for_envelope(entry_key: &ObjectKey) -> Result<Option<ObjectKey>, ArtifactError> {
    let Some(hex) = entry_key
        .as_str()
        .strip_prefix(ENVELOPE_PREFIX)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return Ok(None);
    };
    let Ok(storage_key) = Digest::from_hex(hex) else {
        return Ok(None);
    };
    owners_key(&storage_key).map(Some)
}

/// Current sidecar for a listed envelope, seeding it from the envelope when
/// absent; `None` when the envelope vanished before it could be seeded.
async fn gc_sidecar(
    store: &dyn ObjectDriver,
    scope: &ArtifactScope,
    object_scope: &ObjectScope,
    entry_key: &ObjectKey,
    sidecar_key: &ObjectKey,
) -> Result<Option<LoadedSidecar>, ArtifactError> {
    if let Some(loaded) = read_sidecar(store, object_scope, sidecar_key).await? {
        return Ok(Some(loaded));
    }
    let Some((envelope, header, _)) =
        listed_envelope(store, scope, object_scope, entry_key).await?
    else {
        return Ok(None);
    };
    seed_sidecar(store, object_scope, sidecar_key, &envelope, &header)
        .await
        .map(Some)
}

/// Write `replacement` over the sidecar whose bytes hashed to `expected`.
/// Returns whether the write landed; a lost race is not an error.
async fn replace_sidecar(
    store: &dyn ObjectDriver,
    object_scope: &ObjectScope,
    key: &ObjectKey,
    expected: Digest,
    replacement: Bytes,
) -> Result<bool, ArtifactError> {
    match store
        .replace_if_digest(
            object_scope.clone(),
            key.clone(),
            expected,
            replacement,
            small_object_metadata(OWNERS_MEDIA_TYPE),
        )
        .await
    {
        Ok(_) => Ok(true),
        Err(ObjectError::Conflict | ObjectError::NotFound) => Ok(false),
        Err(error) => Err(map_object_error(error)),
    }
}

fn grace_elapsed(now: Timestamp, since: i64, grace_ms: u64) -> bool {
    now.as_unix_ms()
        .checked_sub(since)
        .and_then(|value| u64::try_from(value).ok())
        .is_some_and(|elapsed| elapsed >= grace_ms)
}

/// Examine one listed envelope: seed or read its sidecar, stamp the
/// unreferenced clock, and once the grace has elapsed commit to deletion by
/// tombstoning the sidecar before removing the envelope, the sidecar, and
/// the blob index in that order.
async fn gc_examine(
    store: &dyn ObjectDriver,
    scope: &ArtifactScope,
    object_scope: &ObjectScope,
    entry_key: ObjectKey,
    now: Timestamp,
    limits: &ArtifactStoreLimits,
) -> Result<GcOutcome, ArtifactError> {
    let Some(sidecar_key) = sidecar_key_for_envelope(&entry_key)? else {
        return Ok(GcOutcome::Kept);
    };
    let Some(loaded) = gc_sidecar(store, scope, object_scope, &entry_key, &sidecar_key).await?
    else {
        return Ok(GcOutcome::Kept);
    };
    let Some(tombstone) = gc_commit(store, object_scope, &sidecar_key, loaded, now, limits).await?
    else {
        return Ok(GcOutcome::Kept);
    };
    gc_delete(store, object_scope, entry_key, sidecar_key, tombstone, now).await
}

/// Decide whether the envelope behind `loaded` is collectible now and, if
/// so, commit by writing the tombstone. Returns the tombstone that landed.
async fn gc_commit(
    store: &dyn ObjectDriver,
    object_scope: &ObjectScope,
    sidecar_key: &ObjectKey,
    loaded: LoadedSidecar,
    now: Timestamp,
    limits: &ArtifactStoreLimits,
) -> Result<Option<LoadedSidecar>, ArtifactError> {
    let LoadedSidecar { sidecar, digest } = loaded;
    if let Some(marked) = sidecar.collecting_since {
        // Another pass committed to this deletion. Give it one grace to
        // finish; after that the mark is from an interrupted pass, so lift
        // it and restart the clock rather than trust a stale decision
        // against what may now be a re-staged envelope.
        if grace_elapsed(now, marked, limits.orphan_grace_ms) {
            let reset = OwnerSidecar {
                unreferenced_since: Some(now.as_unix_ms()),
                collecting_since: None,
                ..sidecar
            };
            replace_sidecar(
                store,
                object_scope,
                sidecar_key,
                digest,
                encode_sidecar(&reset)?,
            )
            .await?;
        }
        return Ok(None);
    }
    if !sidecar.owners.is_empty() {
        return Ok(None);
    }
    let Some(since) = sidecar.unreferenced_since else {
        let stamped = OwnerSidecar {
            unreferenced_since: Some(now.as_unix_ms()),
            ..sidecar
        };
        replace_sidecar(
            store,
            object_scope,
            sidecar_key,
            digest,
            encode_sidecar(&stamped)?,
        )
        .await?;
        return Ok(None);
    };
    if !grace_elapsed(now, since, limits.orphan_grace_ms) {
        return Ok(None);
    }

    // Commit point: a pin that lands first wins the precondition, and one
    // that lands later observes the mark and fails closed.
    let tombstone = OwnerSidecar {
        collecting_since: Some(now.as_unix_ms()),
        ..sidecar
    };
    let bytes = encode_sidecar(&tombstone)?;
    if !replace_sidecar(store, object_scope, sidecar_key, digest, bytes.clone()).await? {
        return Ok(None);
    }
    Ok(Some(LoadedSidecar {
        sidecar: tombstone,
        digest: Digest::blob_content(&bytes),
    }))
}

/// Remove a committed envelope, then its tombstone, then the blob index
/// entry that names it. The envelope is read once here, only for an entry
/// being deleted: the header names the blob (hence the index key) and the
/// content length.
async fn gc_delete(
    store: &dyn ObjectDriver,
    object_scope: &ObjectScope,
    entry_key: ObjectKey,
    sidecar_key: ObjectKey,
    tombstone: LoadedSidecar,
    now: Timestamp,
) -> Result<GcOutcome, ArtifactError> {
    let envelope = match store.get(object_scope.clone(), entry_key.clone()).await {
        Ok(bytes) => Some(bytes),
        Err(ObjectError::NotFound) => None,
        Err(error) => return Err(map_object_error(error)),
    };
    let mut outcome = GcOutcome::Kept;
    let mut index_cleanup = None;
    if let Some(envelope) = envelope {
        let (header, content) = decode_envelope(&envelope)?;
        let envelope_digest = Digest::blob_content(&envelope);
        if envelope_digest != tombstone.sidecar.envelope_digest {
            // Rewritten since the sidecar was seeded (only a pre-sidecar
            // process does that). Lift the mark with the digest actually
            // stored and restart the clock, so the next pass can collect
            // it instead of re-marking against the stale digest forever.
            let healed = OwnerSidecar {
                envelope_digest,
                unreferenced_since: Some(now.as_unix_ms()),
                collecting_since: None,
                ..tombstone.sidecar
            };
            replace_sidecar(
                store,
                object_scope,
                &sidecar_key,
                tombstone.digest,
                encode_sidecar(&healed)?,
            )
            .await?;
            return Ok(GcOutcome::Kept);
        }
        match store
            .delete_if_digest(object_scope.clone(), entry_key, envelope_digest)
            .await
        {
            Ok(()) => {
                outcome = GcOutcome::Deleted(u64::try_from(content.len()).unwrap_or(u64::MAX));
            }
            Err(ObjectError::NotFound) => {}
            Err(ObjectError::Conflict) => return Ok(GcOutcome::Kept),
            Err(error) => return Err(map_object_error(error)),
        }
        index_cleanup = Some((
            by_blob_key(header.artifact.blob())?,
            Digest::blob_content(&encode_blob_index(&header.artifact)?),
        ));
    }
    match store
        .delete_if_digest(object_scope.clone(), sidecar_key, tombstone.digest)
        .await
    {
        Ok(()) | Err(ObjectError::Conflict | ObjectError::NotFound) => {}
        Err(error) => return Err(map_object_error(error)),
    }
    if let Some((index_key, expected)) = index_cleanup {
        // Only the entry naming this exact artifact is removed; an entry
        // re-pointed at another artifact with the same blob survives.
        match store
            .delete_if_digest(object_scope.clone(), index_key, expected)
            .await
        {
            Ok(()) | Err(ObjectError::Conflict | ObjectError::NotFound) => {}
            Err(error) => return Err(map_object_error(error)),
        }
    }
    Ok(outcome)
}

async fn collect_orphans_impl(
    store: &dyn ObjectDriver,
    scope: ArtifactScope,
    now: Timestamp,
    limit: usize,
    limits: ArtifactStoreLimits,
) -> Result<ArtifactGcReport, ArtifactError> {
    scope.digest()?;
    let object_scope = to_object_scope(&scope);
    let prefix = ObjectKey::try_new(ENVELOPE_PREFIX).map_err(map_object_error)?;
    if limit == 0 {
        return Ok(ArtifactGcReport::default());
    }
    let cursor_key = ObjectKey::try_new("artifacts/gc-cursor/v1").map_err(map_object_error)?;
    let mut cursor = load_gc_cursor(store, &object_scope, &cursor_key).await?;
    let mut report = ArtifactGcReport::default();
    while report.examined < limit {
        let listed = store
            .list(
                object_scope.clone(),
                Some(prefix.clone()),
                cursor.page.clone(),
            )
            .await
            .map_err(map_object_error)?;
        let start = cursor.offset;
        let page_len = listed.entries.len();
        let mut consumed = start;
        let mut deleted_on_page = 0;
        for entry in listed.entries.into_iter().skip(start) {
            if report.examined >= limit {
                break;
            }
            consumed += 1;
            report.examined += 1;
            if let GcOutcome::Deleted(bytes) =
                gc_examine(store, &scope, &object_scope, entry.key, now, &limits).await?
            {
                deleted_on_page += 1;
                report.deleted += 1;
                report.bytes_deleted = report.bytes_deleted.saturating_add(bytes);
            }
        }
        if consumed < page_len {
            // A partially consumed page is listed again next time. Account
            // for our own deletions so surviving entries are not skipped.
            cursor.offset = consumed.saturating_sub(deleted_on_page);
            break;
        }
        cursor.offset = 0;
        let Some(next) = listed.next else {
            cursor.page = PageToken::first();
            break;
        };
        cursor.page = next;
    }
    save_gc_cursor(store, object_scope, cursor_key, cursor).await?;
    Ok(report)
}

async fn save_gc_cursor(
    store: &dyn ObjectDriver,
    scope: ObjectScope,
    key: ObjectKey,
    cursor: GcCursor,
) -> Result<(), ArtifactError> {
    let bytes = serde_json::to_vec(&cursor).map_err(|_| ArtifactError::InvalidMetadata {
        message: Arc::from("artifact_gc_cursor_invalid"),
    })?;
    store
        .put(scope, key, Bytes::from(bytes), object_metadata(None))
        .await
        .map_err(map_object_error)?;
    Ok(())
}
