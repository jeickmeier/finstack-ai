//! In-memory [`ObjectDriver`] fake used by the artifact algorithm tests.
//!
//! Test-only: the real drivers are exercised through the public
//! [`LocalArtifactStore`](crate::LocalArtifactStore) surface.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::driver::{
    ObjectDriver, ObjectEntry, ObjectError, ObjectKey, ObjectMetadata, ObjectPage, ObjectRef,
    ObjectScope, ObjectStoreLimits, PageToken, PutPayload, physical_object_key,
    validate_object_metadata,
};
use finstack_ai_kernel::Digest;
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::ports::PortFuture;

/// Number of entries returned per [`FakeObjectDriver::list`] page.
///
/// Deliberately tiny so the contract suite is forced to exercise pagination.
const LIST_PAGE_SIZE: usize = 2;

struct StoredObject {
    scope_digest: Digest,
    content: Bytes,
    content_digest: Digest,
    media_type: Arc<str>,
}

fn prepare_object(
    limits: ObjectStoreLimits,
    scope: &ObjectScope,
    key: ObjectKey,
    content: PutPayload,
    metadata: ObjectMetadata,
) -> Result<(String, StoredObject, ObjectRef), ObjectError> {
    validate_object_metadata(&metadata)?;
    let scope_digest = scope.digest()?;
    let bytes = match content {
        PutPayload::Bytes(bytes) => bytes,
        PutPayload::File(path) => {
            let data = std::fs::read(&path).map_err(|error| ObjectError::Io {
                message: Arc::from(error.to_string()),
            })?;
            Bytes::from(data)
        }
    };
    let length = u64::try_from(bytes.len()).map_err(|_error| ObjectError::Io {
        message: Arc::from("length_overflow"),
    })?;
    if length > limits.max_object_bytes {
        return Err(ObjectError::TooLarge {
            len: length,
            max: limits.max_object_bytes,
        });
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
        media_type: metadata.media_type,
    };
    Ok((physical, stored, object_ref))
}

/// In-memory [`ObjectDriver`] fake intended for unit and contract tests.
///
/// Objects are keyed by [`physical_object_key`] with no key prefix, exactly
/// as the S3 and local-filesystem backends will key theirs. Presign support
/// is unconditional (returns an opaque `fake://` URL) so this fake also
/// exercises the presign-supported half of the contract suite.
pub struct FakeObjectDriver {
    limits: ObjectStoreLimits,
    objects: Mutex<BTreeMap<String, StoredObject>>,
    poison_next_get: AtomicBool,
}

impl Default for FakeObjectDriver {
    fn default() -> Self {
        Self {
            limits: ObjectStoreLimits::default(),
            objects: Mutex::new(BTreeMap::new()),
            poison_next_get: AtomicBool::new(false),
        }
    }
}

