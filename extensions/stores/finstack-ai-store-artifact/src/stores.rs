//! The public artifact stores, one per driver.
//!
//! Each is the shared artifact algorithm bound to one blob driver. The driver
//! itself stays private: an artifact store is what the runtime and every
//! toolset bind to, so it is the only storage contract this crate publishes.

use std::sync::Arc;

use finstack_ai_kernel::{ArtifactRef, BlobRef, Timestamp};
use finstack_ai_runtime::{
    ArtifactError, ArtifactGcReport, ArtifactMetadata, ArtifactOwnerId, ArtifactRead,
    ArtifactScope, ArtifactStore, ArtifactStoreDescriptor, ArtifactStoreLimits, Bytes, PortFuture,
};

use crate::artifact::ObjectArtifactStore;

macro_rules! artifact_store_over_driver {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        pub struct $name {
            inner: ObjectArtifactStore,
        }

        impl $name {
            /// Override the artifact byte ceiling.
            ///
            /// The value is still clamped to what the underlying storage
            /// accepts for a single object.
            #[must_use]
            pub fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
                self.inner = self.inner.with_max_artifact_bytes(max_artifact_bytes);
                self
            }
        }

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

#[cfg(feature = "local")]
artifact_store_over_driver!(
    LocalArtifactStore,
    "`ArtifactStore` backed by a local filesystem root."
);

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

#[cfg(feature = "s3")]
artifact_store_over_driver!(
    S3ArtifactStore,
    "`ArtifactStore` backed by an S3-compatible bucket."
);

#[cfg(feature = "s3")]
impl S3ArtifactStore {
    /// Open an artifact store over the bucket named by `config`.
    ///
    /// # Errors
    ///
    /// Returns an artifact error when the configuration is rejected.
    pub fn try_new(config: crate::s3::S3ObjectStoreConfig) -> Result<Self, ArtifactError> {
        let driver =
            crate::s3::S3ObjectStore::try_new(config).map_err(crate::artifact::map_object_error)?;
        Ok(Self {
            inner: ObjectArtifactStore::new(Arc::new(driver)),
        })
    }
}
