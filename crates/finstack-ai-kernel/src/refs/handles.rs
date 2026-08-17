//! External handle and artifact references.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BlobRef, BoundedString, LABEL_MAX_BYTES};
use crate::primitives::ArtifactId;
use crate::primitives::Digest;
use crate::primitives::Metadata;

use super::error::{RefsError, validated_label};
use crate::primitives::ComponentId;
use crate::primitives::RawJson;

/// Non-secret external handle for deferred effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalHandleRef {
    provider: ComponentId,
    handle: Arc<str>,
    reconciliation_metadata: RawJson,
}

impl ExternalHandleRef {
    /// Construct an external handle reference.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `handle` fails label rules.
    pub fn try_new(
        provider: ComponentId,
        handle: impl AsRef<str>,
        reconciliation_metadata: RawJson,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            provider,
            handle: validated_label(handle.as_ref(), "handle")?,
            reconciliation_metadata,
        })
    }

    /// Borrow the provider component id.
    #[must_use]
    pub fn provider(&self) -> &ComponentId {
        &self.provider
    }

    /// Borrow the opaque handle.
    #[must_use]
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Borrow reconciliation metadata.
    #[must_use]
    pub fn reconciliation_metadata(&self) -> &RawJson {
        &self.reconciliation_metadata
    }
}

impl<'de> Deserialize<'de> for ExternalHandleRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            provider: ComponentId,
            handle: BoundedString<LABEL_MAX_BYTES>,
            reconciliation_metadata: RawJson,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.provider,
            wire.handle.into_inner(),
            wire.reconciliation_metadata,
        )
        .map_err(de::Error::custom)
    }
}

/// Digest-bearing artifact reference for replay-required content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactRef {
    id: ArtifactId,
    kind: Arc<str>,
    blob: BlobRef,
    content_digest: Digest,
    scope_digest: Digest,
    metadata: Metadata,
}

impl ArtifactRef {
    /// Construct an artifact reference.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `kind` fails label rules.
    pub fn try_new(
        id: ArtifactId,
        kind: impl AsRef<str>,
        blob: BlobRef,
        content_digest: Digest,
        scope_digest: Digest,
        metadata: Metadata,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            id,
            kind: validated_label(kind.as_ref(), "kind")?,
            blob,
            content_digest,
            scope_digest,
            metadata,
        })
    }

    /// Borrow the artifact id.
    #[must_use]
    pub fn id(&self) -> ArtifactId {
        self.id
    }

    /// Borrow the kind label.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Borrow the blob reference.
    #[must_use]
    pub fn blob(&self) -> &BlobRef {
        &self.blob
    }

    /// Borrow the content digest.
    #[must_use]
    pub fn content_digest(&self) -> Digest {
        self.content_digest
    }

    /// Borrow the scope digest.
    #[must_use]
    pub fn scope_digest(&self) -> Digest {
        self.scope_digest
    }

    /// Borrow metadata.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

impl<'de> Deserialize<'de> for ArtifactRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: ArtifactId,
            kind: BoundedString<LABEL_MAX_BYTES>,
            blob: BlobRef,
            content_digest: Digest,
            scope_digest: Digest,
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.id,
            wire.kind.into_inner(),
            wire.blob,
            wire.content_digest,
            wire.scope_digest,
            wire.metadata,
        )
        .map_err(de::Error::custom)
    }
}
