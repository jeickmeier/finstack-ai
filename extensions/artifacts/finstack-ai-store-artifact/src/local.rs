//! Local-filesystem [`ObjectDriver`] backend for finstack-ai.
//!
//! Each object is one atomically published, self-describing envelope beneath
//! its full scope digest. The logical key is hashed only for the physical
//! filename and retained in the envelope, so collisions fail closed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::driver::{
    ObjectDriver, ObjectEntry, ObjectError, ObjectKey, ObjectMetadata, ObjectPage, ObjectRef,
    ObjectScope, ObjectStoreLimits, PageToken, validate_object_metadata,
};
use finstack_ai_kernel::Digest;
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::ports::PortFuture;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const LIST_PAGE_SIZE: usize = 1000;
const ENVELOPE_MAGIC: &[u8; 8] = b"FSAIOBJ\0";
const ENVELOPE_VERSION: u16 = 1;
const ENVELOPE_SUFFIX: &str = ".fsaiobj";
const FIXED_HEADER_LEN: usize = 8 + 2 + 32 + 4 + 4 + 32 + 8;
const MAX_STORED_KEY_BYTES: usize = 1024;
const MAX_MEDIA_TYPE_BYTES: usize = 1024;

/// Local-filesystem `ObjectDriver` backend.
///
/// Intended for local development and tests. Existing roots written by the
/// former multi-file layout must be discarded and rebuilt.
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
        Ok(Self {
            root: root_dir,
            limits: ObjectStoreLimits::default(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvelopeHeader {
    scope_digest: Digest,
    key: ObjectKey,
    content_digest: Digest,
    length: u64,
    media_type: Arc<str>,
}

impl EnvelopeHeader {
    fn encode(&self) -> Result<Vec<u8>, ObjectError> {
        let key = self.key.as_str().as_bytes();
        let media_type = self.media_type.as_bytes();
        let key_len = u32::try_from(key.len()).map_err(|_| corrupt("object_key_too_large"))?;
        let media_len =
            u32::try_from(media_type.len()).map_err(|_| corrupt("media_type_too_large"))?;
        let capacity = FIXED_HEADER_LEN
            .checked_add(key.len())
            .and_then(|value| value.checked_add(media_type.len()))
            .ok_or_else(|| corrupt("envelope_header_too_large"))?;
        let mut bytes = Vec::with_capacity(capacity);
        bytes.extend_from_slice(ENVELOPE_MAGIC);
        bytes.extend_from_slice(&ENVELOPE_VERSION.to_be_bytes());
        bytes.extend_from_slice(self.scope_digest.as_bytes());
        bytes.extend_from_slice(&key_len.to_be_bytes());
        bytes.extend_from_slice(&media_len.to_be_bytes());
        bytes.extend_from_slice(self.content_digest.as_bytes());
        bytes.extend_from_slice(&self.length.to_be_bytes());
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(media_type);
        Ok(bytes)
    }
}

struct OpenEnvelope {
    file: tokio::fs::File,
    header: EnvelopeHeader,
}

struct CleanupGuard {
    path: PathBuf,
    armed: bool,
}

impl CleanupGuard {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            armed: true,
        }
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

impl ObjectDriver for LocalObjectStore {
    fn put(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: Bytes,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let root = self.root.clone();
        let limits = self.limits;
        Box::pin(async move {
            let _lock = acquire_mutation_lock(&root, &scope, &key).await?;
            put_impl(&root, limits, scope, key, content, metadata).await
        })
    }

    fn get(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<Bytes, ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move { get_impl(&root, scope, key).await })
    }

    fn delete(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<(), ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move {
            let _lock = acquire_mutation_lock(&root, &scope, &key).await?;
            delete_impl(&root, scope, key).await
        })
    }

    fn put_if_absent(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: Bytes,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let root = self.root.clone();
        let limits = self.limits;
        Box::pin(async move {
            let _lock = acquire_mutation_lock(&root, &scope, &key).await?;
            match open_envelope(&root, &scope, &key).await {
                Ok(_) => return Err(ObjectError::Conflict),
                Err(ObjectError::NotFound) => {}
                Err(error) => return Err(error),
            }
            put_impl(&root, limits, scope, key, content, metadata).await
        })
    }

    fn replace_if_digest(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expected: Digest,
        content: Bytes,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let root = self.root.clone();
        let limits = self.limits;
        Box::pin(async move {
            let _lock = acquire_mutation_lock(&root, &scope, &key).await?;
            if open_envelope(&root, &scope, &key)
                .await?
                .header
                .content_digest
                != expected
            {
                return Err(ObjectError::Conflict);
            }
            put_impl(&root, limits, scope, key, content, metadata).await
        })
    }

    fn delete_if_digest(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expected: Digest,
    ) -> PortFuture<Result<(), ObjectError>> {
        let root = self.root.clone();
        Box::pin(async move {
            let _lock = acquire_mutation_lock(&root, &scope, &key).await?;
            if open_envelope(&root, &scope, &key)
                .await?
                .header
                .content_digest
                != expected
            {
                return Err(ObjectError::Conflict);
            }
            delete_impl(&root, scope, key).await
        })
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

    fn limits(&self) -> ObjectStoreLimits {
        self.limits
    }
}

fn io_error(error: &std::io::Error, context: &str) -> ObjectError {
    ObjectError::Io {
        message: Arc::from(format!("{kind:?}: {context}", kind = error.kind())),
    }
}

fn corrupt(message: &'static str) -> ObjectError {
    ObjectError::Integrity {
        message: Arc::from(message),
    }
}

fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

fn digest_from_bytes(bytes: [u8; 32]) -> Result<Digest, ObjectError> {
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        let _ignored = write!(hex, "{byte:02x}");
    }
    Digest::from_hex(&hex).map_err(|_| corrupt("digest_decode_failed"))
}

fn scope_dir(root: &Path, scope_digest: &Digest) -> PathBuf {
    root.join(scope_digest.to_hex())
}

fn envelope_name(key: &ObjectKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"finstack-ai\0object-key\0v1\0");
    hasher.update(key.as_str().as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut name = String::with_capacity(64 + ENVELOPE_SUFFIX.len());
    for byte in digest {
        let _ignored = write!(name, "{byte:02x}");
    }
    name.push_str(ENVELOPE_SUFFIX);
    name
}

fn envelope_path(root: &Path, scope_digest: &Digest, key: &ObjectKey) -> PathBuf {
    scope_dir(root, scope_digest).join(envelope_name(key))
}

async fn create_temp(parent: &Path, context: &str) -> Result<tempfile::NamedTempFile, ObjectError> {
    let parent = parent.to_path_buf();
    let context = context.to_owned();
    tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(parent))
        .await
        .map_err(|_| ObjectError::Io {
            message: Arc::from("tempfile_worker_failed"),
        })?
        .map_err(|error| io_error(&error, &context))
}

