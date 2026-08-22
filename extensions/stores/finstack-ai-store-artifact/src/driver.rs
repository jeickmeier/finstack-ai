//! Private blob-driver contract shared by this crate's storage drivers.
//!
//! This is deliberately not public API. Artifact storage is the one public
//! storage concept (`ArtifactStore`); the driver exists only so the artifact
//! algorithm in `artifact.rs` -- key scheme, envelope, pin and orphan GC --
//! is written once over both the S3 and local drivers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use finstack_ai_kernel::{Digest, Metadata, RunId, Sensitivity, SessionId};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::ports::{PortFuture, PortObject};

/// Stable code for an unavailable object service.
pub(crate) const OBJECT_UNAVAILABLE: &str = "object_unavailable";
/// Stable code for a missing object.
pub(crate) const OBJECT_NOT_FOUND: &str = "object_not_found";
/// Stable code for a scope-binding failure.
pub(crate) const OBJECT_SCOPE_MISMATCH: &str = "object_scope_mismatch";
/// Stable code for a content integrity failure.
pub(crate) const OBJECT_INTEGRITY_FAILURE: &str = "object_integrity_failure";
/// Stable code for rejected rather than truncated oversized content.
pub(crate) const OBJECT_TOO_LARGE: &str = "object_too_large";
/// Stable code for a malformed logical key.
pub(crate) const OBJECT_INVALID_KEY: &str = "object_invalid_key";
/// Stable code for malformed object metadata.
pub(crate) const OBJECT_INVALID_METADATA: &str = "object_invalid_metadata";
/// Stable code for an operation the backend does not support.
pub(crate) const OBJECT_UNSUPPORTED: &str = "object_unsupported";
/// Stable code for a local I/O failure.
pub(crate) const OBJECT_IO_FAILURE: &str = "object_io_failure";
/// Stable code for a failed conditional object mutation.
pub(crate) const OBJECT_CONFLICT: &str = "object_conflict";

/// Maximum logical key length in bytes.
pub(crate) const MAX_OBJECT_KEY_BYTES: usize = 512;

/// Exact authorization and integrity scope for an object operation.
///
/// Unlike [`crate::ArtifactScope`], `session_id` is optional so objects may
/// outlive a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectScope {
    /// Authenticated tenant scope, never a bearer credential.
    pub tenant_scope: Arc<str>,
    /// Owning session when session-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Owning run when run-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Required sensitivity classification.
    pub sensitivity: Sensitivity,
}

impl ObjectScope {
    /// Validate and hash the exact scope binding.
    ///
    /// # Errors
    ///
    /// Returns a metadata error for empty/NUL tenant scope or failed
    /// canonicalization.
    pub fn digest(&self) -> Result<Digest, ObjectError> {
        if self.tenant_scope.is_empty() || self.tenant_scope.as_bytes().contains(&0) {
            return Err(ObjectError::InvalidMetadata {
                message: Arc::from("invalid_tenant_scope"),
            });
        }
        let canonical = serde_json_canonicalizer::to_vec(self).map_err(|error| {
            ObjectError::InvalidMetadata {
                message: Arc::from(error.to_string()),
            }
        })?;
        Digest::domain_separated("object-scope", 1, &canonical).map_err(|error| {
            ObjectError::InvalidMetadata {
                message: Arc::from(error.to_string()),
            }
        })
    }
}

/// Validated logical object key: `/`-joined segments of `[A-Za-z0-9._-]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "Arc<str>", into = "Arc<str>")]
pub(crate) struct ObjectKey(Arc<str>);

impl ObjectKey {
    /// Validate a logical key.
    ///
    /// # Errors
    ///
    /// Rejects empty keys, keys over [`MAX_OBJECT_KEY_BYTES`], leading `/`,
    /// empty segments, `.`/`..` segments, and characters outside
    /// `[A-Za-z0-9._-]`.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, ObjectError> {
        let value = value.as_ref();
        let invalid = |message: &str| ObjectError::InvalidKey {
            message: Arc::from(message),
        };
        if value.is_empty() || value.len() > MAX_OBJECT_KEY_BYTES {
            return Err(invalid("key_length_out_of_range"));
        }
        for segment in value.split('/') {
            if segment.is_empty() {
                return Err(invalid("empty_key_segment"));
            }
            if segment == "." || segment == ".." {
                return Err(invalid("relative_key_segment"));
            }
            if !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(invalid("key_charset"));
            }
        }
        Ok(Self(Arc::from(value)))
    }

    /// Borrow the validated key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<Arc<str>> for ObjectKey {
    type Error = ObjectError;
    fn try_from(value: Arc<str>) -> Result<Self, Self::Error> {
        Self::try_new(value.as_ref())
    }
}

