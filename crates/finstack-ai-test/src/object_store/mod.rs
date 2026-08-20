//! In-memory [`ObjectStore`] fake and the backend-agnostic contract suite.
//!
//! [`run_object_store_contract_suite`] is written once here and reused
//! verbatim by every `ObjectStore` backend (this fake, local filesystem, S3):
//! it asserts only what the trait contract in
//! `finstack_ai_runtime::services::object` promises, never backend-specific
//! behavior.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{Digest, Metadata, Sensitivity};
use finstack_ai_runtime::{
    Bytes, OBJECT_INVALID_KEY, OBJECT_NOT_FOUND, OBJECT_TOO_LARGE, OBJECT_UNSUPPORTED,
    ObjectEntry, ObjectError, ObjectKey, ObjectMetadata, ObjectPage, ObjectRef, ObjectScope,
    ObjectStore, ObjectStoreLimits, PageToken, PortFuture, PresignedUrl, PutPayload,
    physical_object_key, validate_object_metadata,
};

/// Number of entries returned per [`FakeObjectStore::list`] page.
///
/// Deliberately tiny so the contract suite is forced to exercise pagination.
const LIST_PAGE_SIZE: usize = 2;

struct StoredObject {
    scope_digest: Digest,
    content: Bytes,
    content_digest: Digest,
    media_type: Arc<str>,
}

/// In-memory [`ObjectStore`] fake intended for unit and contract tests.
///
/// Objects are keyed by [`physical_object_key`] with no key prefix, exactly
/// as the S3 and local-filesystem backends will key theirs. Presign support
/// is unconditional (returns an opaque `fake://` URL) so this fake also
/// exercises the presign-supported half of the contract suite.
pub struct FakeObjectStore {
    limits: ObjectStoreLimits,
    objects: Mutex<BTreeMap<String, StoredObject>>,
    poison_next_get: AtomicBool,
}

impl Default for FakeObjectStore {
    fn default() -> Self {
        Self {
            limits: ObjectStoreLimits::default(),
            objects: Mutex::new(BTreeMap::new()),
            poison_next_get: AtomicBool::new(false),
        }
    }
}

impl FakeObjectStore {
    /// Construct a fake with explicit size ceilings.
    #[must_use]
    pub fn with_limits(limits: ObjectStoreLimits) -> Self {
        Self { limits, objects: Mutex::new(BTreeMap::new()), poison_next_get: AtomicBool::new(false) }
    }

    /// Poison the next `get`/`get_to_file` call so it returns
    /// [`ObjectError::Integrity`] instead of the stored content.
    ///
    /// Used by the injected-failure test outside the shared contract suite
    /// (backends cannot be made to corrupt their own content on demand, so
    /// this is fake-only surface, not part of the trait contract).
    pub fn fail_next_get_with_integrity(&self) {
        self.poison_next_get.store(true, Ordering::SeqCst);
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, StoredObject>>, ObjectError> {
        self.objects
            .lock()
            .map_err(|_error| ObjectError::Unavailable { message: Arc::from("poisoned_lock") })
    }
}

impl ObjectStore for FakeObjectStore {
    fn put(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: PutPayload,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let result = (|| {
            validate_object_metadata(&metadata)?;
            let scope_digest = scope.digest()?;
            let bytes = match content {
                PutPayload::Bytes(bytes) => bytes,
                PutPayload::File(path) => {
                    let data = std::fs::read(&path)
                        .map_err(|error| ObjectError::Io { message: Arc::from(error.to_string()) })?;
                    Bytes::from(data)
                }
            };
            let length = u64::try_from(bytes.len())
                .map_err(|_error| ObjectError::Io { message: Arc::from("length_overflow") })?;
            if length > self.limits.max_object_bytes {
                return Err(ObjectError::TooLarge { len: length, max: self.limits.max_object_bytes });
            }
            let content_digest = Digest::blob_content(&bytes);
            let physical = physical_object_key(None, &scope_digest, &key);
            let stored = StoredObject {
                scope_digest,
                content: bytes,
                content_digest,
                media_type: Arc::clone(&metadata.media_type),
            };
            let object_ref = ObjectRef {
                key,
                scope_digest,
                content_digest,
                length,
                media_type: Arc::clone(&metadata.media_type),
            };
            let mut objects = self.lock()?;
            objects.insert(physical, stored);
            Ok(object_ref)
        })();
        Box::pin(async move { result })
    }