async fn sync_directory(path: &Path, context: &str) -> Result<(), ObjectError> {
    let path = path.to_path_buf();
    let context = context.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(path).and_then(|directory| directory.sync_all())
    })
    .await
    .map_err(|_| ObjectError::Io {
        message: Arc::from("directory_sync_worker_failed"),
    })?
    .map_err(|error| io_error(&error, &context))
}

async fn acquire_mutation_lock(
    root: &Path,
    scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<CleanupGuard, ObjectError> {
    let scope_digest = scope.digest()?;
    let object_path = envelope_path(root, &scope_digest, key);
    let Some(parent) = object_path.parent() else {
        return Err(ObjectError::Io {
            message: Arc::from("object_path_has_no_parent"),
        });
    };
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    let mut lock_name = object_path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .await
    {
        Ok(_) => Ok(CleanupGuard::new(lock_path)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(ObjectError::Conflict)
        }
        Err(error) => Err(io_error(&error, key.as_str())),
    }
}

async fn put_impl(
    root: &Path,
    limits: ObjectStoreLimits,
    scope: ObjectScope,
    key: ObjectKey,
    content: Bytes,
    metadata: ObjectMetadata,
) -> Result<ObjectRef, ObjectError> {
    validate_object_metadata(&metadata)?;
    let scope_digest = scope.digest()?;
    let path = envelope_path(root, &scope_digest, &key);
    let Some(parent) = path.parent().map(Path::to_path_buf) else {
        return Err(ObjectError::Io {
            message: Arc::from("object_path_has_no_parent"),
        });
    };
    tokio::fs::create_dir_all(&parent)
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;

    let placeholder = EnvelopeHeader {
        scope_digest,
        key: key.clone(),
        content_digest: Digest::blob_content(&[]),
        length: 0,
        media_type: metadata.media_type.clone(),
    };
    let placeholder_bytes = placeholder.encode()?;
    let temp = create_temp(&parent, key.as_str()).await?;
    let mut guard = CleanupGuard::new(temp.path());
    let clone = temp
        .as_file()
        .try_clone()
        .map_err(|error| io_error(&error, key.as_str()))?;
    let mut output = tokio::fs::File::from_std(clone);
    output
        .write_all(&placeholder_bytes)
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    let (content_digest, length) =
        write_payload(content, &mut output, limits.max_object_bytes, &key).await?;
    let header = EnvelopeHeader {
        scope_digest,
        key: key.clone(),
        content_digest,
        length,
        media_type: metadata.media_type,
    };
    let header_bytes = header.encode()?;
    if header_bytes.len() != placeholder_bytes.len() {
        return Err(corrupt("envelope_header_length_changed"));
    }
    output
        .seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    output
        .write_all(&header_bytes)
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    output
        .flush()
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    output
        .sync_all()
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    drop(output);

    let key_label = key.as_str().to_owned();
    tokio::task::spawn_blocking(move || temp.persist(path))
        .await
        .map_err(|_| ObjectError::Io {
            message: Arc::from("envelope_publish_worker_failed"),
        })?
        .map_err(|error| io_error(&error.error, &key_label))?;
    guard.disarm();
    sync_directory(&parent, key.as_str()).await?;
    Ok(object_ref(header))
}

async fn write_payload(
    content: Bytes,
    output: &mut tokio::fs::File,
    max_object_bytes: u64,
    key: &ObjectKey,
) -> Result<(Digest, u64), ObjectError> {
    let length = u64::try_from(content.len()).map_err(|_| ObjectError::Io {
        message: Arc::from("length_overflow"),
    })?;
    if length > max_object_bytes {
        return Err(ObjectError::TooLarge {
            len: length,
            max: max_object_bytes,
        });
    }
    output
        .write_all(&content)
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    Ok((Digest::blob_content(&content), length))
}

async fn open_envelope(
    root: &Path,
    scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<OpenEnvelope, ObjectError> {
    let scope_digest = scope.digest()?;
    let path = envelope_path(root, &scope_digest, key);
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if is_not_found(&error) => return Err(ObjectError::NotFound),
        Err(error) => return Err(io_error(&error, key.as_str())),
    };
    let (header, payload_offset) = read_header(&mut file, key.as_str()).await?;
    if header.scope_digest != scope_digest {
        return Err(ObjectError::ScopeMismatch {
            expected: scope_digest,
            actual: header.scope_digest,
        });
    }
    if header.key != *key {
        return Err(corrupt("object_key_collision"));
    }
    validate_file_length(&file, payload_offset, header.length, key.as_str()).await?;
    Ok(OpenEnvelope { file, header })
}

async fn read_header(
    file: &mut tokio::fs::File,
    context: &str,
) -> Result<(EnvelopeHeader, u64), ObjectError> {
    let mut fixed = [0_u8; FIXED_HEADER_LEN];
    file.read_exact(&mut fixed).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            corrupt("envelope_truncated")
        } else {
            io_error(&error, context)
        }
    })?;
    if fixed.get(..8) != Some(ENVELOPE_MAGIC.as_slice()) {
        return Err(corrupt("envelope_magic_invalid"));
    }
    if read_u16(&fixed, 8)? != ENVELOPE_VERSION {
        return Err(corrupt("envelope_version_unsupported"));
    }
    let scope_digest = read_digest(&fixed, 10)?;
    let key_len = usize::try_from(read_u32(&fixed, 42)?)
        .map_err(|_| corrupt("envelope_key_length_invalid"))?;
    let media_len = usize::try_from(read_u32(&fixed, 46)?)
        .map_err(|_| corrupt("envelope_media_length_invalid"))?;
    if key_len == 0 || key_len > MAX_STORED_KEY_BYTES {
        return Err(corrupt("envelope_key_length_invalid"));
    }
    if media_len == 0 || media_len > MAX_MEDIA_TYPE_BYTES {
        return Err(corrupt("envelope_media_length_invalid"));
    }
    let content_digest = read_digest(&fixed, 50)?;
    let length = read_u64(&fixed, 82)?;
    let variable_len = key_len
        .checked_add(media_len)
        .ok_or_else(|| corrupt("envelope_header_too_large"))?;
    let mut variable = vec![0_u8; variable_len];
    file.read_exact(&mut variable).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            corrupt("envelope_truncated")
        } else {
            io_error(&error, context)
        }
    })?;
    let key_bytes = variable
        .get(..key_len)
        .ok_or_else(|| corrupt("envelope_key_invalid"))?;
    let media_bytes = variable
        .get(key_len..)
        .ok_or_else(|| corrupt("envelope_media_type_invalid"))?;
    let key_text = std::str::from_utf8(key_bytes).map_err(|_| corrupt("envelope_key_invalid"))?;
    let key = ObjectKey::try_new(key_text).map_err(|_| corrupt("envelope_key_invalid"))?;
    let media_type =
        std::str::from_utf8(media_bytes).map_err(|_| corrupt("envelope_media_type_invalid"))?;
    if media_type.is_empty() || media_type.contains('\0') {
        return Err(corrupt("envelope_media_type_invalid"));
    }
    let payload_offset = u64::try_from(FIXED_HEADER_LEN + variable_len)
        .map_err(|_| corrupt("envelope_header_too_large"))?;
    Ok((
        EnvelopeHeader {
            scope_digest,
            key,
            content_digest,
            length,
            media_type: Arc::from(media_type),
        },
        payload_offset,
    ))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ObjectError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| corrupt("envelope_truncated"))?;
    Ok(u16::from_be_bytes(
        value
            .try_into()
            .map_err(|_| corrupt("envelope_truncated"))?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ObjectError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| corrupt("envelope_truncated"))?;
    Ok(u32::from_be_bytes(
        value
            .try_into()
            .map_err(|_| corrupt("envelope_truncated"))?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ObjectError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| corrupt("envelope_truncated"))?;
    Ok(u64::from_be_bytes(
        value
            .try_into()
            .map_err(|_| corrupt("envelope_truncated"))?,
    ))
}

fn read_digest(bytes: &[u8], offset: usize) -> Result<Digest, ObjectError> {
    let value = bytes
        .get(offset..offset + 32)
        .ok_or_else(|| corrupt("envelope_truncated"))?;
    digest_from_bytes(
        value
            .try_into()
            .map_err(|_| corrupt("envelope_truncated"))?,
    )
}

async fn validate_file_length(
    file: &tokio::fs::File,
    payload_offset: u64,
    payload_length: u64,
    context: &str,
) -> Result<(), ObjectError> {
    let expected = payload_offset
        .checked_add(payload_length)
        .ok_or_else(|| corrupt("envelope_length_overflow"))?;
    let actual = file
        .metadata()
        .await
        .map_err(|error| io_error(&error, context))?
        .len();
    if actual != expected {
        return Err(corrupt(if actual < expected {
            "envelope_truncated"
        } else {
            "envelope_trailing_bytes"
        }));
    }
    Ok(())
}

fn object_ref(header: EnvelopeHeader) -> ObjectRef {
    ObjectRef {
        key: header.key,
        scope_digest: header.scope_digest,
        content_digest: header.content_digest,
        length: header.length,
        media_type: header.media_type,
    }
}

async fn get_impl(root: &Path, scope: ObjectScope, key: ObjectKey) -> Result<Bytes, ObjectError> {
    let mut opened = open_envelope(root, &scope, &key).await?;
    let capacity =
        usize::try_from(opened.header.length).map_err(|_| corrupt("envelope_length_overflow"))?;
    let mut content = Vec::with_capacity(capacity);
    opened
        .file
        .read_to_end(&mut content)
        .await
        .map_err(|error| io_error(&error, key.as_str()))?;
    if Digest::blob_content(&content) != opened.header.content_digest {
        return Err(corrupt("content_digest_mismatch"));
    }
    Ok(Bytes::from(content))
}

async fn delete_impl(root: &Path, scope: ObjectScope, key: ObjectKey) -> Result<(), ObjectError> {
    let scope_digest = scope.digest()?;
    let path = envelope_path(root, &scope_digest, &key);
    match tokio::fs::remove_file(path).await {
        Ok(()) => sync_directory(&scope_dir(root, &scope_digest), key.as_str()).await,
        Err(error) if is_not_found(&error) => Ok(()),
        Err(error) => Err(io_error(&error, key.as_str())),
    }
}

async fn list_impl(
    root: &Path,
    scope: ObjectScope,
    prefix: Option<ObjectKey>,
    page: PageToken,
) -> Result<ObjectPage, ObjectError> {
    let scope_digest = scope.digest()?;
    let directory = scope_dir(root, &scope_digest);
    let mut matching = Vec::new();
    let mut entries = match tokio::fs::read_dir(&directory).await {
        Ok(entries) => entries,
        Err(error) if is_not_found(&error) => {
            return Ok(ObjectPage {
                entries: Vec::new(),
                next: None,
            });
        }
        Err(error) => return Err(io_error(&error, "list")),
    };
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error(&error, "list"))?
    {
        let file_type = entry
            .file_type()
            .await
            .map_err(|error| io_error(&error, "list"))?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !file_type.is_file() || !file_name.ends_with(ENVELOPE_SUFFIX) {
            continue;
        }
        let mut file = tokio::fs::File::open(entry.path())
            .await
            .map_err(|error| io_error(&error, "list"))?;
        let (header, payload_offset) = read_header(&mut file, "list").await?;
        if header.scope_digest != scope_digest {
            return Err(corrupt("envelope_scope_mismatch"));
        }
        if envelope_name(&header.key) != file_name {
            return Err(corrupt("object_key_collision"));
        }
        validate_file_length(&file, payload_offset, header.length, "list").await?;
        if prefix
            .as_ref()
            .is_none_or(|prefix| header.key.as_str().starts_with(prefix.as_str()))
        {
            matching.push((header.key, header.length));
        }
    }
    matching.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    let start_index = match page.value() {
        Some(cursor) => matching
            .iter()
            .position(|(key, _)| key.as_str() == cursor)
            .map_or(0, |index| index + 1),
        None => 0,
    };
    let remaining = matching
        .get(start_index.min(matching.len())..)
        .unwrap_or(&[]);
    let mut page_entries = Vec::new();
    let mut last_key = None;
    for (key, length) in remaining.iter().take(LIST_PAGE_SIZE) {
        page_entries.push(ObjectEntry {
            key: key.clone(),
            length: *length,
        });
        last_key = Some(key.as_str().to_owned());
    }
    let consumed = start_index + page_entries.len();
    let next = if consumed < matching.len() {
        last_key.map(PageToken::opaque)
    } else {
        None
    };
    Ok(ObjectPage {
        entries: page_entries,
        next,
    })
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{Metadata, Sensitivity};

    use super::*;

    fn scope(tenant: &str) -> ObjectScope {
        ObjectScope {
            tenant_scope: Arc::from(tenant),
            session_id: None,
            run_id: None,
            sensitivity: Sensitivity::Internal,
        }
    }

    fn test_metadata() -> ObjectMetadata {
        ObjectMetadata {
            media_type: Arc::from("application/octet-stream"),
            name: None,
            attributes: Metadata::empty(),
        }
    }

    #[tokio::test]
    async fn segmented_and_parent_keys_coexist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
        let scope = scope("tenant-a");
        for (key, body) in [("a", b"one".as_slice()), ("a/b", b"two".as_slice())] {
            store
                .put(
                    scope.clone(),
                    ObjectKey::try_new(key).expect("key"),
                    Bytes::copy_from_slice(body),
                    test_metadata(),
                )
                .await
                .expect("put");
        }
        assert_eq!(
            store
                .get(scope.clone(), ObjectKey::try_new("a").expect("key"))
                .await
                .expect("get"),
            Bytes::from_static(b"one")
        );
        assert_eq!(
            store
                .get(scope, ObjectKey::try_new("a/b").expect("key"))
                .await
                .expect("get"),
            Bytes::from_static(b"two")
        );
    }

    #[tokio::test]
    async fn tampered_content_fails_integrity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
        let scope = scope("tenant-a");
        let key = ObjectKey::try_new("docs/a.bin").expect("key");
        store
            .put(
                scope.clone(),
                key.clone(),
                Bytes::from(vec![7_u8; 32]),
                test_metadata(),
            )
            .await
            .expect("put");
        let path = envelope_path(dir.path(), &scope.digest().expect("digest"), &key);
        let mut bytes = std::fs::read(&path).expect("read envelope");
        *bytes.last_mut().expect("payload byte") ^= 0xff;
        std::fs::write(path, bytes).expect("tamper");
        assert_eq!(
            store.get(scope, key).await.expect_err("must fail").code(),
            crate::driver::OBJECT_INTEGRITY_FAILURE
        );
    }

    #[tokio::test]
    async fn concurrent_overwrite_reads_only_complete_envelopes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            LocalObjectStore::try_new(dir.path().to_path_buf()).expect("construct local store"),
        );
        let scope = scope("tenant-a");
        let key = ObjectKey::try_new("concurrent").expect("key");
        let old = Bytes::from(vec![0x55_u8; 128 * 1024]);
        let new = Bytes::from(vec![0xaa_u8; 128 * 1024]);
        store
            .put(scope.clone(), key.clone(), old.clone(), test_metadata())
            .await
            .expect("seed");

        let writer_store = Arc::clone(&store);
        let writer_scope = scope.clone();
        let writer_key = key.clone();
        let writer_old = old.clone();
        let writer_new = new.clone();
        let writer = tokio::spawn(async move {
            for iteration in 0..40 {
                let body = if iteration % 2 == 0 {
                    writer_new.clone()
                } else {
                    writer_old.clone()
                };
                writer_store
                    .put(
                        writer_scope.clone(),
                        writer_key.clone(),
                        body,
                        test_metadata(),
                    )
                    .await
                    .expect("overwrite");
            }
        });
        for _ in 0..160 {
            let observed = store
                .get(scope.clone(), key.clone())
                .await
                .expect("concurrent read");
            assert!(observed == old || observed == new);
            tokio::task::yield_now().await;
        }
        writer.await.expect("writer task");
    }

    #[tokio::test]
    async fn truncated_and_trailing_envelopes_fail_closed() {
        for trailing in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
            let scope = scope("tenant-a");
            let key = ObjectKey::try_new("object").expect("key");
            store
                .put(
                    scope.clone(),
                    key.clone(),
                    Bytes::from_static(b"payload"),
                    test_metadata(),
                )
                .await
                .expect("put");
            let path = envelope_path(dir.path(), &scope.digest().expect("digest"), &key);
            let mut bytes = std::fs::read(&path).expect("read");
            if trailing {
                bytes.push(0);
            } else {
                bytes.pop();
            }
            std::fs::write(path, bytes).expect("write");
            assert_eq!(
                store.get(scope, key).await.expect_err("must fail").code(),
                crate::driver::OBJECT_INTEGRITY_FAILURE
            );
        }
    }
}