impl From<ObjectKey> for Arc<str> {
    fn from(key: ObjectKey) -> Self {
        key.0
    }
}

/// Payload for a put: in-memory bytes or a streamed local file.
#[derive(Debug, Clone)]
pub(crate) enum PutPayload {
    /// Fully materialized content.
    Bytes(Bytes),
    /// Content streamed from a local file; never fully materialized.
    File(PathBuf),
}

/// Exact metadata mapped to the returned [`ObjectRef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectMetadata {
    /// Object media type.
    pub media_type: Arc<str>,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Arc<str>>,
    /// Bounded non-secret, non-authoritative attributes.
    pub attributes: Metadata,
}

/// Reference to one stored object; carries scope binding and integrity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectRef {
    /// Logical key within the scope.
    pub key: ObjectKey,
    /// Frozen scope binding.
    pub scope_digest: Digest,
    /// SHA-256 content digest.
    pub content_digest: Digest,
    /// Content length in bytes.
    pub length: u64,
    /// Object media type.
    pub media_type: Arc<str>,
}

/// One listing entry; S3 listings carry no user metadata, so neither does this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectEntry {
    /// Logical key within the scope.
    pub key: ObjectKey,
    /// Content length in bytes.
    pub length: u64,
}

/// Backend-opaque pagination cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageToken(
    #[serde(default, skip_serializing_if = "Option::is_none")] Option<Arc<str>>,
);

impl PageToken {
    /// First page.
    #[must_use]
    pub const fn first() -> Self {
        Self(None)
    }
    /// Continuation from a backend-provided cursor.
    #[must_use]
    pub fn opaque(value: impl AsRef<str>) -> Self {
        Self(Some(Arc::from(value.as_ref())))
    }
    /// Backend cursor, if continuing.
    #[must_use]
    pub fn value(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// One page of listing results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectPage {
    /// Entries in this page.
    pub entries: Vec<ObjectEntry>,
    /// Cursor for the next page, when more results exist.
    pub next: Option<PageToken>,
}

/// Time-limited download URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PresignedUrl {
    /// Complete presigned URL.
    pub url: Arc<str>,
    /// Requested validity window in seconds.
    pub expires_in_secs: u64,
}

/// Per-store object size ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObjectStoreLimits {
    /// Reject puts above this size; never truncate.
    pub max_object_bytes: u64,
}

impl Default for ObjectStoreLimits {
    fn default() -> Self {
        // S3 single-PUT protocol ceiling; the cap until multipart lands.
        Self {
            max_object_bytes: 5 * 1024 * 1024 * 1024,
        }
    }
}