    fn get(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<Bytes, ObjectError>> {
        let poisoned = self.poison_next_get.swap(false, Ordering::SeqCst);
        let result = (|| {
            let scope_digest = scope.digest()?;
            let physical = physical_object_key(None, &scope_digest, &key);
            let objects = self.lock()?;
            let stored = objects.get(&physical).ok_or(ObjectError::NotFound)?;
            if stored.scope_digest != scope_digest {
                return Err(ObjectError::ScopeMismatch { expected: scope_digest, actual: stored.scope_digest });
            }
            if poisoned {
                return Err(ObjectError::Integrity { message: Arc::from("injected_failure") });
            }
            if Digest::blob_content(&stored.content) != stored.content_digest {
                return Err(ObjectError::Integrity { message: Arc::from("digest_mismatch") });
            }
            Ok(stored.content.clone())
        })();
        Box::pin(async move { result })
    }

    fn get_to_file(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        dest: PathBuf,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let poisoned = self.poison_next_get.swap(false, Ordering::SeqCst);
        let result = (|| {
            let scope_digest = scope.digest()?;
            let physical = physical_object_key(None, &scope_digest, &key);
            let objects = self.lock()?;
            let stored = objects.get(&physical).ok_or(ObjectError::NotFound)?;
            if stored.scope_digest != scope_digest {
                return Err(ObjectError::ScopeMismatch { expected: scope_digest, actual: stored.scope_digest });
            }
            std::fs::write(&dest, &stored.content)
                .map_err(|error| ObjectError::Io { message: Arc::from(error.to_string()) })?;
            if poisoned {
                return Err(ObjectError::Integrity { message: Arc::from("injected_failure") });
            }
            if Digest::blob_content(&stored.content) != stored.content_digest {
                return Err(ObjectError::Integrity { message: Arc::from("digest_mismatch") });
            }
            let length = u64::try_from(stored.content.len())
                .map_err(|_error| ObjectError::Io { message: Arc::from("length_overflow") })?;
            Ok(ObjectRef {
                key,
                scope_digest,
                content_digest: stored.content_digest,
                length,
                media_type: Arc::clone(&stored.media_type),
            })
        })();
        Box::pin(async move { result })
    }

    fn head(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let result = (|| {
            let scope_digest = scope.digest()?;
            let physical = physical_object_key(None, &scope_digest, &key);
            let objects = self.lock()?;
            let stored = objects.get(&physical).ok_or(ObjectError::NotFound)?;
            if stored.scope_digest != scope_digest {
                return Err(ObjectError::ScopeMismatch { expected: scope_digest, actual: stored.scope_digest });
            }
            let length = u64::try_from(stored.content.len())
                .map_err(|_error| ObjectError::Io { message: Arc::from("length_overflow") })?;
            Ok(ObjectRef {
                key,
                scope_digest,
                content_digest: stored.content_digest,
                length,
                media_type: Arc::clone(&stored.media_type),
            })
        })();
        Box::pin(async move { result })
    }

    fn delete(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<(), ObjectError>> {
        let result = (|| {
            let scope_digest = scope.digest()?;
            let physical = physical_object_key(None, &scope_digest, &key);
            let mut objects = self.lock()?;
            objects.remove(&physical);
            Ok(())
        })();
        Box::pin(async move { result })
    }

    fn list(
        &self,
        scope: ObjectScope,
        prefix: Option<ObjectKey>,
        page: PageToken,
    ) -> PortFuture<Result<ObjectPage, ObjectError>> {
        let result = (|| {
            let scope_digest = scope.digest()?;
            let hex = scope_digest.to_hex();
            let hex16 = hex.get(..16).unwrap_or(&hex).to_string();
            let scope_prefix = format!("{hex16}/");
            let prefix_str = prefix.as_ref().map(|value| value.as_str().to_string());

            let matching: Vec<(String, u64)> = {
                let objects = self.lock()?;
                objects
                    .iter()
                    .filter(|(physical, _)| physical.starts_with(&scope_prefix))
                    .filter(|(physical, _)| {
                        let logical = &physical[scope_prefix.len()..];
                        prefix_str.as_deref().is_none_or(|prefix| logical.starts_with(prefix))
                    })
                    .map(|(physical, stored)| (physical.clone(), stored.content.len() as u64))
                    .collect()
            };

            let start_index = match page.value() {
                Some(cursor) => matching
                    .iter()
                    .position(|(physical, _)| physical == cursor)
                    .map_or(0, |index| index + 1),
                None => 0,
            };

            let remaining = &matching[start_index.min(matching.len())..];
            let mut entries = Vec::new();
            let mut last_physical: Option<String> = None;
            for (physical, length) in remaining.iter().take(LIST_PAGE_SIZE) {
                let logical = &physical[scope_prefix.len()..];
                let key = ObjectKey::try_new(logical)
                    .map_err(|_error| ObjectError::Io { message: Arc::from("corrupt_physical_key") })?;
                entries.push(ObjectEntry { key, length: *length });
                last_physical = Some(physical.clone());
            }

            let consumed = start_index + entries.len();
            let next = if consumed < matching.len() { last_physical.map(PageToken::opaque) } else { None };

            Ok(ObjectPage { entries, next })
        })();
        Box::pin(async move { result })
    }

    fn presign_get(
        &self,
        _scope: ObjectScope,
        key: ObjectKey,
        expiry: Duration,
    ) -> PortFuture<Result<PresignedUrl, ObjectError>> {
        let url = format!("fake://{key}", key = key.as_str());
        let result = Ok(PresignedUrl { url: Arc::from(url), expires_in_secs: expiry.as_secs() });
        Box::pin(async move { result })
    }

    fn limits(&self) -> ObjectStoreLimits {
        self.limits
    }
}

// ---------------------------------------------------------------------------
// Shared contract suite
// ---------------------------------------------------------------------------

fn test_scope(tenant: &str) -> ObjectScope {
    ObjectScope {
        tenant_scope: Arc::from(tenant),
        session_id: None,
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

fn test_metadata() -> ObjectMetadata {
    ObjectMetadata { media_type: Arc::from("application/octet-stream"), name: None, attributes: Metadata::empty() }
}

fn deterministic_content() -> Vec<u8> {
    vec![7_u8; 1024]
}

/// Run the full `ObjectStore` contract suite against `store`.
///
/// Every case panics with a labeled assertion on the first contract
/// violation it observes. Set `supports_presign` to match the backend: the
/// suite exercises exactly one of the presign-supported or
/// presign-unsupported branches, never both.
pub async fn run_object_store_contract_suite(store: Arc<dyn ObjectStore>, supports_presign: bool) {
    put_get_round_trip(&*store).await;
    put_file_and_get_to_file_round_trip(&*store).await;
    head_matches_put_ref(&*store).await;
    cross_scope_read_fails_closed(&*store).await;
    missing_object_is_not_found(&*store).await;
    delete_is_idempotent(&*store).await;
    list_pages_within_scope_only(&*store).await;
    oversize_put_is_rejected(&*store).await;
    invalid_key_never_reaches_backend();
    if supports_presign {
        presign_returns_url(&*store).await;
    } else {
        presign_is_unsupported(&*store).await;
    }
}

async fn put_get_round_trip(store: &dyn ObjectStore) {
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/a.bin").expect("put_get_round_trip: key must be valid");
    let content = deterministic_content();
    let metadata = test_metadata();

    let object_ref = store
        .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(content.clone())), metadata.clone())
        .await
        .expect("put_get_round_trip: put must succeed");

    assert_eq!(object_ref.key, key, "put_get_round_trip: ref key must echo the logical key");
    assert_eq!(object_ref.length, content.len() as u64, "put_get_round_trip: ref length must match payload");
    assert_eq!(object_ref.media_type, metadata.media_type, "put_get_round_trip: ref media type must match metadata");
    assert_eq!(
        object_ref.scope_digest,
        scope.digest().expect("put_get_round_trip: scope digest"),
        "put_get_round_trip: ref scope digest must match the caller's scope"
    );
    assert_eq!(
        object_ref.content_digest,
        Digest::blob_content(&content),
        "put_get_round_trip: ref content digest must be the SHA-256 of the exact bytes"
    );

    let fetched = store.get(scope, key).await.expect("put_get_round_trip: get must succeed");
    assert_eq!(fetched.as_ref(), content.as_slice(), "put_get_round_trip: get must return the exact bytes");
}

async fn put_file_and_get_to_file_round_trip(store: &dyn ObjectStore) {
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/b.bin").expect("put_file_and_get_to_file_round_trip: key must be valid");
    let content = deterministic_content();
    let metadata = test_metadata();

    let source =
        tempfile::NamedTempFile::new().expect("put_file_and_get_to_file_round_trip: create source tempfile");
    std::fs::write(source.path(), &content)
        .expect("put_file_and_get_to_file_round_trip: write source tempfile");

    let put_ref = store
        .put(scope.clone(), key.clone(), PutPayload::File(source.path().to_path_buf()), metadata)
        .await
        .expect("put_file_and_get_to_file_round_trip: put from file must succeed");

    let dest = tempfile::NamedTempFile::new().expect("put_file_and_get_to_file_round_trip: create dest tempfile");
    let get_ref = store
        .get_to_file(scope, key, dest.path().to_path_buf())
        .await
        .expect("put_file_and_get_to_file_round_trip: get_to_file must succeed");

    assert_eq!(
        put_ref.content_digest, get_ref.content_digest,
        "put_file_and_get_to_file_round_trip: content digests must be equal"
    );
    let written =
        std::fs::read(dest.path()).expect("put_file_and_get_to_file_round_trip: read dest tempfile");
    assert_eq!(written, content, "put_file_and_get_to_file_round_trip: dest file bytes must match source");
}

async fn head_matches_put_ref(store: &dyn ObjectStore) {
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/c.bin").expect("head_matches_put_ref: key must be valid");
    let metadata = test_metadata();

    let put_ref = store
        .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(deterministic_content())), metadata)
        .await
        .expect("head_matches_put_ref: put must succeed");
    let head_ref = store.head(scope, key).await.expect("head_matches_put_ref: head must succeed");

