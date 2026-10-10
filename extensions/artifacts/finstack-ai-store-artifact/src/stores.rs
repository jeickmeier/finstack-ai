//! The public artifact stores, one per driver.
//!
//! Each is the shared artifact algorithm bound to one blob driver. The driver
//! itself stays private: an artifact store is what the runtime and every
//! toolset bind to, so it is the only storage contract this crate publishes.

use std::sync::Arc;

use finstack_ai_kernel::{ArtifactRef, BlobRef, Timestamp};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactRead,
    ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits,
};
use finstack_ai_runtime::ports::PortFuture;

use crate::artifact::ObjectArtifactStore;

macro_rules! artifact_store_over_driver {
    ($name:ident) => {
        impl ArtifactStore for $name {
            fn descriptor(&self) -> ArtifactStoreDescriptor {
                self.inner.descriptor()
            }

            fn stage_put(
                &self,
                scope: ArtifactScope,
                content: Bytes,
                metadata: ArtifactMetadata,
            ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
                self.inner.stage_put(scope, content, metadata)
            }

            fn get(
                &self,
                scope: ArtifactScope,
                artifact: ArtifactRef,
            ) -> PortFuture<Result<Bytes, ArtifactError>> {
                self.inner.get(scope, artifact)
            }

            fn get_by_blob(
                &self,
                scope: ArtifactScope,
                blob: BlobRef,
            ) -> PortFuture<Result<ArtifactRead, ArtifactError>> {
                self.inner.get_by_blob(scope, blob)
            }

            fn limits(&self) -> ArtifactStoreLimits {
                self.inner.limits()
            }

            fn pin(
                &self,
                scope: ArtifactScope,
                artifact: ArtifactRef,
                owner: ArtifactOwnerId,
            ) -> PortFuture<Result<(), ArtifactError>> {
                self.inner.pin(scope, artifact, owner)
            }

            fn unpin(
                &self,
                scope: ArtifactScope,
                artifact: ArtifactRef,
                owner: ArtifactOwnerId,
                now: Timestamp,
            ) -> PortFuture<Result<(), ArtifactError>> {
                self.inner.unpin(scope, artifact, owner, now)
            }

            fn collect_orphans(
                &self,
                scope: ArtifactScope,
                now: Timestamp,
                limit: usize,
            ) -> PortFuture<Result<ArtifactGcReport, ArtifactError>> {
                self.inner.collect_orphans(scope, now, limit)
            }
        }
    };
}

/// `ArtifactStore` backed by a local filesystem root.
#[cfg(feature = "local")]
pub struct LocalArtifactStore {
    inner: ObjectArtifactStore,
}

#[cfg(feature = "local")]
artifact_store_over_driver!(LocalArtifactStore);

#[cfg(feature = "local")]
impl LocalArtifactStore {
    /// Open an artifact store rooted at `root_dir`, creating it if needed.
    ///
    /// # Errors
    ///
    /// Returns an artifact error when `root_dir` cannot be created.
    pub fn try_new(root_dir: std::path::PathBuf) -> Result<Self, ArtifactError> {
        let driver = crate::local::LocalObjectStore::try_new(root_dir)
            .map_err(crate::artifact::map_object_error)?;
        Ok(Self {
            inner: ObjectArtifactStore::new(Arc::new(driver)),
        })
    }
}

#[cfg(feature = "local")]
impl LocalArtifactStore {
    /// Override the artifact byte ceiling.
    ///
    /// The value is still clamped to what the underlying storage accepts
    /// for a single object.
    #[must_use]
    pub fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
        self.inner = self.inner.with_max_artifact_bytes(max_artifact_bytes);
        self
    }
}

/// `ArtifactStore` backed by an S3-compatible bucket.
///
/// Supported backends are S3-compatible services that honour `If-None-Match:
/// *` on PUT and `If-Match` on PUT/DELETE with `412 Precondition Failed`
/// (AWS S3, `MinIO`, Garage). Constructors never touch the network; a backend
/// that ignores those preconditions is detected only by the conditional
/// mutations failing at use.
#[cfg(feature = "s3")]
pub struct S3ArtifactStore {
    inner: ObjectArtifactStore,
    driver: Arc<crate::s3::S3ObjectStore>,
    /// The caller's configured single-object ceiling: the hard upper bound
    /// for every artifact ceiling this store is later given.
    configured_max_object_bytes: u64,
}