/// Scoped host-supplied unstructured object service.
pub(crate) trait ObjectDriver: PortObject {
    /// Durably store exact content under the caller's scope.
    fn put(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: PutPayload,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>>;

    /// Read exact bytes; verifies content digest before returning.
    fn get(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<Bytes, ObjectError>>;

    /// Stream the object to `dest`; verifies content digest after writing.
    fn get_to_file(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        dest: PathBuf,
    ) -> PortFuture<Result<ObjectRef, ObjectError>>;

    /// Fetch the reference without content.
    fn head(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
    ) -> PortFuture<Result<ObjectRef, ObjectError>>;

    /// Delete one object; deleting a missing object is not an error.
    fn delete(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<(), ObjectError>>;

    /// Store only when no object exists at the exact scoped key.
    fn put_if_absent(
        &self,
        _scope: ObjectScope,
        _key: ObjectKey,
        _content: PutPayload,
        _metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        Box::pin(async {
            Err(ObjectError::Unsupported {
                operation: Arc::from("put_if_absent"),
            })
        })
    }

    /// Replace only when the currently stored content digest equals `expected`.
    fn replace_if_digest(
        &self,
        _scope: ObjectScope,
        _key: ObjectKey,
        _expected: Digest,
        _content: PutPayload,
        _metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        Box::pin(async {
            Err(ObjectError::Unsupported {
                operation: Arc::from("replace_if_digest"),
            })
        })
    }

    /// Delete only when the currently stored content digest equals `expected`.
    fn delete_if_digest(
        &self,
        _scope: ObjectScope,
        _key: ObjectKey,
        _expected: Digest,
    ) -> PortFuture<Result<(), ObjectError>> {
        Box::pin(async {
            Err(ObjectError::Unsupported {
                operation: Arc::from("delete_if_digest"),
            })
        })
    }

    /// List keys within the caller's scope, optionally under a prefix.
    fn list(
        &self,
        scope: ObjectScope,
        prefix: Option<ObjectKey>,
        page: PageToken,
    ) -> PortFuture<Result<ObjectPage, ObjectError>>;

    /// Produce a time-limited download URL when the backend supports it.
    fn presign_get(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expiry: Duration,
    ) -> PortFuture<Result<PresignedUrl, ObjectError>>;

    /// This store's size ceilings.
    fn limits(&self) -> ObjectStoreLimits {
        ObjectStoreLimits::default()
    }
}

/// Compose the physical backend key: `{prefix}/{scope-digest-hex}/{key}`.
#[must_use]
pub(crate) fn physical_object_key(
    key_prefix: Option<&str>,
    scope_digest: &Digest,
    key: &ObjectKey,
) -> String {
    let hex = scope_digest.to_hex();
    match key_prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{hex}/{}", key.as_str()),
        _ => format!("{hex}/{}", key.as_str()),
    }
}

/// Validate bounded, non-secret object metadata.
///
/// # Errors
///
/// Rejects empty or NUL-bearing media type and name fields.
pub(crate) fn validate_object_metadata(metadata: &ObjectMetadata) -> Result<(), ObjectError> {
    if metadata.media_type.is_empty()
        || metadata.media_type.as_bytes().contains(&0)
        || metadata
            .name
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.as_bytes().contains(&0))
    {
        return Err(ObjectError::InvalidMetadata {
            message: Arc::from("metadata_field_invalid"),
        });
    }
    Ok(())
}

/// Object service, integrity, or local I/O failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum ObjectError {
    /// Service unavailable (transport, auth, or server failure).
    #[error("{}: {message}", OBJECT_UNAVAILABLE)]
    Unavailable {
        /// Bounded non-secret diagnostic.
        message: Arc<str>,
    },
    /// Object is missing.
    #[error("{}: object is missing", OBJECT_NOT_FOUND)]
    NotFound,
    /// A conditional mutation observed a different current object.
    #[error("{}: conditional mutation precondition failed", OBJECT_CONFLICT)]
    Conflict,
    /// Requested scope differs from the object's frozen binding.
    #[error(
        "{}: expected scope {expected}, actual scope {actual}",
        OBJECT_SCOPE_MISMATCH
    )]
    ScopeMismatch {
        /// Requested scope digest.
        expected: Digest,
        /// Stored scope digest.
        actual: Digest,
    },
    /// Content digest verification failed.
    #[error("{}: {message}", OBJECT_INTEGRITY_FAILURE)]
    Integrity {
        /// Stable diagnostic.
        message: Arc<str>,
    },
    /// Content exceeds this store's ceiling.
    #[error("{}: object has {len} bytes; maximum is {max}", OBJECT_TOO_LARGE)]
    TooLarge {
        /// Submitted bytes.
        len: u64,
        /// Maximum bytes.
        max: u64,
    },
    /// Logical key is malformed.
    #[error("{}: {message}", OBJECT_INVALID_KEY)]
    InvalidKey {
        /// Stable diagnostic.
        message: Arc<str>,
    },
    /// Metadata is malformed.
    #[error("{}: {message}", OBJECT_INVALID_METADATA)]
    InvalidMetadata {
        /// Stable diagnostic.
        message: Arc<str>,
    },
    /// Backend does not support this operation.
    #[error("{}: {operation} is not supported by this backend", OBJECT_UNSUPPORTED)]
    Unsupported {
        /// Operation name.
        operation: Arc<str>,
    },
    /// Local filesystem I/O failed.
    #[error("{}: {message}", OBJECT_IO_FAILURE)]
    Io {
        /// Bounded non-secret diagnostic.
        message: Arc<str>,
    },
}