    assert_eq!(put_ref, head_ref, "head_matches_put_ref: head must return the exact ref returned by put");
}

async fn cross_scope_read_fails_closed(store: &dyn ObjectStore) {
    let scope_a = test_scope("tenant-a");
    let scope_b = test_scope("tenant-b");
    let key = ObjectKey::try_new("docs/d.bin").expect("cross_scope_read_fails_closed: key must be valid");
    let metadata = test_metadata();

    store
        .put(scope_a, key.clone(), PutPayload::Bytes(Bytes::from(deterministic_content())), metadata)
        .await
        .expect("cross_scope_read_fails_closed: put must succeed");

    let error = store
        .get(scope_b, key)
        .await
        .expect_err("cross_scope_read_fails_closed: cross-scope get must fail");
    assert!(
        matches!(error, ObjectError::ScopeMismatch { .. } | ObjectError::NotFound),
        "cross_scope_read_fails_closed: must fail closed with ScopeMismatch or NotFound, got {error:?}"
    );
}

async fn missing_object_is_not_found(store: &dyn ObjectStore) {
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/missing.bin").expect("missing_object_is_not_found: key must be valid");

    let error = store.get(scope, key).await.expect_err("missing_object_is_not_found: get must fail");
    assert_eq!(error.code(), OBJECT_NOT_FOUND, "missing_object_is_not_found: must report object_not_found");
}

async fn delete_is_idempotent(store: &dyn ObjectStore) {
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/e.bin").expect("delete_is_idempotent: key must be valid");
    let metadata = test_metadata();

    store
        .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(deterministic_content())), metadata)
        .await
        .expect("delete_is_idempotent: put must succeed");
    store.delete(scope.clone(), key.clone()).await.expect("delete_is_idempotent: first delete must succeed");
    store.delete(scope.clone(), key.clone()).await.expect("delete_is_idempotent: second delete must succeed");

    let error = store
        .get(scope, key)
        .await
        .expect_err("delete_is_idempotent: get after delete must fail");
    assert_eq!(error.code(), OBJECT_NOT_FOUND, "delete_is_idempotent: deleted object must be not_found");
}