impl FakeObjectDriver {
    /// Poison the next `get`/`get_to_file` call so it returns
    /// [`ObjectError::Integrity`] instead of the stored content.
    ///
    /// Used by the injected-failure test outside the shared contract suite
    /// (backends cannot be made to corrupt their own content on demand, so
    /// this is fake-only surface, not part of the trait contract).
    pub fn fail_next_get_with_integrity(&self) {
        self.poison_next_get.store(true, Ordering::SeqCst);
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, StoredObject>>, ObjectError> {
        self.objects
            .lock()
            .map_err(|_error| ObjectError::Unavailable {
                message: Arc::from("poisoned_lock"),
            })
    }
}

impl ObjectDriver for FakeObjectDriver {
    fn put(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: PutPayload,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let result = (|| {
            let (physical, stored, object_ref) =
                prepare_object(self.limits, &scope, key, content, metadata)?;
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
                return Err(ObjectError::ScopeMismatch {
                    expected: scope_digest,
                    actual: stored.scope_digest,
                });
            }
            if poisoned {
                return Err(ObjectError::Integrity {
                    message: Arc::from("injected_failure"),
                });
            }
            if Digest::blob_content(&stored.content) != stored.content_digest {
                return Err(ObjectError::Integrity {
                    message: Arc::from("digest_mismatch"),
                });
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
                return Err(ObjectError::ScopeMismatch {
                    expected: scope_digest,
                    actual: stored.scope_digest,
                });
            }
            std::fs::write(&dest, &stored.content).map_err(|error| ObjectError::Io {
                message: Arc::from(error.to_string()),
            })?;
            if poisoned {
                return Err(ObjectError::Integrity {
                    message: Arc::from("injected_failure"),
                });
            }
            if Digest::blob_content(&stored.content) != stored.content_digest {
                return Err(ObjectError::Integrity {
                    message: Arc::from("digest_mismatch"),
                });
            }
            let length = u64::try_from(stored.content.len()).map_err(|_error| ObjectError::Io {
                message: Arc::from("length_overflow"),
            })?;
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

    fn head(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let result = (|| {
            let scope_digest = scope.digest()?;
            let physical = physical_object_key(None, &scope_digest, &key);
            let objects = self.lock()?;
            let stored = objects.get(&physical).ok_or(ObjectError::NotFound)?;
            if stored.scope_digest != scope_digest {
                return Err(ObjectError::ScopeMismatch {
                    expected: scope_digest,
                    actual: stored.scope_digest,
                });
            }
            let length = u64::try_from(stored.content.len()).map_err(|_error| ObjectError::Io {
                message: Arc::from("length_overflow"),
            })?;
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

    fn put_if_absent(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: PutPayload,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let result = (|| {
            let (physical, stored, object_ref) =
                prepare_object(self.limits, &scope, key, content, metadata)?;
            let mut objects = self.lock()?;
            if objects.contains_key(&physical) {
                return Err(ObjectError::Conflict);
            }
            objects.insert(physical, stored);
            Ok(object_ref)
        })();
        Box::pin(async move { result })
    }

    fn replace_if_digest(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expected: Digest,
        content: PutPayload,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let result = (|| {
            let (physical, stored, object_ref) =
                prepare_object(self.limits, &scope, key, content, metadata)?;
            let mut objects = self.lock()?;
            if objects.get(&physical).map(|value| value.content_digest) != Some(expected) {
                return Err(ObjectError::Conflict);
            }
            objects.insert(physical, stored);
            Ok(object_ref)
        })();
        Box::pin(async move { result })
    }

    fn delete_if_digest(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expected: Digest,
    ) -> PortFuture<Result<(), ObjectError>> {
        let result = (|| {
            let scope_digest = scope.digest()?;
            let physical = physical_object_key(None, &scope_digest, &key);
            let mut objects = self.lock()?;
            if objects.get(&physical).map(|value| value.content_digest) != Some(expected) {
                return Err(ObjectError::Conflict);
            }
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
            let scope_prefix = format!("{hex}/");
            let prefix_str = prefix.as_ref().map(|value| value.as_str().to_string());

            let matching: Vec<(String, u64)> = {
                let objects = self.lock()?;
                objects
                    .iter()
                    .filter(|(physical, _)| physical.starts_with(&scope_prefix))
                    .filter(|(physical, _)| {
                        let logical = &physical[scope_prefix.len()..];
                        prefix_str
                            .as_deref()
                            .is_none_or(|prefix| logical.starts_with(prefix))
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
                let key = ObjectKey::try_new(logical).map_err(|_error| ObjectError::Io {
                    message: Arc::from("corrupt_physical_key"),
                })?;
                entries.push(ObjectEntry {
                    key,
                    length: *length,
                });
                last_physical = Some(physical.clone());
            }

            let consumed = start_index + entries.len();
            let next = if consumed < matching.len() {
                last_physical.map(PageToken::opaque)
            } else {
                None
            };

            Ok(ObjectPage { entries, next })
        })();
        Box::pin(async move { result })
    }

    fn limits(&self) -> ObjectStoreLimits {
        self.limits
    }
}

// ---------------------------------------------------------------------------
// Shared contract suite
// ---------------------------------------------------------------------------
