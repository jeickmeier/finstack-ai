//! Blob and media references.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::Digest;

use super::BoundedString;
use super::error::ContentError;
use super::text::{LABEL_MAX_BYTES, validated_label};
use serde::Deserializer;

/// Reference to externally stored media bytes.
///
/// The kernel never dereferences a blob. Large content is represented by this
/// reference rather than inline payload bytes.
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::BlobRef;
///
/// let blob = BlobRef::try_new(
///     "blob-1",
///     "image/png",
///     1_048_576,
///     None,
///     Some("diagram.png"),
/// )
/// .expect("blob");
/// assert_eq!(blob.length(), 1_048_576);
/// assert!(blob.name().is_some());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct BlobRef {
    id: Arc<str>,
    media_type: Arc<str>,
    length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<Arc<str>>,
}

impl BlobRef {
    /// Construct a validated blob reference.
    ///
    /// # Arguments
    ///
    /// * `id` - Caller-assigned blob identity label.
    /// * `media_type` - Media-type label such as `image/png`.
    /// * `length` - Declared payload length in bytes. The kernel does not fetch
    ///   the bytes.
    /// * `digest` - Optional content digest; `None` when the host has not hashed
    ///   the payload.
    /// * `name` - Optional display name; `None` omits it.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when labels are empty, oversized, or contain NUL.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::BlobRef;
    ///
    /// let blob = BlobRef::try_new("blob-1", "image/png", 1024, None, Some("diagram.png"))
    ///     .expect("blob");
    /// assert_eq!(blob.length(), 1024);
    /// ```
    pub fn try_new(
        id: impl AsRef<str>,
        media_type: impl AsRef<str>,
        length: u64,
        digest: Option<Digest>,
        name: Option<impl AsRef<str>>,
    ) -> Result<Self, ContentError> {
        let id = validated_label(id.as_ref(), "id")?;
        let media_type = validated_label(media_type.as_ref(), "media_type")?;
        let name = match name {
            Some(value) => Some(validated_label(value.as_ref(), "name")?),
            None => None,
        };
        Ok(Self {
            id,
            media_type,
            length,
            digest,
            name,
        })
    }

    /// Borrow the opaque blob identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Borrow the media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Declared content length in bytes.
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    /// Optional integrity digest over exact blob bytes (`blob-content` domain).
    #[must_use]
    pub const fn digest(&self) -> Option<&Digest> {
        self.digest.as_ref()
    }

    /// Optional display name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: BoundedString<LABEL_MAX_BYTES>,
            media_type: BoundedString<LABEL_MAX_BYTES>,
            length: u64,
            #[serde(default)]
            digest: Option<Digest>,
            #[serde(default)]
            name: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.id.into_inner(),
            wire.media_type.into_inner(),
            wire.length,
            wire.digest,
            wire.name.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}

/// Media content referenced by [`BlobRef`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaRef {
    blob: BlobRef,
}

impl MediaRef {
    /// Wrap a blob reference as media content.
    #[must_use]
    pub fn new(blob: BlobRef) -> Self {
        Self { blob }
    }

    /// Borrow the blob reference.
    #[must_use]
    pub const fn blob(&self) -> &BlobRef {
        &self.blob
    }
}