async fn list_pages_within_scope_only(store: &dyn ObjectStore) {
    let scope_a = test_scope("tenant-a");
    let scope_b = test_scope("tenant-b");
    let metadata = test_metadata();

    let mut expected_keys = Vec::new();
    for index in 0_u8..5 {
        let key = ObjectKey::try_new(format!("list/{index}.bin"))
            .expect("list_pages_within_scope_only: key must be valid");
        store
            .put(
                scope_a.clone(),
                key.clone(),
                PutPayload::Bytes(Bytes::from(deterministic_content())),
                metadata.clone(),
            )
            .await
            .expect("list_pages_within_scope_only: put in scope A must succeed");
        expected_keys.push(key);
    }

    let other_key =
        ObjectKey::try_new("list/other.bin").expect("list_pages_within_scope_only: key must be valid");
    store
        .put(scope_b, other_key, PutPayload::Bytes(Bytes::from(deterministic_content())), metadata)
        .await
        .expect("list_pages_within_scope_only: put in scope B must succeed");

    let prefix = Some(ObjectKey::try_new("list").expect("list_pages_within_scope_only: prefix must be valid"));
    let mut collected = Vec::new();
    let mut page = PageToken::first();
    loop {
        let result = store
            .list(scope_a.clone(), prefix.clone(), page)
            .await
            .expect("list_pages_within_scope_only: list must succeed");
        for entry in &result.entries {
            assert!(
                !collected.contains(&entry.key),
                "list_pages_within_scope_only: key {entry_key} must not repeat across pages",
                entry_key = entry.key.as_str()
            );
            assert_ne!(
                entry.key.as_str(),
                "list/other.bin",
                "list_pages_within_scope_only: scope B's object must never appear"
            );
            collected.push(entry.key.clone());
        }
        match result.next {
            Some(next) => page = next,
            None => break,
        }
    }

    let mut collected_sorted = collected;
    collected_sorted.sort();
    let mut expected_sorted = expected_keys;
    expected_sorted.sort();
    assert_eq!(
        collected_sorted, expected_sorted,
        "list_pages_within_scope_only: full walk must return exactly scope A's entries"
    );
}