#[cfg(feature = "s3")]
artifact_store_over_driver!(S3ArtifactStore);

#[cfg(feature = "s3")]
impl S3ArtifactStore {
    /// Open an artifact store over the bucket named by `config`.
    ///
    /// The driver's single-object ceiling is clamped to the artifact ceiling
    /// plus envelope-header slack, so a GET never buffers more than one
    /// artifact's worth of bytes regardless of the configured S3 limit.
    ///
    /// # Errors
    ///
    /// Returns an artifact error when the configuration is rejected.
    pub fn try_new(config: crate::s3::S3ObjectStoreConfig) -> Result<Self, ArtifactError> {
        let configured_max_object_bytes = config.max_object_bytes();
        let driver =
            crate::s3::S3ObjectStore::try_new(config).map_err(crate::artifact::map_object_error)?;
        let driver = Arc::new(
            driver.with_max_object_bytes(crate::artifact::clamp_object_ceiling(
                configured_max_object_bytes,
                crate::DEFAULT_MAX_ARTIFACT_BYTES,
            )),
        );
        Ok(Self {
            inner: ObjectArtifactStore::new(Arc::clone(&driver) as Arc<_>),
            driver,
            configured_max_object_bytes,
        })
    }
}

#[cfg(feature = "s3")]
impl S3ArtifactStore {
    /// Override the artifact byte ceiling.
    ///
    /// The value is still clamped to the configured S3 single-object limit;
    /// the driver's GET buffer bound follows the new ceiling.
    #[must_use]
    pub fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
        let driver = Arc::new(self.driver.with_max_object_bytes(
            crate::artifact::clamp_object_ceiling(
                self.configured_max_object_bytes,
                max_artifact_bytes,
            ),
        ));
        self.inner = ObjectArtifactStore::new(Arc::clone(&driver) as Arc<_>)
            .with_max_artifact_bytes(max_artifact_bytes);
        self.driver = driver;
        self
    }
}

#[cfg(all(test, feature = "s3"))]
mod tests {
    use super::S3ArtifactStore;
    use crate::artifact::ENVELOPE_HEADER_SLACK_BYTES;
    use crate::s3::S3ObjectStoreConfig;
    use finstack_ai_runtime::artifact::ArtifactStore;

    const MIB: u64 = 1024 * 1024;

    fn config() -> S3ObjectStoreConfig {
        S3ObjectStoreConfig::try_new("http://127.0.0.1:9000", "bucket", "garage").expect("config")
    }

    #[test]
    fn s3_object_ceiling_is_clamped_to_the_artifact_ceiling() {
        let store = S3ArtifactStore::try_new(config()).expect("store");
        assert_eq!(store.limits().max_artifact_bytes, 64 * 1024 * 1024);
        assert_eq!(
            store.inner.object_limits().max_object_bytes,
            64 * MIB + ENVELOPE_HEADER_SLACK_BYTES
        );

        let raised = store.with_max_artifact_bytes(128 * 1024 * 1024);
        assert_eq!(raised.limits().max_artifact_bytes, 128 * 1024 * 1024);
        assert_eq!(
            raised.inner.object_limits().max_object_bytes,
            128 * MIB + ENVELOPE_HEADER_SLACK_BYTES
        );
    }

    #[test]
    fn configured_s3_limit_remains_the_hard_bound() {
        let store = S3ArtifactStore::try_new(config().with_max_object_bytes(10 * MIB))
            .expect("store")
            .with_max_artifact_bytes(64 * 1024 * 1024);
        assert_eq!(store.inner.object_limits().max_object_bytes, 10 * MIB);
        assert_eq!(store.limits().max_artifact_bytes, 10 * 1024 * 1024);
    }
}
