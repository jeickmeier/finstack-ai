//! In-memory [`ObjectDriver`] fake used by the artifact algorithm tests.
//!
//! Test-only: the real drivers are exercised through the public
//! [`LocalArtifactStore`](crate::LocalArtifactStore) surface.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::driver::{
    ObjectDriver, ObjectEntry, ObjectError, ObjectKey, ObjectMetadata, ObjectPage, ObjectRef,
    ObjectScope, ObjectStoreLimits, PageToken, physical_object_key, validate_object_metadata,
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
    content: Bytes,
    metadata: ObjectMetadata,
) -> Result<(String, StoredObject, ObjectRef), ObjectError> {
    validate_object_metadata(&metadata)?;
    let scope_digest = scope.digest()?;
    let bytes = content;
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
/// as the S3 backend keys its own.
pub struct FakeObjectDriver {
    limits: ObjectStoreLimits,
    objects: Mutex<BTreeMap<String, StoredObject>>,
    poison_next_get: AtomicBool,
    list_calls: AtomicUsize,
}

impl Default for FakeObjectDriver {
    fn default() -> Self {
        Self {
            limits: ObjectStoreLimits::default(),
            objects: Mutex::new(BTreeMap::new()),
            poison_next_get: AtomicBool::new(false),
            list_calls: AtomicUsize::new(0),
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

    /// Number of `list` calls so far: lets tests prove an index lookup
    /// never fell back to the listing scan.
    pub fn list_calls(&self) -> usize {
        self.list_calls.load(Ordering::SeqCst)
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
        content: Bytes,
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
            Ok(ObjectRef {
                key,
                scope_digest,
                content_digest: stored.content_digest,
                length: stored.content.len() as u64,
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
        content: Bytes,
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
        content: Bytes,
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
        self.list_calls.fetch_add(1, Ordering::SeqCst);
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

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{Metadata, Sensitivity};

    fn scope() -> ObjectScope {
        ObjectScope {
            tenant_scope: Arc::from("tenant-a"),
            session_id: None,
            run_id: None,
            sensitivity: Sensitivity::Internal,
        }
    }

    fn metadata() -> ObjectMetadata {
        ObjectMetadata {
            media_type: Arc::from("application/octet-stream"),
            name: None,
            attributes: Metadata::empty(),
        }
    }

    async fn seeded() -> (FakeObjectDriver, ObjectKey) {
        let driver = FakeObjectDriver::default();
        let key = ObjectKey::try_new("conditional").expect("key");
        driver
            .put(scope(), key.clone(), Bytes::from_static(b"v1"), metadata())
            .await
            .expect("seed");
        (driver, key)
    }

    #[tokio::test]
    async fn replace_if_digest_mismatch_is_conflict() {
        let (driver, key) = seeded().await;
        let wrong = Digest::blob_content(b"not v1");
        let error = driver
            .replace_if_digest(
                scope(),
                key.clone(),
                wrong,
                Bytes::from_static(b"v2"),
                metadata(),
            )
            .await
            .expect_err("must conflict");
        assert_eq!(error, ObjectError::Conflict);
        assert_eq!(
            driver.get(scope(), key.clone()).await.expect("get"),
            Bytes::from_static(b"v1")
        );
        driver
            .replace_if_digest(
                scope(),
                key,
                Digest::blob_content(b"v1"),
                Bytes::from_static(b"v2"),
                metadata(),
            )
            .await
            .expect("matching digest replaces");
    }

    #[tokio::test]
    async fn delete_if_digest_mismatch_is_conflict() {
        let (driver, key) = seeded().await;
        let error = driver
            .delete_if_digest(scope(), key.clone(), Digest::blob_content(b"not v1"))
            .await
            .expect_err("must conflict");
        assert_eq!(error, ObjectError::Conflict);
        driver
            .get(scope(), key.clone())
            .await
            .expect("still stored");
        driver
            .delete_if_digest(scope(), key.clone(), Digest::blob_content(b"v1"))
            .await
            .expect("matching digest deletes");
        assert_eq!(
            driver.get(scope(), key).await.expect_err("gone"),
            ObjectError::NotFound
        );
    }

    #[tokio::test]
    async fn put_if_absent_existing_is_conflict() {
        let (driver, key) = seeded().await;
        let error = driver
            .put_if_absent(scope(), key.clone(), Bytes::from_static(b"v2"), metadata())
            .await
            .expect_err("must conflict");
        assert_eq!(error, ObjectError::Conflict);
        assert_eq!(
            driver.get(scope(), key).await.expect("get"),
            Bytes::from_static(b"v1")
        );
    }
}