/// Ceiling above which a synthetic oversize payload is impractical to
/// materialize in-process; backends with a larger ceiling skip this case.
const OVERSIZE_CEILING_PROXY_MAX: u64 = 64 * 1024 * 1024;

async fn oversize_put_is_rejected(store: &dyn ObjectStore) {
    let limits = store.limits();
    if limits.max_object_bytes >= OVERSIZE_CEILING_PROXY_MAX {
        // Materializing max_object_bytes + 1 in memory is impractical for
        // large-ceiling backends; the case is skipped rather than allocating
        // gigabytes just to exercise a size check.
        return;
    }

    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/oversize.bin").expect("oversize_put_is_rejected: key must be valid");
    let oversized_len = usize::try_from(limits.max_object_bytes + 1)
        .expect("oversize_put_is_rejected: oversized length must fit in usize");
    let metadata = test_metadata();

    let error = store
        .put(scope, key, PutPayload::Bytes(Bytes::from(vec![7_u8; oversized_len])), metadata)
        .await
        .expect_err("oversize_put_is_rejected: put over the ceiling must fail");
    assert_eq!(error.code(), OBJECT_TOO_LARGE, "oversize_put_is_rejected: must report object_too_large");
}

fn invalid_key_never_reaches_backend() {
    for bad in ["", "/abs", "a//b", "a/../b", "..", "a b"] {
        let error = ObjectKey::try_new(bad).expect_err("invalid_key_never_reaches_backend: key must be rejected");
        assert_eq!(
            error.code(),
            OBJECT_INVALID_KEY,
            "invalid_key_never_reaches_backend: {bad:?} must report object_invalid_key"
        );
    }
}

async fn presign_returns_url(store: &dyn ObjectStore) {
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/f.bin").expect("presign_returns_url: key must be valid");
    let metadata = test_metadata();

    store
        .put(scope.clone(), key.clone(), PutPayload::Bytes(Bytes::from(deterministic_content())), metadata)
        .await
        .expect("presign_returns_url: put must succeed");

    let presigned = store
        .presign_get(scope, key, Duration::from_mins(1))
        .await
        .expect("presign_returns_url: presign must succeed when supported");
    assert!(!presigned.url.is_empty(), "presign_returns_url: url must be non-empty");
    assert_eq!(presigned.expires_in_secs, 60, "presign_returns_url: expiry must echo the request");
}

async fn presign_is_unsupported(store: &dyn ObjectStore) {
    let scope = test_scope("tenant-a");
    let key = ObjectKey::try_new("docs/g.bin").expect("presign_is_unsupported: key must be valid");

    let error = store
        .presign_get(scope, key, Duration::from_mins(1))
        .await
        .expect_err("presign_is_unsupported: presign must fail when unsupported");
    assert_eq!(
        error.code(),
        OBJECT_UNSUPPORTED,
        "presign_is_unsupported: must report object_unsupported"
    );
}
