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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactEnvelopeHeader {
    artifact: ArtifactRef,
    owners: BTreeSet<ArtifactOwnerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unreferenced_since: Option<Timestamp>,
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
}

fn clamp_to_object_limit(requested: usize, object_max_bytes: u64) -> usize {
    let object_max = usize::try_from(object_max_bytes).unwrap_or(usize::MAX);
    requested.min(object_max)
}

pub(crate) fn to_object_scope(scope: &ArtifactScope) -> ObjectScope {
    ObjectScope {
        tenant_scope: Arc::clone(&scope.tenant_scope),
        session_id: Some(scope.session_id),
        run_id: scope.run_id,
        sensitivity: scope.sensitivity,
    }
}

fn artifact_key(storage_key: &Digest) -> Result<ObjectKey, ArtifactError> {
    ObjectKey::try_new(format!("artifacts/v2/{}", storage_key.to_hex())).map_err(map_object_error)
}

fn encode_envelope(
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

fn decode_envelope(envelope: &[u8]) -> Result<(ArtifactEnvelopeHeader, Bytes), ArtifactError> {
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

/// Map an [`ObjectError`] onto the matching [`ArtifactError`] per the
/// adapter's fixed error table.
pub(crate) fn map_object_error(error: ObjectError) -> ArtifactError {
    match error {
        ObjectError::NotFound => ArtifactError::NotFound,
        ObjectError::Conflict => ArtifactError::Unavailable {
            message: Arc::from("artifact_concurrent_mutation"),
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
        media_type: Arc::from("application/vnd.finstack.artifact-v2"),
        name,
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
        || object_ref.media_type.as_ref() != "application/vnd.finstack.artifact-v2"
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
    let key = artifact_key(&artifact_storage_key(&scope, &artifact)?)?;
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
        Ok(object_ref) => validate_object_ref(&object_ref, &object_scope, &key, &envelope)?,
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
    Ok(artifact)
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
    let prefix = ObjectKey::try_new("artifacts/v2").map_err(map_object_error)?;
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
    update_envelope(store, &scope, &artifact, |header| {
        if header.owners.contains(&owner) {
            return Ok(false);
        }
        if header.owners.len() >= limits.max_owners_per_artifact {
            return Err(ArtifactError::CapacityExceeded {
                resource: "owners",
                limit: u64::try_from(limits.max_owners_per_artifact).unwrap_or(u64::MAX),
            });
        }
        header.owners.insert(owner.clone());
        header.unreferenced_since = None;
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
    update_envelope(store, &scope, &artifact, |header| {
        if !header.owners.remove(&owner) {
            return Ok(false);
        }
        if header.owners.is_empty() {
            header.unreferenced_since = Some(now);
        }
        Ok(true)
    })
    .await
}

/// Apply `update` to the stored envelope header and write it back under a
/// digest precondition, retrying from a fresh read on every conflict.
///
/// `update` returns `Ok(false)` when the header already has the desired
/// shape, in which case nothing is written.
async fn update_envelope(
    store: &dyn ObjectDriver,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
    mut update: impl FnMut(&mut ArtifactEnvelopeHeader) -> Result<bool, ArtifactError>,
) -> Result<(), ArtifactError> {
    let key = artifact_key(&artifact_storage_key(scope, artifact)?)?;
    let object_scope = to_object_scope(scope);
    for _ in 0..MAX_CONDITIONAL_RETRIES {
        let (envelope, mut header, content) = load_envelope(store, scope, artifact).await?;
        if !update(&mut header)? {
            return Ok(());
        }
        let replacement = encode_envelope(&header, &content)?;
        let result = store
            .replace_if_digest(
                object_scope.clone(),
                key.clone(),
                Digest::blob_content(&envelope),
                replacement.clone(),
                object_metadata(artifact.blob().name().map(Arc::from)),
            )
            .await;
        match result {
            Ok(object_ref) => {
                validate_object_ref(&object_ref, &object_scope, &key, &replacement)?;
                return Ok(());
            }
            Err(ObjectError::Conflict) => {}
            Err(error) => return Err(map_object_error(error)),
        }
    }
    Err(ArtifactError::Unavailable {
        message: Arc::from("artifact_concurrent_mutation"),
    })
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

async fn collect_orphans_impl(
    store: &dyn ObjectDriver,
    scope: ArtifactScope,
    now: Timestamp,
    limit: usize,
    limits: ArtifactStoreLimits,
) -> Result<ArtifactGcReport, ArtifactError> {
    scope.digest()?;
    let object_scope = to_object_scope(&scope);
    let prefix = ObjectKey::try_new("artifacts/v2").map_err(map_object_error)?;
    let mut page = PageToken::first();
    let mut report = ArtifactGcReport::default();
    while report.examined < limit {
        let listed = store
            .list(object_scope.clone(), Some(prefix.clone()), page)
            .await
            .map_err(map_object_error)?;
        for entry in listed.entries {
            if report.examined >= limit {
                break;
            }
            report.examined += 1;
            let Some((envelope, mut header, content)) =
                listed_envelope(store, &scope, &object_scope, &entry.key).await?
            else {
                continue;
            };
            if !header.owners.is_empty() {
                continue;
            }
            let envelope_digest = Digest::blob_content(&envelope);
            let Some(since) = header.unreferenced_since else {
                header.unreferenced_since = Some(now);
                let replacement = encode_envelope(&header, &content)?;
                match store
                    .replace_if_digest(
                        object_scope.clone(),
                        entry.key.clone(),
                        envelope_digest,
                        replacement,
                        object_metadata(header.artifact.blob().name().map(Arc::from)),
                    )
                    .await
                {
                    Ok(_) | Err(ObjectError::Conflict) => continue,
                    Err(error) => return Err(map_object_error(error)),
                }
            };
            let elapsed = now.as_unix_ms().checked_sub(since.as_unix_ms());
            if elapsed.and_then(|value| u64::try_from(value).ok()) < Some(limits.orphan_grace_ms) {
                continue;
            }
            match store
                .delete_if_digest(
                    object_scope.clone(),
                    entry.key,
                    Digest::blob_content(&envelope),
                )
                .await
            {
                Ok(()) => {
                    report.deleted += 1;
                    report.bytes_deleted = report
                        .bytes_deleted
                        .saturating_add(u64::try_from(content.len()).unwrap_or(u64::MAX));
                }
                Err(ObjectError::Conflict | ObjectError::NotFound) => {}
                Err(error) => return Err(map_object_error(error)),
            }
        }
        let Some(next) = listed.next else {
            break;
        };
        page = next;
    }
    Ok(report)
}
