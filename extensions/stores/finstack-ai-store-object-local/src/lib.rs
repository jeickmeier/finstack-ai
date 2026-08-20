//! Local-filesystem `ObjectStore` backend for finstack-ai.
//!
//! Content lands at `{root}/{physical_object_key(None, scope, key)}`, with a
//! JSON sidecar carrying `{ scope_digest, content_digest, length, media_type
//! }` at the same path plus `.meta.json`. Writes go through
//! [`tempfile::NamedTempFile`] in the destination directory and are
//! published by renaming the sidecar into place, then the content: a crash
//! between those two renames leaves an orphan sidecar with no content file,
//! which [`ObjectStore::get`]/`head`/`get_to_file` all treat as
//! [`ObjectError::NotFound`].

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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::Digest;
use finstack_ai_runtime::{
    Bytes, ObjectError, ObjectEntry, ObjectKey, ObjectMetadata, ObjectPage, ObjectRef, ObjectScope,
    ObjectStore, ObjectStoreLimits, PageToken, PortFuture, PresignedUrl, PutPayload,
    physical_object_key, validate_object_metadata,
};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Domain name matching [`Digest::blob_content`]'s internal hashing so
/// streamed content digests are bit-identical to in-memory ones.
const BLOB_CONTENT_DOMAIN: &[u8] = b"blob-content";
/// Schema version matching [`Digest::blob_content`].
const BLOB_CONTENT_SCHEMA_VERSION: u32 = 1;
/// Bytes read per streaming chunk for file payloads and `get_to_file`.
const STREAM_CHUNK_BYTES: usize = 8 * 1024;
/// Sidecar file suffix appended to the content path.
const SIDECAR_SUFFIX: &str = ".meta.json";
/// Maximum entries returned per [`LocalObjectStore::list`] page.
const LIST_PAGE_SIZE: usize = 1000;

/// Local-filesystem `ObjectStore` backend.
///
/// Not for production use: intended for local development and tests. Does
/// not support [`ObjectStore::presign_get`].
pub struct LocalObjectStore {
    root: PathBuf,
    limits: ObjectStoreLimits,
}

impl LocalObjectStore {
    /// Construct a store rooted at `root_dir`, creating it if necessary.
    ///
    /// # Errors
    ///
    /// Returns [`ObjectError::Io`] if `root_dir` cannot be created.
    pub fn try_new(root_dir: PathBuf) -> Result<Self, ObjectError> {
        std::fs::create_dir_all(&root_dir).map_err(|error| io_error(&error, "root_dir"))?;
        Ok(Self { root: root_dir, limits: ObjectStoreLimits::default() })
    }

    /// Set this store's size ceilings.
    #[must_use]
    pub const fn with_limits(mut self, limits: ObjectStoreLimits) -> Self {
        self.limits = limits;
        self
    }

}

fn scope_hex16(scope_digest: &Digest) -> String {
    let hex = scope_digest.to_hex();
    hex.get(..16).unwrap_or(&hex).to_owned()
}

fn sidecar_path(content_path: &Path) -> PathBuf {
    let mut os = content_path.as_os_str().to_owned();
    os.push(SIDECAR_SUFFIX);
    PathBuf::from(os)
}

/// Bounded, non-secret I/O diagnostic: error kind plus a caller-controlled
/// relative context (never an absolute filesystem path).
fn io_error(error: &std::io::Error, context: &str) -> ObjectError {
    ObjectError::Io { message: Arc::from(format!("{kind:?}: {context}", kind = error.kind())) }
}

fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// JSON sidecar persisted alongside each object's content.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarMeta {
    scope_digest: String,
    content_digest: String,
    length: u64,
    media_type: String,
}

impl SidecarMeta {
    fn to_json_bytes(&self) -> Result<Vec<u8>, ObjectError> {
        let value = serde_json::json!({
            "scope_digest": self.scope_digest,
            "content_digest": self.content_digest,
            "length": self.length,
            "media_type": self.media_type,
        });
        serde_json::to_vec(&value)
            .map_err(|_error| ObjectError::Io { message: Arc::from("sidecar_encode_failed") })
    }