impl ObjectError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => OBJECT_UNAVAILABLE,
            Self::NotFound => OBJECT_NOT_FOUND,
            Self::Conflict => OBJECT_CONFLICT,
            Self::ScopeMismatch { .. } => OBJECT_SCOPE_MISMATCH,
            Self::Integrity { .. } => OBJECT_INTEGRITY_FAILURE,
            Self::TooLarge { .. } => OBJECT_TOO_LARGE,
            Self::InvalidKey { .. } => OBJECT_INVALID_KEY,
            Self::InvalidMetadata { .. } => OBJECT_INVALID_METADATA,
            Self::Unsupported { .. } => OBJECT_UNSUPPORTED,
            Self::Io { .. } => OBJECT_IO_FAILURE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ObjectScope {
        ObjectScope {
            tenant_scope: Arc::from("tenant-a"),
            session_id: None,
            run_id: None,
            sensitivity: Sensitivity::Internal,
        }
    }

    #[test]
    fn scope_digest_is_stable_and_session_optional() {
        let digest_a = scope().digest().expect("digest");
        let digest_b = scope().digest().expect("digest");
        assert_eq!(digest_a, digest_b);
        let mut with_session = scope();
        with_session.session_id = Some(SessionId::from_bytes([1; 16]));
        assert_ne!(digest_a, with_session.digest().expect("digest"));
    }

    #[test]
    fn empty_or_nul_tenant_scope_is_rejected() {
        let mut bad = scope();
        bad.tenant_scope = Arc::from("");
        assert_eq!(
            bad.digest().expect_err("must fail").code(),
            OBJECT_INVALID_METADATA
        );
        let mut nul = scope();
        nul.tenant_scope = Arc::from("a\0b");
        assert_eq!(
            nul.digest().expect_err("must fail").code(),
            OBJECT_INVALID_METADATA
        );
    }

    #[test]
    fn object_key_accepts_segmented_names() {
        let key = ObjectKey::try_new("reports/2026/q3.pdf").expect("valid key");
        assert_eq!(key.as_str(), "reports/2026/q3.pdf");
    }

    #[test]
    fn object_key_rejects_traversal_absolute_empty_and_bad_chars() {
        for bad in [
            "",
            "/abs",
            "a//b",
            "a/../b",
            "..",
            "a b",
            "ключ",
            &"x".repeat(513),
        ] {
            let error = ObjectKey::try_new(bad).expect_err("must reject");
            assert_eq!(error.code(), OBJECT_INVALID_KEY);
        }
    }

    #[test]
    fn physical_key_layout_prefixes_scope_digest() {
        let digest = scope().digest().expect("digest");
        let key = ObjectKey::try_new("doc.pdf").expect("key");
        let physical = physical_object_key(Some("finstack"), &digest, &key);
        let hex = digest.to_hex();
        assert_eq!(physical, format!("finstack/{hex}/doc.pdf"));
        assert_eq!(
            physical_object_key(None, &digest, &key),
            format!("{hex}/doc.pdf")
        );
    }

    #[test]
    fn limits_default_is_five_gib() {
        assert_eq!(
            ObjectStoreLimits::default().max_object_bytes,
            5 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn error_codes_are_frozen() {
        assert_eq!(ObjectError::NotFound.code(), "object_not_found");
        assert_eq!(
            ObjectError::Unsupported {
                operation: Arc::from("presign_get")
            }
            .code(),
            "object_unsupported"
        );
    }

    #[test]
    fn metadata_validation_rejects_empty_and_nul_fields() {
        let bad = ObjectMetadata {
            media_type: Arc::from(""),
            name: None,
            attributes: Metadata::parse(b"{}").expect("metadata"),
        };
        assert_eq!(
            validate_object_metadata(&bad)
                .expect_err("must fail")
                .code(),
            OBJECT_INVALID_METADATA
        );
    }
}