    fn from_json_bytes(bytes: &[u8]) -> Result<Self, ObjectError> {
        // A corrupt/unparseable sidecar is malformed metadata, not a
        // content-digest verification failure — `Integrity` is reserved for
        // an actual content-hash mismatch against a well-formed sidecar.
        let corrupt = || ObjectError::InvalidMetadata { message: Arc::from("sidecar_corrupt") };
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_error| corrupt())?;
        let scope_digest =
            value.get("scope_digest").and_then(serde_json::Value::as_str).ok_or_else(corrupt)?.to_owned();
        let content_digest =
            value.get("content_digest").and_then(serde_json::Value::as_str).ok_or_else(corrupt)?.to_owned();
        let length = value.get("length").and_then(serde_json::Value::as_u64).ok_or_else(corrupt)?;
        let media_type =
            value.get("media_type").and_then(serde_json::Value::as_str).ok_or_else(corrupt)?.to_owned();
        Ok(Self { scope_digest, content_digest, length, media_type })
    }

    fn content_digest(&self) -> Result<Digest, ObjectError> {
        Digest::from_hex(&self.content_digest)
            .map_err(|_error| ObjectError::InvalidMetadata { message: Arc::from("sidecar_corrupt") })
    }

    fn scope_digest(&self) -> Result<Digest, ObjectError> {
        Digest::from_hex(&self.scope_digest)
            .map_err(|_error| ObjectError::InvalidMetadata { message: Arc::from("sidecar_corrupt") })
    }
}

/// Incremental hasher reproducing [`Digest::blob_content`]'s
/// domain-separated encoding without materializing the full payload.
struct StreamingBlobDigest {
    hasher: Sha256,
    len: u64,
}

impl StreamingBlobDigest {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"finstack-ai");
        hasher.update([0_u8]);
        hasher.update(BLOB_CONTENT_DOMAIN);
        hasher.update([0_u8]);
        hasher.update(BLOB_CONTENT_SCHEMA_VERSION.to_be_bytes());
        hasher.update([0_u8]);
        Self { hasher, len: 0 }
    }

    fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
        self.len = self.len.saturating_add(chunk.len() as u64);
    }

    fn finish(self) -> Result<(Digest, u64), ObjectError> {
        let bytes: [u8; 32] = self.hasher.finalize().into();
        let mut hex = String::with_capacity(64);
        for byte in bytes {
            // `write!` into a `String` never fails.
            let _ignored = write!(hex, "{byte:02x}");
        }
        let digest = Digest::from_hex(&hex)
            .map_err(|_error| ObjectError::Io { message: Arc::from("digest_encode_failed") })?;
        Ok((digest, self.len))
    }
}

/// Guard that best-effort removes a path on drop unless disarmed.
///
/// Used so an oversize or failed streaming `put` never leaves a stray
/// half-written content file behind under the store's root. Holds an owned
/// path rather than borrowing the temp file, so it can be armed/disarmed
/// independently of consuming the temp file via `persist`.
struct CleanupGuard {
    path: PathBuf,
    armed: bool,
}

impl CleanupGuard {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), armed: true }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ignored = std::fs::remove_file(&self.path);
        }
    }
}

impl ObjectStore for LocalObjectStore {
    fn put(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: PutPayload,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let root = self.root.clone();
        let limits = self.limits;
        Box::pin(async move { put_impl(&root, limits, scope, key, content, metadata).await })
    }

    fn get(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<Bytes, ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move { get_impl(&root, scope, key).await })
    }

    fn get_to_file(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        dest: PathBuf,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move { get_to_file_impl(&root, scope, key, dest).await })
    }

    fn head(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move { head_impl(&root, scope, key).await })
    }

    fn delete(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<(), ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move { delete_impl(&root, scope, key).await })
    }

    fn list(
        &self,
        scope: ObjectScope,
        prefix: Option<ObjectKey>,
        page: PageToken,
    ) -> PortFuture<Result<ObjectPage, ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move { list_impl(&root, scope, prefix, page).await })
    }

    fn presign_get(
        &self,
        _scope: ObjectScope,
        _key: ObjectKey,
        _expiry: Duration,
    ) -> PortFuture<Result<PresignedUrl, ObjectError>> {
        Box::pin(async move {
            Err(ObjectError::Unsupported { operation: Arc::from("presign_get") })
        })
    }

    fn limits(&self) -> ObjectStoreLimits {
        self.limits
    }
}

async fn put_impl(
    root: &Path,
    limits: ObjectStoreLimits,
    scope: ObjectScope,
    key: ObjectKey,
    content: PutPayload,
    metadata: ObjectMetadata,
) -> Result<ObjectRef, ObjectError> {
    validate_object_metadata(&metadata)?;
    let scope_digest = scope.digest()?;
    let content_path = root.join(physical_object_key(None, &scope_digest, &key));
    let sidecar = sidecar_path(&content_path);
    let Some(parent) = content_path.parent() else {
        return Err(ObjectError::Io { message: Arc::from("content_path_has_no_parent") });
    };
    tokio::fs::create_dir_all(parent).await.map_err(|error| io_error(&error, key.as_str()))?;

    let content_temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| io_error(&error, key.as_str()))?;
    let mut content_guard = CleanupGuard::new(content_temp.path());

    let (content_digest, length) = match content {
        PutPayload::Bytes(bytes) => {
            let length = u64::try_from(bytes.len())
                .map_err(|_error| ObjectError::Io { message: Arc::from("length_overflow") })?;
            if length > limits.max_object_bytes {
                return Err(ObjectError::TooLarge { len: length, max: limits.max_object_bytes });
            }
            std::io::Write::write_all(&mut content_temp.as_file(), &bytes)
                .map_err(|error| io_error(&error, key.as_str()))?;
            (Digest::blob_content(&bytes), length)
        }
        PutPayload::File(source_path) => {
            let mut source = tokio::fs::File::open(&source_path)
                .await
                .map_err(|error| io_error(&error, key.as_str()))?;
            let clone = content_temp.as_file().try_clone().map_err(|error| io_error(&error, key.as_str()))?;
            let mut dest = tokio::fs::File::from_std(clone);
            let mut hasher = StreamingBlobDigest::new();
            let mut buffer = [0_u8; STREAM_CHUNK_BYTES];
            loop {
                let read = source.read(&mut buffer).await.map_err(|error| io_error(&error, key.as_str()))?;
                if read == 0 {
                    break;
                }
                let chunk = buffer.get(..read).unwrap_or(&[]);
                hasher.update(chunk);
                if hasher.len > limits.max_object_bytes {
                    return Err(ObjectError::TooLarge { len: hasher.len, max: limits.max_object_bytes });
                }
                dest.write_all(chunk).await.map_err(|error| io_error(&error, key.as_str()))?;
            }
            dest.flush().await.map_err(|error| io_error(&error, key.as_str()))?;
            hasher.finish()?
        }
    };

    let sidecar_meta = SidecarMeta {
        scope_digest: scope_digest.to_hex(),
        content_digest: content_digest.to_hex(),
        length,
        media_type: metadata.media_type.to_string(),
    };
    let sidecar_bytes = sidecar_meta.to_json_bytes()?;

    let sidecar_temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| io_error(&error, key.as_str()))?;
    let mut sidecar_guard = CleanupGuard::new(sidecar_temp.path());
    std::io::Write::write_all(&mut sidecar_temp.as_file(), &sidecar_bytes)
        .map_err(|error| io_error(&error, key.as_str()))?;

    // Publish order: sidecar first, then content. A crash between the two
    // renames leaves only an orphan sidecar, which reads treat as NotFound.
    sidecar_temp.persist(&sidecar).map_err(|error| io_error(&error.error, key.as_str()))?;
    sidecar_guard.disarm();
    content_temp.persist(&content_path).map_err(|error| io_error(&error.error, key.as_str()))?;
    content_guard.disarm();

    Ok(ObjectRef { key, scope_digest, content_digest, length, media_type: metadata.media_type })
}

/// Read and validate the sidecar for `scope`/`key`, checking scope binding.
///
/// Returns [`ObjectError::NotFound`] when no sidecar exists at all.
async fn read_sidecar(
    root: &Path,
    scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<(Digest, PathBuf, SidecarMeta), ObjectError> {
    let scope_digest = scope.digest()?;
    let content_path = root.join(physical_object_key(None, &scope_digest, key));
    let sidecar = sidecar_path(&content_path);

    let sidecar_bytes = match tokio::fs::read(&sidecar).await {
        Ok(bytes) => bytes,
        Err(error) if is_not_found(&error) => return Err(ObjectError::NotFound),
        Err(error) => return Err(io_error(&error, key.as_str())),
    };
    let meta = SidecarMeta::from_json_bytes(&sidecar_bytes)?;
    let stored_scope_digest = meta.scope_digest()?;
    if stored_scope_digest != scope_digest {
        return Err(ObjectError::ScopeMismatch { expected: scope_digest, actual: stored_scope_digest });
    }
    Ok((scope_digest, content_path, meta))
}

async fn get_impl(root: &Path, scope: ObjectScope, key: ObjectKey) -> Result<Bytes, ObjectError> {
    let (_scope_digest, content_path, meta) = read_sidecar(root, &scope, &key).await?;

    let content = match tokio::fs::read(&content_path).await {
        Ok(bytes) => bytes,
        // An orphan sidecar (content never published, or removed) reads as
        // a plain missing object.
        Err(error) if is_not_found(&error) => return Err(ObjectError::NotFound),
        Err(error) => return Err(io_error(&error, key.as_str())),
    };

    let expected = meta.content_digest()?;
    if Digest::blob_content(&content) != expected {
        return Err(ObjectError::Integrity { message: Arc::from("content_digest_mismatch") });
    }
    Ok(Bytes::from(content))
}

async fn get_to_file_impl(
    root: &Path,
    scope: ObjectScope,
    key: ObjectKey,
    dest: PathBuf,
) -> Result<ObjectRef, ObjectError> {
    let (scope_digest, content_path, meta) = read_sidecar(root, &scope, &key).await?;

    match tokio::fs::copy(&content_path, &dest).await {
        Ok(_bytes_copied) => {}
        Err(error) if is_not_found(&error) => return Err(ObjectError::NotFound),
        Err(error) => return Err(io_error(&error, key.as_str())),
    }

    let mut file = tokio::fs::File::open(&dest).await.map_err(|error| io_error(&error, key.as_str()))?;
    let mut hasher = StreamingBlobDigest::new();
    let mut buffer = [0_u8; STREAM_CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| io_error(&error, key.as_str()))?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or(&[]));
    }
    let (actual_digest, length) = hasher.finish()?;
    let expected = meta.content_digest()?;
    if actual_digest != expected {
        return Err(ObjectError::Integrity { message: Arc::from("content_digest_mismatch") });
    }

    Ok(ObjectRef {
        key,
        scope_digest,
        content_digest: expected,
        length,
        media_type: Arc::from(meta.media_type.as_str()),
    })
}

async fn head_impl(root: &Path, scope: ObjectScope, key: ObjectKey) -> Result<ObjectRef, ObjectError> {
    let (scope_digest, content_path, meta) = read_sidecar(root, &scope, &key).await?;
    match tokio::fs::metadata(&content_path).await {
        Ok(_metadata) => {}
        Err(error) if is_not_found(&error) => return Err(ObjectError::NotFound),
        Err(error) => return Err(io_error(&error, key.as_str())),
    }
    let content_digest = meta.content_digest()?;
    Ok(ObjectRef {
        key,
        scope_digest,
        content_digest,
        length: meta.length,
        media_type: Arc::from(meta.media_type.as_str()),
    })
}

async fn delete_impl(root: &Path, scope: ObjectScope, key: ObjectKey) -> Result<(), ObjectError> {
    let scope_digest = scope.digest()?;
    let content_path = root.join(physical_object_key(None, &scope_digest, &key));
    let sidecar = sidecar_path(&content_path);

    // Deleting a missing object is not an error, so both removals are
    // best-effort: a missing sidecar or content file is exactly the
    // already-deleted state the caller wants.
    for path in [&content_path, &sidecar] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if is_not_found(&error) => {}
            Err(error) => return Err(io_error(&error, key.as_str())),
        }
    }
    Ok(())
}

async fn list_impl(
    root: &Path,
    scope: ObjectScope,
    prefix: Option<ObjectKey>,
    page: PageToken,
) -> Result<ObjectPage, ObjectError> {
    let scope_digest = scope.digest()?;
    let scope_dir = root.join(scope_hex16(&scope_digest));
    let prefix_str = prefix.as_ref().map(ObjectKey::as_str);

    let mut matching = walk_scope_dir(&scope_dir).await?;
    if let Some(prefix_str) = prefix_str {
        matching.retain(|(logical, _length)| logical.starts_with(prefix_str));
    }
    matching.sort_by(|left, right| left.0.cmp(&right.0));

    let start_index = match page.value() {
        Some(cursor) => matching
            .iter()
            .position(|(logical, _length)| logical == cursor)
            .map_or(0, |index| index + 1),
        None => 0,
    };
    let remaining = matching.get(start_index.min(matching.len())..).unwrap_or(&[]);

    let mut entries = Vec::new();
    let mut last_logical: Option<String> = None;
    for (logical, length) in remaining.iter().take(LIST_PAGE_SIZE) {
        let key = ObjectKey::try_new(logical)
            .map_err(|_error| ObjectError::Io { message: Arc::from("corrupt_physical_key") })?;
        entries.push(ObjectEntry { key, length: *length });
        last_logical = Some(logical.clone());
    }

    let consumed = start_index + entries.len();
    let next = if consumed < matching.len() { last_logical.map(PageToken::opaque) } else { None };

    Ok(ObjectPage { entries, next })
}

/// Iteratively walk `scope_dir`, returning `(logical_key, content_length)`
/// for every content file (sidecars are skipped). A missing `scope_dir`
/// (no objects ever put under this scope) yields an empty list, not an
/// error.
async fn walk_scope_dir(scope_dir: &Path) -> Result<Vec<(String, u64)>, ObjectError> {
    let mut out = Vec::new();
    let mut stack = vec![scope_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if is_not_found(&error) => continue,
            Err(error) => return Err(io_error(&error, "list")),
        };

        while let Some(entry) = entries.next_entry().await.map_err(|error| io_error(&error, "list"))? {
            let file_type = entry.file_type().await.map_err(|error| io_error(&error, "list"))?;
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_name.ends_with(SIDECAR_SUFFIX) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(scope_dir) else {
                continue;
            };
            let logical = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let file_metadata = entry.metadata().await.map_err(|error| io_error(&error, "list"))?;
            out.push((logical, file_metadata.len()));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{Metadata, Sensitivity};

    use super::*;

    fn scope(tenant: &str) -> ObjectScope {
        ObjectScope { tenant_scope: Arc::from(tenant), session_id: None, run_id: None, sensitivity: Sensitivity::Internal }
    }

    fn test_metadata() -> ObjectMetadata {
        ObjectMetadata { media_type: Arc::from("application/octet-stream"), name: None, attributes: Metadata::empty() }
    }

    #[test]
    fn sidecar_round_trips_serde() {
        let meta = SidecarMeta {
            scope_digest: "a".repeat(64),
            content_digest: "b".repeat(64),
            length: 1234,
            media_type: "text/plain".to_owned(),
        };
        let bytes = meta.to_json_bytes().expect("encode");
        let round_tripped = SidecarMeta::from_json_bytes(&bytes).expect("decode");
        assert_eq!(meta, round_tripped);
    }

    #[test]
    fn sidecar_from_json_rejects_missing_fields() {
        let error = SidecarMeta::from_json_bytes(br#"{"scope_digest":"a"}"#).expect_err("must reject");
        assert_eq!(error.code(), finstack_ai_runtime::OBJECT_INVALID_METADATA);
    }

    #[tokio::test]
    async fn tampered_content_fails_integrity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
        let scope = scope("tenant-a");
        let key = ObjectKey::try_new("docs/a.bin").expect("key");

        let object_ref = store
            .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(vec![7_u8; 32])), test_metadata())
            .await
            .expect("put must succeed");

        let scope_digest = scope.digest().expect("digest");
        let content_path = dir.path().join(physical_object_key(None, &scope_digest, &key));
        std::fs::write(&content_path, vec![9_u8; 32]).expect("tamper content");

        let error = store.get(scope, key).await.expect_err("get must fail after tampering");
        assert_eq!(error.code(), finstack_ai_runtime::OBJECT_INTEGRITY_FAILURE);
        assert_eq!(object_ref.length, 32);
    }

    #[tokio::test]
    async fn orphan_sidecar_without_content_is_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
        let scope = scope("tenant-a");
        let key = ObjectKey::try_new("docs/orphan.bin").expect("key");

        store
            .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(vec![1_u8; 16])), test_metadata())
            .await
            .expect("put must succeed");

        let scope_digest = scope.digest().expect("digest");
        let content_path = dir.path().join(physical_object_key(None, &scope_digest, &key));
        std::fs::remove_file(&content_path).expect("remove content, leaving orphan sidecar");

        let error = store.get(scope, key).await.expect_err("get must fail for orphan sidecar");
        assert_eq!(error.code(), finstack_ai_runtime::OBJECT_NOT_FOUND);
    }

    #[tokio::test]
    async fn presign_is_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
        let key = ObjectKey::try_new("docs/a.bin").expect("key");

        let error = store
            .presign_get(scope("tenant-a"), key, Duration::from_mins(1))
            .await
            .expect_err("presign must be unsupported");
        assert_eq!(error.code(), finstack_ai_runtime::OBJECT_UNSUPPORTED);
    }
}
