# Object Store Service and S3 Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A host-supplied `ObjectStore` service trait with S3-compatible (AWS/MinIO/Garage), local-filesystem, and in-memory backends, plus an `ArtifactStore` adapter that lifts the artifact ceiling to 64 MiB for large PDFs.

**Architecture:** The trait and its types live in `finstack-ai-runtime/src/services/object.rs` beside `ArtifactStore` (host service, NOT a seventh registry port). Three new T1 leaf crates under `extensions/stores/` implement it; `finstack-ai-test` gains a `FakeObjectStore` and a shared contract suite run against every backend. `ArtifactStore` gains a `limits()` method (default 4 MiB) so per-store ceilings replace the hard constant.

**Tech Stack:** Rust workspace; `reqwest` (existing, `stream` feature), `sha2` (existing), `hmac 0.12` (the single new external dependency), `tempfile`, `tokio`. Hand-rolled SigV4 — no AWS SDK.

**Spec:** `docs/superpowers/specs/2026-08-19-object-store-design.md`

## Global Constraints

- All leaf-crate dependencies MUST be `{ workspace = true }`; new external deps go in root `[workspace.dependencies]` first. Exactly one new external dep: `hmac = { version = "0.12", default-features = false }`.
- Every new crate copies the lint header from `extensions/context/finstack-ai-context-memory/src/lib.rs:1-22` verbatim (`#![warn(missing_docs)]`, `#![forbid(unsafe_code)]`, `deny(clippy::unwrap_used/expect_used/panic/unreachable)`, with the `cfg_attr(test, allow(...))` escape).
- No `std::env` reads anywhere. Credentials arrive as explicit `SecretString` values on config structs; every secret-bearing type gets a hand-written `Debug` printing `[REDACTED]`.
- Never contact live services in tests: S3 tests run against in-process loopback `tokio::net::TcpListener` fixtures only.
- Frozen error codes (spec §3.4): `object_unavailable`, `object_not_found`, `object_scope_mismatch`, `object_integrity_failure`, `object_too_large`, `object_invalid_key`, `object_invalid_metadata`, `object_unsupported`, `object_io_failure`.
- Oversize content is rejected, never truncated.
- After any public-API change to `finstack-ai-runtime` or `finstack-ai`: `mise run check-public-api` and commit the regenerated `fixtures/compatibility/public-rust-api/cargo-public-api/*.txt`.
- Verification gates: `cargo nextest run -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, and `mise run ci-rust` before the final task completes. Check exit codes explicitly (do not trust rtk summaries for gating).
- Commit after every task with the given message; end commits with `Co-Authored-By:` per repo convention only if other commits do (they do not — match existing style).

---

### Task 1: `ObjectStore` trait and types in `finstack-ai-runtime`

**Files:**
- Create: `crates/finstack-ai-runtime/src/services/object.rs`
- Modify: `crates/finstack-ai-runtime/src/services/mod.rs` (add `pub(crate) mod object;`)
- Modify: `crates/finstack-ai-runtime/src/lib.rs` (re-export block, after the `services::artifact` re-export)
- Test: inline `#[cfg(test)] mod tests` in `object.rs`

**Interfaces:**
- Consumes: `crate::{Bytes, Digest, Metadata, PortFuture, PortObject, RunId, Sensitivity, SessionId}` (same imports as `artifact.rs`).
- Produces (later tasks depend on these exact names):
  - `trait ObjectStore: PortObject` with methods `put, get, get_to_file, head, delete, list, presign_get, limits` (signatures below)
  - `ObjectScope { tenant_scope: Arc<str>, session_id: Option<SessionId>, run_id: Option<RunId>, sensitivity: Sensitivity }` + `fn digest(&self) -> Result<Digest, ObjectError>`
  - `ObjectKey::try_new(impl AsRef<str>) -> Result<ObjectKey, ObjectError>`, `fn as_str(&self) -> &str`
  - `PutPayload::{Bytes(Bytes), File(PathBuf)}`
  - `ObjectMetadata { media_type: Arc<str>, name: Option<Arc<str>>, attributes: Metadata }`
  - `ObjectRef { key: ObjectKey, scope_digest: Digest, content_digest: Digest, length: u64, media_type: Arc<str> }`
  - `ObjectEntry { key: ObjectKey, length: u64 }`, `ObjectPage { entries: Vec<ObjectEntry>, next: Option<PageToken> }`
  - `PageToken::first()`, `PageToken::opaque(impl AsRef<str>)`, `fn value(&self) -> Option<&str>`
  - `PresignedUrl { url: Arc<str>, expires_in_secs: u64 }`
  - `ObjectStoreLimits { max_object_bytes: u64 }` with `Default` = `5 * 1024 * 1024 * 1024`
  - `ObjectError` enum + `fn code(&self) -> &'static str` + the nine `OBJECT_*` const codes
  - `pub fn physical_object_key(key_prefix: Option<&str>, scope_digest: &Digest, key: &ObjectKey) -> String` — shared by all backends: `{prefix}/{first-16-hex-of-scope-digest}/{key}` (prefix segment omitted when `None`/empty)
  - `pub fn validate_object_metadata(metadata: &ObjectMetadata) -> Result<(), ObjectError>`

- [ ] **Step 1: Write the failing tests** (inline `mod tests` at the bottom of a new `object.rs` that starts with only the module doc comment, so the file compiles test-first via stubs is NOT required — in Rust write tests and types together per repo convention; the "failing" state is the file not existing. Create the full file in Step 2 and make these the test module.)

Tests to include verbatim:

```rust
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
        assert_eq!(bad.digest().expect_err("must fail").code(), OBJECT_INVALID_METADATA);
        let mut nul = scope();
        nul.tenant_scope = Arc::from("a\0b");
        assert_eq!(nul.digest().expect_err("must fail").code(), OBJECT_INVALID_METADATA);
    }

    #[test]
    fn object_key_accepts_segmented_names() {
        let key = ObjectKey::try_new("reports/2026/q3.pdf").expect("valid key");
        assert_eq!(key.as_str(), "reports/2026/q3.pdf");
    }

    #[test]
    fn object_key_rejects_traversal_absolute_empty_and_bad_chars() {
        for bad in ["", "/abs", "a//b", "a/../b", "..", "a b", "ключ", &"x".repeat(513)] {
            let error = ObjectKey::try_new(bad).expect_err("must reject");
            assert_eq!(error.code(), OBJECT_INVALID_KEY);
        }
    }

    #[test]
    fn physical_key_layout_prefixes_scope_digest() {
        let digest = scope().digest().expect("digest");
        let key = ObjectKey::try_new("doc.pdf").expect("key");
        let physical = physical_object_key(Some("finstack"), &digest, &key);
        let hex16: String = digest.to_hex().chars().take(16).collect();
        assert_eq!(physical, format!("finstack/{hex16}/doc.pdf"));
        assert_eq!(
            physical_object_key(None, &digest, &key),
            format!("{hex16}/doc.pdf")
        );
    }

    #[test]
    fn limits_default_is_five_gib() {
        assert_eq!(ObjectStoreLimits::default().max_object_bytes, 5 * 1024 * 1024 * 1024);
    }

    #[test]
    fn error_codes_are_frozen() {
        assert_eq!(ObjectError::NotFound.code(), "object_not_found");
        assert_eq!(
            ObjectError::Unsupported { operation: Arc::from("presign_get") }.code(),
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
            validate_object_metadata(&bad).expect_err("must fail").code(),
            OBJECT_INVALID_METADATA
        );
    }
}
```

- [ ] **Step 2: Write the implementation**

`crates/finstack-ai-runtime/src/services/object.rs` (module doc: `//! Scoped unstructured object storage contract shared by store backends.`):

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Bytes, Digest, Metadata, PortFuture, PortObject, RunId, Sensitivity, SessionId};

/// Stable code for an unavailable object service.
pub const OBJECT_UNAVAILABLE: &str = "object_unavailable";
/// Stable code for a missing object.
pub const OBJECT_NOT_FOUND: &str = "object_not_found";
/// Stable code for a scope-binding failure.
pub const OBJECT_SCOPE_MISMATCH: &str = "object_scope_mismatch";
/// Stable code for a content integrity failure.
pub const OBJECT_INTEGRITY_FAILURE: &str = "object_integrity_failure";
/// Stable code for rejected rather than truncated oversized content.
pub const OBJECT_TOO_LARGE: &str = "object_too_large";
/// Stable code for a malformed logical key.
pub const OBJECT_INVALID_KEY: &str = "object_invalid_key";
/// Stable code for malformed object metadata.
pub const OBJECT_INVALID_METADATA: &str = "object_invalid_metadata";
/// Stable code for an operation the backend does not support.
pub const OBJECT_UNSUPPORTED: &str = "object_unsupported";
/// Stable code for a local I/O failure.
pub const OBJECT_IO_FAILURE: &str = "object_io_failure";

/// Maximum logical key length in bytes.
pub const MAX_OBJECT_KEY_BYTES: usize = 512;

/// Exact authorization and integrity scope for an object operation.
///
/// Unlike [`crate::ArtifactScope`], `session_id` is optional so objects may
/// outlive a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectScope {
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
            ObjectError::InvalidMetadata { message: Arc::from(error.to_string()) }
        })?;
        Digest::domain_separated("object-scope", 1, &canonical).map_err(|error| {
            ObjectError::InvalidMetadata { message: Arc::from(error.to_string()) }
        })
    }
}

/// Validated logical object key: `/`-joined segments of `[A-Za-z0-9._-]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "Arc<str>", into = "Arc<str>")]
pub struct ObjectKey(Arc<str>);

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
        let invalid = |message: &str| ObjectError::InvalidKey { message: Arc::from(message) };
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
pub enum PutPayload {
    /// Fully materialized content.
    Bytes(Bytes),
    /// Content streamed from a local file; never fully materialized.
    File(PathBuf),
}

/// Exact metadata mapped to the returned [`ObjectRef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMetadata {
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
pub struct ObjectRef {
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
pub struct ObjectEntry {
    /// Logical key within the scope.
    pub key: ObjectKey,
    /// Content length in bytes.
    pub length: u64,
}

/// Backend-opaque pagination cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageToken(#[serde(default, skip_serializing_if = "Option::is_none")] Option<Arc<str>>);

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
pub struct ObjectPage {
    /// Entries in this page.
    pub entries: Vec<ObjectEntry>,
    /// Cursor for the next page, when more results exist.
    pub next: Option<PageToken>,
}

/// Time-limited download URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresignedUrl {
    /// Complete presigned URL.
    pub url: Arc<str>,
    /// Requested validity window in seconds.
    pub expires_in_secs: u64,
}

/// Per-store object size ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectStoreLimits {
    /// Reject puts above this size; never truncate.
    pub max_object_bytes: u64,
}

impl Default for ObjectStoreLimits {
    fn default() -> Self {
        // S3 single-PUT protocol ceiling; the cap until multipart lands.
        Self { max_object_bytes: 5 * 1024 * 1024 * 1024 }
    }
}

/// Scoped host-supplied unstructured object service.
pub trait ObjectStore: PortObject {
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
    fn head(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<ObjectRef, ObjectError>>;

    /// Delete one object; deleting a missing object is not an error.
    fn delete(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<(), ObjectError>>;

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

/// Compose the physical backend key: `{prefix}/{scope-digest-16-hex}/{key}`.
#[must_use]
pub fn physical_object_key(key_prefix: Option<&str>, scope_digest: &Digest, key: &ObjectKey) -> String {
    let hex = scope_digest.to_hex();
    let hex16 = hex.get(..16).unwrap_or(&hex);
    match key_prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{hex16}/{}", key.as_str()),
        _ => format!("{hex16}/{}", key.as_str()),
    }
}

/// Validate bounded, non-secret object metadata.
///
/// # Errors
///
/// Rejects empty or NUL-bearing media type and name fields.
pub fn validate_object_metadata(metadata: &ObjectMetadata) -> Result<(), ObjectError> {
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
pub enum ObjectError {
    /// Service unavailable (transport, auth, or server failure).
    #[error("{}: {message}", OBJECT_UNAVAILABLE)]
    Unavailable {
        /// Bounded non-secret diagnostic.
        message: Arc<str>,
    },
    /// Object is missing.
    #[error("{}: object is missing", OBJECT_NOT_FOUND)]
    NotFound,
    /// Requested scope differs from the object's frozen binding.
    #[error("{}: expected scope {expected}, actual scope {actual}", OBJECT_SCOPE_MISMATCH)]
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
```

Wire up: in `services/mod.rs` add `pub(crate) mod object;` (alphabetical position). In `lib.rs`, after the `services::artifact` re-export block add:

```rust
pub use services::object::{
    MAX_OBJECT_KEY_BYTES, OBJECT_INTEGRITY_FAILURE, OBJECT_INVALID_KEY, OBJECT_INVALID_METADATA,
    OBJECT_IO_FAILURE, OBJECT_NOT_FOUND, OBJECT_SCOPE_MISMATCH, OBJECT_TOO_LARGE,
    OBJECT_UNAVAILABLE, OBJECT_UNSUPPORTED, ObjectEntry, ObjectError, ObjectKey, ObjectMetadata,
    ObjectPage, ObjectRef, ObjectScope, ObjectStore, ObjectStoreLimits, PageToken, PresignedUrl,
    PutPayload, physical_object_key, validate_object_metadata,
};
```

- [ ] **Step 3: Run tests, verify pass**

Run: `cargo nextest run -p finstack-ai-runtime object` — expect all Task 1 tests PASS.
Run: `cargo clippy -p finstack-ai-runtime --all-targets -- -D warnings` — expect clean.

- [ ] **Step 4: Regenerate the public-API baseline**

Run: `mise run check-public-api` (it fails first, regenerates or instructs; follow its output to update `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-runtime.txt`). Re-run to confirm green.

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai-runtime fixtures/compatibility/public-rust-api
git commit -m "feat(runtime): add ObjectStore host service contract"
```

---

### Task 2: Per-store `ArtifactStore` limits

**Files:**
- Modify: `crates/finstack-ai-runtime/src/services/artifact.rs`
- Modify: `crates/finstack-ai-runtime/src/lib.rs` (export `ArtifactStoreLimits`)
- Modify: `extensions/context/finstack-ai-context-memory/src/lib.rs` (`InProcessArtifactStore`)
- Test: inline in both files

**Interfaces:**
- Produces: `pub struct ArtifactStoreLimits { pub max_artifact_bytes: usize }` (`Default` = `MAX_ARTIFACT_BYTES`); `ArtifactStore::limits(&self) -> ArtifactStoreLimits` with a default implementation; `stage_required_artifact` enforcing `store.limits()`; `validate_staged_artifact(scope, content, metadata, artifact, limits: &ArtifactStoreLimits)` (signature gains the limits param); `InProcessArtifactStore::with_max_artifact_bytes(usize)`.
- `MAX_ARTIFACT_BYTES` stays exported, now documented as the default.

- [ ] **Step 1: Write failing tests** (append to the existing `mod tests` in `artifact.rs`):

```rust
#[test]
fn default_limits_match_the_v1_ceiling() {
    struct DefaultStore;
    impl ArtifactStore for DefaultStore {
        fn stage_put(&self, _: ArtifactScope, _: Bytes, _: ArtifactMetadata)
            -> PortFuture<Result<ArtifactRef, ArtifactError>> {
            Box::pin(async { Err(ArtifactError::NotFound) })
        }
        fn get(&self, _: ArtifactScope, _: ArtifactRef)
            -> PortFuture<Result<Bytes, ArtifactError>> {
            Box::pin(async { Err(ArtifactError::NotFound) })
        }
    }
    assert_eq!(DefaultStore.limits().max_artifact_bytes, MAX_ARTIFACT_BYTES);
}

#[test]
fn staging_respects_store_limits_not_the_constant() {
    // A store that raises its ceiling accepts content above MAX_ARTIFACT_BYTES.
    // Uses the existing test-double store from this module, overriding limits():
    // add `limits: ArtifactStoreLimits` field to the module's RecordingStore and
    // return it from limits(). Content of MAX_ARTIFACT_BYTES + 1 bytes must be
    // accepted when the store reports a higher ceiling, and rejected with
    // ARTIFACT_TOO_LARGE by a store with the default ceiling.
    let content = Bytes::from(vec![0_u8; MAX_ARTIFACT_BYTES + 1]);
    let raised = ArtifactStoreLimits { max_artifact_bytes: 8 * 1024 * 1024 };
    assert!(validate_artifact_input(&scope(), &content, &metadata(), &raised).is_ok());
    let default = ArtifactStoreLimits::default();
    assert_eq!(
        validate_artifact_input(&scope(), &content, &metadata(), &default)
            .expect_err("must reject")
            .code(),
        ARTIFACT_TOO_LARGE
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p finstack-ai-runtime artifact` — expect FAIL (no `limits` method, no `ArtifactStoreLimits`).

- [ ] **Step 3: Implement**

In `artifact.rs`:

```rust
/// Per-store artifact size ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactStoreLimits {
    /// Reject staged content above this size; never truncate.
    pub max_artifact_bytes: usize,
}

impl Default for ArtifactStoreLimits {
    fn default() -> Self {
        Self { max_artifact_bytes: MAX_ARTIFACT_BYTES }
    }
}
```

Add to the trait (after `get`):

```rust
    /// This store's size ceilings. Defaults to the v1 4 MiB constant.
    fn limits(&self) -> ArtifactStoreLimits {
        ArtifactStoreLimits::default()
    }
```

Change `validate_artifact_input(scope, content, metadata, limits: &ArtifactStoreLimits)` — the size check becomes `content.len() > limits.max_artifact_bytes` (error carries `max: limits.max_artifact_bytes`); make the function `pub(crate)`→ keep private but tests in-module reach it. `stage_required_artifact` computes `let limits = store.limits();` and passes `&limits` to both validation calls. `validate_staged_artifact` gains the trailing `limits: &ArtifactStoreLimits` parameter (public breaking change; baselines regenerated below). Update the doc comment on `MAX_ARTIFACT_BYTES` to `/// Default individual byte-string ceiling; stores may override via limits().`

In `extensions/context/finstack-ai-context-memory/src/lib.rs`: give `InProcessArtifactStore` a `limits: ArtifactStoreLimits` field (Default::default in `Default` impl), add

```rust
    /// Override the artifact byte ceiling for this in-process store.
    #[must_use]
    pub fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
        self.limits = ArtifactStoreLimits { max_artifact_bytes };
        self
    }
```

and implement `fn limits(&self) -> ArtifactStoreLimits { self.limits }` in its `ArtifactStore` impl. Fix all callers of `validate_staged_artifact` found via `grep -rn "validate_staged_artifact" crates extensions` (pass `&store.limits()` where a store is in hand, else `&ArtifactStoreLimits::default()`).

- [ ] **Step 4: Run tests + workspace build**

Run: `cargo nextest run -p finstack-ai-runtime artifact && cargo check --workspace` — expect PASS/clean (the workspace check catches every `validate_staged_artifact` caller).

- [ ] **Step 5: Regenerate public-API baselines, commit**

Run: `mise run check-public-api`, update baselines.

```bash
git add crates/finstack-ai-runtime extensions/context/finstack-ai-context-memory fixtures/compatibility/public-rust-api
git commit -m "feat(runtime): per-store artifact limits replace the hard 4 MiB constant"
```

---

### Task 3: Consumers derive ceilings from `store.limits()`

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-document-ingest/src/lib.rs` (`try_new` around line 204)
- Modify: `extensions/toolsets/finstack-ai-tools-document/src/parser.rs` (doc comment only — `DocumentLimits` is already configurable)
- Modify: `extensions/toolsets/finstack-ai-tools-filesystem/src/lib.rs:111` and `src/operation.rs:53`
- Test: existing inline test modules in each crate

**Interfaces:**
- Consumes: `ArtifactStore::limits()` from Task 2.
- Produces: document-ingest `try_new(store, index)` defaults `DocumentLimits.max_input_bytes` to `store.limits().max_artifact_bytes as u64` (explicit `try_with_limits` still wins). Filesystem toolset: `FileSystemLimits::validate` takes `max_artifact_bytes: usize` (called with the store's limit in `with_artifact_store`, else `MAX_ARTIFACT_BYTES`); `operation.rs` compares against a `max_artifact_bytes` argument threaded from the toolset instead of the constant.

- [ ] **Step 1: Write failing test** (document-ingest `src/tests.rs`):

```rust
#[test]
fn ingest_limit_follows_the_store_ceiling() {
    let store: Arc<dyn ArtifactStore> =
        Arc::new(InProcessArtifactStore::default().with_max_artifact_bytes(64 * 1024 * 1024));
    let middleware = DocumentIngestMiddleware::try_new(store, index()).expect("middleware");
    assert_eq!(middleware.limits().max_input_bytes, 64 * 1024 * 1024);
}
```

(Add a `pub(crate) fn limits(&self) -> &DocumentLimits` accessor if none exists.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p finstack-ai-middleware-document-ingest` — expect FAIL (limit still 4 MiB).

- [ ] **Step 3: Implement all three consumer changes**

- Document-ingest `try_new`: replace `DocumentLimits::default()` with

```rust
let limits = DocumentLimits {
    max_input_bytes: u64::try_from(store.limits().max_artifact_bytes).unwrap_or(u64::MAX),
    ..DocumentLimits::default()
};
Self::try_with_limits(store, index, limits)
```

- `parser.rs:11` doc comment becomes `/// Reject inputs above this size; defaults to the artifact store ceiling at wiring time.` (Default impl keeps 4 MiB — it is the store-less default.)
- Filesystem toolset: change `FileSystemLimits::validate(self)` to `validate(self, max_artifact_bytes: usize)`; the `file_bytes` ceiling check becomes `self.file_bytes > max_artifact_bytes`. `with_artifact_store` re-validates limits against `store.limits().max_artifact_bytes`. `operation.rs:53`: the serialized-result check receives the ceiling as a parameter from the toolset (`self.max_artifact_bytes`, a field set at construction/`with_artifact_store`) instead of referencing the constant.

- [ ] **Step 4: Run tests**

Run: `cargo nextest run -p finstack-ai-middleware-document-ingest -p finstack-ai-tools-document -p finstack-ai-tools-filesystem` — expect PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/middleware/finstack-ai-middleware-document-ingest extensions/toolsets/finstack-ai-tools-document extensions/toolsets/finstack-ai-tools-filesystem
git commit -m "feat(extensions): document and filesystem consumers follow store artifact limits"
```

---

### Task 4: `FakeObjectStore` and the shared contract suite in `finstack-ai-test`

**Files:**
- Create: `crates/finstack-ai-test/src/object_store/mod.rs` (fake + suite)
- Modify: `crates/finstack-ai-test/src/lib.rs` (add `pub mod object_store;`)
- Test: `crates/finstack-ai-test/tests/object_store_contract.rs` (runs the suite against the fake)

**Interfaces:**
- Consumes: everything Task 1 exported from `finstack_ai_runtime`.
- Produces:
  - `pub struct FakeObjectStore` (`Default`), `pub fn with_limits(ObjectStoreLimits) -> Self`, `pub fn fail_next_get_with_integrity(&self)` (poisons the next `get`/`get_to_file` to return `Integrity` — used by the suite's injection case)
  - `pub async fn run_object_store_contract_suite(store: Arc<dyn ObjectStore>, supports_presign: bool)` — panics with a labeled assertion on any contract violation. Every backend task calls this.

- [ ] **Step 1: Write the contract suite and fake together** (the suite IS the test; the fake is its first subject).

`FakeObjectStore` implementation sketch (complete in the file): `Mutex<BTreeMap<String, StoredObject>>` keyed by `physical_object_key(None, &scope_digest, &key)`, where

```rust
struct StoredObject {
    scope_digest: Digest,
    content: Bytes,
    content_digest: Digest,
    media_type: Arc<str>,
}
```

`put`: validate key already typed; `validate_object_metadata`; compute length from payload (`Bytes::len`, or `std::fs::metadata(path)` for `File` — the fake reads the file fully with `std::fs::read`); enforce `limits().max_object_bytes` → `TooLarge`; store; return `ObjectRef`. `get`: look up, compare stored `scope_digest` to the caller's `scope.digest()?` → `ScopeMismatch` on difference, honor the integrity-poison flag, re-verify digest, return bytes. `get_to_file`: `std::fs::write` then same checks. `head`/`delete`/`list` straightforward (list filters by scope-digest prefix + optional key prefix, pages of 2 entries to force pagination in the suite, cursor = last key via `PageToken::opaque`). `presign_get`: `Ok(PresignedUrl { url: Arc::from(format!("fake://{key}", key = key.as_str())), expires_in_secs: expiry.as_secs() })`.

Contract suite cases (each an `async fn` the runner calls, panicking with the case name on failure):

```rust
pub async fn run_object_store_contract_suite(store: Arc<dyn ObjectStore>, supports_presign: bool) {
    put_get_round_trip(&*store).await;                 // bytes payload, ref fields exact
    put_file_and_get_to_file_round_trip(&*store).await; // tempfile in, tempfile out, digests equal
    head_matches_put_ref(&*store).await;
    cross_scope_read_fails_closed(&*store).await;      // second tenant gets ScopeMismatch or NotFound, never bytes
    missing_object_is_not_found(&*store).await;
    delete_is_idempotent(&*store).await;
    list_pages_within_scope_only(&*store).await;       // 5 objects in scope A, 1 in scope B; walk pages; B never appears
    oversize_put_is_rejected(&*store).await;           // only when limits().max_object_bytes < u64::MAX proxy: put limits+1 via Bytes if ceiling <= 64 MiB else skip
    invalid_key_never_reaches_backend();               // ObjectKey::try_new rejections
    if supports_presign { presign_returns_url(&*store).await; }
    else { presign_is_unsupported(&*store).await; }
}
```

Write each case fully in the file with concrete scopes (`tenant-a`/`tenant-b`), keys (`docs/a.bin` …), and 1 KiB deterministic content (`vec![7_u8; 1024]`).

`tests/object_store_contract.rs`:

```rust
use std::sync::Arc;
use finstack_ai_test::object_store::{FakeObjectStore, run_object_store_contract_suite};

#[tokio::test(flavor = "multi_thread")]
async fn fake_object_store_satisfies_the_contract() {
    run_object_store_contract_suite(Arc::new(FakeObjectStore::default()), true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fake_object_store_reports_injected_integrity_failure() {
    let store = FakeObjectStore::default();
    store.fail_next_get_with_integrity();
    // put then get; expect ObjectError::Integrity
}
```

- [ ] **Step 2: Run to verify green**

Run: `cargo nextest run -p finstack-ai-test object_store` — expect PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/finstack-ai-test
git commit -m "feat(test): FakeObjectStore and shared ObjectStore contract suite"
```

---

### Task 5: `finstack-ai-store-object-local`

**Files:**
- Create: `extensions/stores/finstack-ai-store-object-local/Cargo.toml`, `src/lib.rs`, `README.md`
- Modify: root `Cargo.toml` (`members` + `[workspace.dependencies] finstack-ai-store-object-local = { path = "extensions/stores/finstack-ai-store-object-local", version = "1.0.0" }`)
- Test: `src/lib.rs` inline + `tests/contract.rs`

**Interfaces:**
- Consumes: Task 1 types; Task 4 suite (dev-dependency).
- Produces: `LocalObjectStore::try_new(root_dir: PathBuf) -> Result<Self, ObjectError>` implementing `ObjectStore`; `with_limits(ObjectStoreLimits)`.

`Cargo.toml` (model: sqlite store crate):

```toml
[package]
name = "finstack-ai-store-object-local"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Local-filesystem ObjectStore backend for finstack-ai"
readme = "README.md"

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
serde_json = { workspace = true }
sha2 = { workspace = true }
tempfile = { workspace = true }
tokio = { workspace = true, features = ["fs", "io-util"] }

[dev-dependencies]
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }

[lints]
workspace = true
```

Layout on disk: object at `{root}/{physical_object_key(None, scope, key)}`; sidecar metadata at the same path + `.meta.json` holding `{ "scope_digest": hex, "content_digest": hex, "length": n, "media_type": s }` (serde struct `SidecarMeta`). Write order: content to `tempfile::NamedTempFile` in the destination directory → sidecar tempfile → `persist()` sidecar → `persist()` content (content rename last publishes the object; a crash beforehand leaves only an orphan sidecar which `get` treats as `NotFound` because the content file is missing). `get` reads content with `tokio::fs::read`, recomputes SHA-256, compares to sidecar → `Integrity` on mismatch; compares caller scope digest to sidecar → `ScopeMismatch`. `get_to_file` streams with `tokio::fs::copy` then verifies by reading the destination through a `sha2` incremental hash (8 KiB `tokio::io::AsyncReadExt::read` loop). `put` with `PutPayload::File` streams source→temp via the same 8 KiB loop, hashing as it copies, counting bytes against `limits`. `list` walks `tokio::fs::read_dir` recursively under the scope directory, sorts keys, pages by 1000, cursor = `PageToken::opaque(last_key)`. `presign_get` → `Err(ObjectError::Unsupported { operation: Arc::from("presign_get") })`. All `std::io::Error`s map to `ObjectError::Io { message: Arc::from(error.kind().to_string()) }` (kind only — never the raw path-bearing message... paths are local and non-secret, but keep messages bounded: use kind + relative key).

- [ ] **Step 1: Write `tests/contract.rs` first**

```rust
use std::sync::Arc;
use finstack_ai_store_object_local::LocalObjectStore;
use finstack_ai_test::object_store::run_object_store_contract_suite;

#[tokio::test(flavor = "multi_thread")]
async fn local_store_satisfies_the_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalObjectStore::try_new(dir.path().to_path_buf()).expect("store");
    run_object_store_contract_suite(Arc::new(store), false).await;
}
```

- [ ] **Step 2: Run to verify it fails** (crate skeleton with `todo-free` stub returning `Unavailable` compiles; contract suite panics). Run: `cargo nextest run -p finstack-ai-store-object-local` — expect FAIL.

- [ ] **Step 3: Implement `LocalObjectStore` fully** (as specified above), plus inline unit tests:

```rust
#[test]
fn sidecar_round_trips_serde() { /* SidecarMeta -> json -> SidecarMeta */ }

#[tokio::test(flavor = "multi_thread")]
async fn tampered_content_fails_integrity() {
    // put, then overwrite the content file bytes directly, then get -> Integrity
}

#[tokio::test(flavor = "multi_thread")]
async fn presign_is_unsupported() { /* code() == OBJECT_UNSUPPORTED */ }
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo nextest run -p finstack-ai-store-object-local && cargo clippy -p finstack-ai-store-object-local --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git add extensions/stores/finstack-ai-store-object-local Cargo.toml Cargo.lock
git commit -m "feat(stores): local-filesystem ObjectStore backend"
```

---

### Task 6: S3 crate skeleton, config, and SigV4 signer

**Files:**
- Create: `extensions/stores/finstack-ai-store-object-s3/Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/sigv4.rs`, `README.md`
- Modify: root `Cargo.toml` (member + workspace dep + `hmac = { version = "0.12", default-features = false }` in `[workspace.dependencies]`)
- Test: inline in `config.rs` and `sigv4.rs`

**Interfaces:**
- Consumes: `finstack_ai_runtime::SecretString` (already exported), Task 1 types.
- Produces:
  - `S3ObjectStoreConfig::try_new(endpoint: &str, bucket: &str, region: &str) -> Result<Self, ObjectError>` + builders `with_key_prefix(&str)`, `with_addressing(Addressing)`, `with_credentials(access_key_id: &str, secret_access_key: SecretString) -> Result<Self, ObjectError>`, `with_timeout(Duration)`, `with_max_object_bytes(u64)`, `with_presign_expiry_max(Duration)`
  - `pub enum Addressing { Path, VirtualHost }` (default `Path`)
  - `sigv4::SigningParams { access_key_id: &str, secret_key: &str, region: &str, service: &str, timestamp: UtcStamp }`
  - `sigv4::sign_headers(params, method, url_path, canonical_query, host, payload_sha256_hex, extra_headers: &[(String, String)]) -> Vec<(String, String)>` returning `authorization`, `x-amz-date`, `x-amz-content-sha256` (+ passthroughs)
  - `sigv4::presign_url(params, method, url_path, host, scheme, expiry_secs) -> String`
  - `sigv4::UtcStamp { pub date: String /* YYYYMMDD */, pub datetime: String /* YYYYMMDDTHHMMSSZ */ }` with `UtcStamp::now()` built from `std::time::SystemTime` via the days-from-civil inverse (no chrono; implement `civil_from_days` per Howard Hinnant's algorithm, ~15 lines)

`Cargo.toml` dependencies: `finstack-ai-kernel`, `finstack-ai-runtime` (as Task 5), `hmac`, `sha2`, `reqwest`, `serde`, `serde_json`, `tokio = { workspace = true, features = ["fs", "io-util"] }`, `futures-util`, `tempfile`; `[features] default = []` / `vendored-tls = ["reqwest/native-tls-vendored"]`; dev-deps `finstack-ai-test`, `tokio` with `net, rt-multi-thread, macros`.

Config validation (mirrors `AnthropicConfig::try_new`): parse endpoint with `reqwest::Url`; reject non-http(s) scheme, embedded username/password, query, fragment; reject empty bucket/region or NUL bytes; bucket charset `[a-z0-9.-]`, 3–63 chars. Hand-written `Debug` for the config prints `access_key_id` in full (it is an identifier, matching AWS convention) and `secret_access_key: SecretString([REDACTED])` (free — `SecretString`'s own Debug). Errors are `ObjectError::InvalidMetadata` with stable messages (`"invalid_endpoint"`, `"invalid_bucket"`, ...).

SigV4 core (all in `sigv4.rs`, pure functions, no I/O):

```rust
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable_hmac()); // HMAC accepts any key length; helper avoids panic lint
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> [u8; 32] {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}
```

Canonical request: `METHOD\n{uri_encoded_path}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}`; string-to-sign: `AWS4-HMAC-SHA256\n{datetime}\n{date}/{region}/{service}/aws4_request\n{sha256_hex(canonical_request)}`. URI-encode path segments per RFC 3986 keeping `/` (write `fn uri_encode(input: &str, encode_slash: bool) -> String`). Presign: query carries `X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`, `X-Amz-SignedHeaders=host`, payload hash literal `UNSIGNED-PAYLOAD`, then append `X-Amz-Signature`.

- [ ] **Step 1: Write the failing known-answer tests** (in `sigv4.rs` `mod tests`) — AWS's published SigV4 test vector:

```rust
// AWS documented example: GET https://examplebucket.s3.amazonaws.com/test.txt
// with access key AKIAIOSFODNN7EXAMPLE, secret
// wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY, region us-east-1, service s3,
// timestamp 20130524T000000Z, Range: bytes=0-9, empty-body payload hash
// e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.
// Expected signature:
// f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41
#[test]
fn sigv4_header_signing_matches_the_aws_known_answer() { /* assert exact signature */ }

// AWS documented presign example, same credentials/bucket/key, 86400s expiry.
// Expected signature:
// aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404
#[test]
fn sigv4_presign_matches_the_aws_known_answer() { /* assert exact query signature */ }

#[test]
fn utc_stamp_formats_a_known_epoch() {
    // 1369353600 == 2013-05-24T00:00:00Z
    let stamp = UtcStamp::from_unix_secs(1_369_353_600);
    assert_eq!(stamp.date, "20130524");
    assert_eq!(stamp.datetime, "20130524T000000Z");
}
```

And in `config.rs` `mod tests`:

```rust
#[test]
fn config_rejects_query_fragment_and_credentials_in_endpoint() { /* three cases, code OBJECT_INVALID_METADATA */ }

#[test]
fn debug_and_errors_never_leak_the_secret_key() {
    let config = S3ObjectStoreConfig::try_new("http://127.0.0.1:9000", "bucket", "garage")
        .expect("config")
        .with_credentials("AKIAEXAMPLE", SecretString::try_new("SUPERSECRET").expect("secret"))
        .expect("credentials");
    let rendered = format!("{config:?}");
    assert!(!rendered.contains("SUPERSECRET"));
    assert!(rendered.contains("[REDACTED]"));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p finstack-ai-store-object-s3` fails to compile (crate empty). Scaffold `lib.rs` with lint header + `pub mod config; pub mod sigv4;` stubs; tests now FAIL on missing items, then on wrong signatures.

- [ ] **Step 3: Implement `config.rs` and `sigv4.rs` until the known-answer tests pass**

Run: `cargo nextest run -p finstack-ai-store-object-s3` — expect PASS. The known-answer test failing means the canonicalization is wrong; debug against the AWS doc's printed canonical request (include it in a test comment for the implementer).

- [ ] **Step 4: Commit**

```bash
git add extensions/stores/finstack-ai-store-object-s3 Cargo.toml Cargo.lock
git commit -m "feat(stores): S3 object store config and hand-rolled SigV4 signer"
```

---

### Task 7: `S3ObjectStore` operations against loopback fixtures

**Files:**
- Create: `extensions/stores/finstack-ai-store-object-s3/src/store.rs`, `src/request.rs` (URL building + response mapping), `tests/loopback.rs`
- Modify: `src/lib.rs` (export `S3ObjectStore`)
- Test: `tests/loopback.rs`

**Interfaces:**
- Consumes: Task 6 config/signer; Task 1 trait; Task 4 suite.
- Produces: `S3ObjectStore::try_new(config: S3ObjectStoreConfig) -> Result<Self, ObjectError>` implementing `ObjectStore` completely.

Operation mapping (all URLs built in `request.rs`; `Addressing::Path` → `{endpoint}/{bucket}/{physical_key}`, `VirtualHost` → `{scheme}://{bucket}.{host}/{physical_key}`):

| Trait method | HTTP | Notes |
|---|---|---|
| `put` | `PUT` | headers: `content-type` from metadata, `content-length`, `x-amz-meta-fsai-digest: {content_digest_hex}`, `x-amz-meta-fsai-scope: {scope_digest_hex}`, `x-amz-meta-fsai-name` (uri-encoded, when present); payload hash = real SHA-256 (computed streaming for `File` via 64 KiB read loop **before** the request — file read twice: once to hash+count, once to stream; enforce `limits` during the hash pass) |
| `get` | `GET` | verify `x-amz-meta-fsai-scope` equals caller scope digest → `ScopeMismatch`; hash body → compare `x-amz-meta-fsai-digest` → `Integrity` |
| `get_to_file` | `GET` | `resp.bytes_stream()` → tempfile in `dest` parent → incremental hash → verify → atomic rename to `dest` |
| `head` | `HEAD` | build `ObjectRef` from the two `x-amz-meta-fsai-*` headers + `content-length` + `content-type` |
| `delete` | `DELETE` | 204/200 → Ok; 404 → Ok (idempotent) |
| `list` | `GET ?list-type=2&prefix={scope_prefix}&continuation-token=...` | parse XML minimally with a hand-written scanner over `<Key>`, `<Size>`, `<NextContinuationToken>`, `<IsTruncated>` (no XML dep; write `fn extract_tag_values(body: &str, tag: &str) -> Vec<String>` handling XML entity decoding for `&amp; &lt; &gt; &quot; &#39;`); strip the scope prefix from returned keys before `ObjectKey::try_new` |
| `presign_get` | none | `sigv4::presign_url`; clamp expiry to `presign_expiry_max` |

Streaming `PutPayload::File` body: `reqwest::Body::wrap_stream(futures_util::stream::unfold(file, |mut file| async move { /* read 64 KiB, yield Ok(Bytes), None on EOF */ }))`.

Response mapping: connection/timeout errors → `Unavailable { message: "transport_failure" }` (NEVER interpolate the reqwest error string — it can carry the full URL including presign signatures); 404 → `NotFound`; 403 → `Unavailable { message: "access_denied" }`; other non-2xx → `Unavailable { message: format!("http_{status}") }`.

- [ ] **Step 1: Write failing loopback tests** (`tests/loopback.rs`, modeled on `extensions/providers/finstack-ai-provider-anthropic/tests/provider_fixtures.rs` — `TcpListener::bind("127.0.0.1:0")`, accept one connection, read the request into a `String` until body length satisfied, assert on it, write a canned HTTP/1.1 response):

Test list (write each fully; the serve helper takes `Vec<CannedResponse { status_line, headers, body }>` and returns captured requests):

```text
put_signs_path_style_and_sends_metadata_headers
  - assert request line: PUT /bucket/finstack/{hex16}/docs/a.pdf HTTP/1.1
  - assert authorization header starts "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/"
  - assert x-amz-content-sha256 equals the real payload hash
  - assert x-amz-meta-fsai-digest and x-amz-meta-fsai-scope present
put_virtual_host_addressing_targets_bucket_host
  - Addressing::VirtualHost; assert Host: bucket.127.0.0.1... path has no /bucket
get_verifies_scope_and_digest
  - respond 200 with correct meta headers and body -> Ok(bytes)
get_with_wrong_scope_header_fails_closed        -> ScopeMismatch
get_with_tampered_body_fails_integrity          -> Integrity
missing_object_maps_404_to_not_found
delete_treats_404_as_success
list_walks_continuation_tokens
  - two canned XML pages with NextContinuationToken; assert second request
    carries continuation-token; entries concatenate; keys have scope prefix stripped
presign_get_produces_a_signed_query_url
  - no server; assert URL contains X-Amz-Signature, X-Amz-Expires=900, bucket path
oversize_file_put_is_rejected_before_any_request
  - 6 GiB is impractical; construct store with with_max_object_bytes(1024),
    put a 2 KiB tempfile -> TooLarge, and assert the listener saw zero connections
s3_store_satisfies_the_contract  (optional gate, see Step 3)
```

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p finstack-ai-store-object-s3 --test loopback` — FAIL (no `S3ObjectStore`).

- [ ] **Step 3: Implement `store.rs`/`request.rs` until loopback tests pass.** Then add the contract-suite test backed by a minimal in-process S3 stub: implement `mod s3_stub` in `tests/loopback.rs` — a tokio task holding a `BTreeMap<String, (Vec<u8>, Vec<(String,String)>)>` speaking just enough HTTP/1.1 (PUT stores body+`x-amz-meta-*` headers, GET/HEAD serve them, DELETE removes, ListV2 renders XML sorted with `max-keys` honoring `continuation-token`) to pass `run_object_store_contract_suite(store, true)`. ~150 lines; it is test code and may use `expect`.

Run: `cargo nextest run -p finstack-ai-store-object-s3` — expect PASS.

- [ ] **Step 4: Secret canary for the store level**

Add to `tests/loopback.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn transport_errors_never_leak_signing_material() {
    // point the store at a closed port; put; assert the rendered error string
    // contains neither the secret nor "X-Amz-Signature"
}
```

Run: `cargo nextest run -p finstack-ai-store-object-s3` — expect PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/stores/finstack-ai-store-object-s3
git commit -m "feat(stores): S3ObjectStore operations over loopback-tested SigV4 client"
```

---### Task 8: `ObjectArtifactStore` adapter crate

**Files:**
- Create: `extensions/stores/finstack-ai-store-artifact-object/Cargo.toml`, `src/lib.rs`, `README.md`
- Modify: root `Cargo.toml` (member + workspace dep)
- Test: inline `src/tests.rs`

**Interfaces:**
- Consumes: `ObjectStore` (Task 1), `ArtifactStore`/`ArtifactStoreLimits` (Task 2), `FakeObjectStore` (Task 4, dev-dep).
- Produces: `ObjectArtifactStore::new(store: Arc<dyn ObjectStore>) -> Self`, `with_max_artifact_bytes(usize)` (default **64 MiB**, clamped to `store.limits().max_object_bytes`), implementing `ArtifactStore`.

Mapping rules:
- `ArtifactScope { tenant_scope, session_id, run_id, sensitivity }` → `ObjectScope { tenant_scope, session_id: Some(session_id), run_id, sensitivity }`.
- Artifact id: same derivation as `InProcessArtifactStore` (first 16 bytes of `Digest::blob_content(content)` → `ArtifactId::from_bytes`). Logical key: `ObjectKey::try_new(format!("artifacts/{}", digest.to_hex()))`.
- `stage_put` → `put(PutPayload::Bytes)`; on success rebuild `ArtifactRef` exactly as `InProcessArtifactStore` does (`BlobRef::try_new(digest.to_hex(), media_type, len, Some(digest), name)` then `ArtifactRef::try_new(artifact_id, kind, blob, digest, scope_digest, attributes)` — copy the exact call from `extensions/context/finstack-ai-context-memory/src/lib.rs:90-120`). The artifact `kind` and `attributes` don't survive the object round-trip as authoritative data; store them in the object's `ObjectMetadata.attributes` verbatim so `get` can revalidate.
- `get` → `ObjectStore::get` by recomputing the key from `artifact.content_digest()`; verify returned bytes hash to the ref's digest (`Integrity` otherwise).
- Errors: `NotFound→NotFound`, `TooLarge{len,max}→TooLarge` (usize-cast), `ScopeMismatch→ScopeMismatch`, `Integrity→Integrity`, `InvalidKey|InvalidMetadata→InvalidMetadata`, `Unavailable|Io|Unsupported→Unavailable`.
- `limits()` returns the configured 64 MiB.

- [ ] **Step 1: Write failing tests** (`src/tests.rs`):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn stage_and_get_round_trip_through_the_object_store() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectStore::default()));
    let content = Bytes::from(vec![9_u8; 5 * 1024 * 1024]); // > old 4 MiB cap
    let artifact = stage_required_artifact(&adapter, scope(), content.clone(), metadata())
        .await
        .expect("staged past the old ceiling");
    let read = adapter.get(scope(), artifact).await.expect("get");
    assert_eq!(read, content);
}

#[tokio::test(flavor = "multi_thread")]
async fn default_limit_is_64_mib_and_enforced() {
    let adapter = ObjectArtifactStore::new(Arc::new(FakeObjectStore::default()));
    assert_eq!(adapter.limits().max_artifact_bytes, 64 * 1024 * 1024);
    let oversize = Bytes::from(vec![0_u8; 64 * 1024 * 1024 + 1]);
    let error = stage_required_artifact(&adapter, scope(), oversize, metadata())
        .await
        .expect_err("must reject");
    assert_eq!(error.code(), ARTIFACT_TOO_LARGE);
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_scope_get_fails_closed() { /* stage in scope a, get with scope b -> ScopeMismatch */ }

#[tokio::test(flavor = "multi_thread")]
async fn object_errors_map_to_artifact_codes() { /* injected integrity -> ARTIFACT_INTEGRITY_FAILURE */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p finstack-ai-store-artifact-object` — FAIL.

- [ ] **Step 3: Implement; run to pass.** `cargo nextest run -p finstack-ai-store-artifact-object` + clippy gate.

- [ ] **Step 4: Commit**

```bash
git add extensions/stores/finstack-ai-store-artifact-object Cargo.toml Cargo.lock
git commit -m "feat(stores): ArtifactStore adapter over ObjectStore with 64 MiB default ceiling"
```

---

### Task 9: Host wiring — `RuntimeServices.object_store` + `HostFeature::ObjectStore`

**Files:**
- Modify: `crates/finstack-ai/src/bundle/catalog.rs` (struct + `validate`)
- Modify: `crates/finstack-ai/src/bundle/types.rs:65` (`HostFeature` enum)
- Modify: `crates/finstack-ai/src/bundle/expand.rs:249-252` (`required_services`) and the `RequiredServices` struct it fills
- Modify: `crates/finstack-ai/src/bundle/resolver.rs:501-506` (`host_feature_name`)
- Test: existing bundle test module (find via `grep -rn "artifact_store" crates/finstack-ai/src/bundle/*tests*` and mirror the artifact-store cases)

**Interfaces:**
- Consumes: `finstack_ai_runtime::ObjectStore`.
- Produces: `RuntimeServices { ..., pub object_store: Option<Arc<dyn ObjectStore>> }`; `HostFeature::ObjectStore` (serde `snake_case` → `"object_store"`); requirement name string `"object_store"`.

- [ ] **Step 1: Write failing test** (mirror the existing artifact-store validation test in the bundle tests):

```rust
#[test]
fn missing_required_object_store_fails_resolution() {
    // bundle with RequiredHostFeature { feature: HostFeature::ObjectStore },
    // RuntimeServices::default() -> BundleResolutionError::Missing { item: "object_store" }
}

#[test]
fn present_object_store_satisfies_the_requirement() {
    // services.object_store = Some(Arc::new(FakeObjectStore::default())) -> Ok
}
```

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p finstack-ai bundle` — FAIL (no variant/field).

- [ ] **Step 3: Implement** — add the enum variant (with doc `/// Scoped object storage service.`), the struct field, one tuple in `validate`'s array, one arm in `required_services`, one arm in `host_feature_name`. `RequiredServices` gains `pub object_store: bool`.

- [ ] **Step 4: Run tests + public-API regen**

Run: `cargo nextest run -p finstack-ai bundle && mise run check-public-api` (update `finstack-ai.txt` baseline).

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai fixtures/compatibility/public-rust-api
git commit -m "feat(sdk): object_store host service wiring in RuntimeServices and HostFeature"
```

---

### Task 10: Integration lane — >4 MiB PDF through S3-backed artifacts

**Files:**
- Modify: `crates/finstack-ai-test/tests/lanes/document_ingest.rs` (add one test)
- Modify: `crates/finstack-ai-test/Cargo.toml` (dev-dep on `finstack-ai-store-artifact-object`; keep it out of the library's dependency graph — dev-dependencies only)

**Interfaces:**
- Consumes: `ObjectArtifactStore` (Task 8), `FakeObjectStore` (Task 4), the existing lane's builder scaffolding.

- [ ] **Step 1: Write the test** (adapt the lane's existing setup — reuse its agent/middleware builder helpers, swapping the store):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn large_attachment_stages_through_the_object_backed_artifact_store() {
    let object_store = Arc::new(FakeObjectStore::default());
    let store: Arc<dyn ArtifactStore> = Arc::new(ObjectArtifactStore::new(object_store));
    // 6 MiB synthetic attachment (vec![0x25; 6 * 1024 * 1024], media type application/pdf)
    // run the lane's existing ingest path with this store;
    // assert the run succeeds and the staged ArtifactRef length is 6 MiB.
}
```

- [ ] **Step 2: Run to verify it fails, then passes** — first run may fail if the lane's default limits still pin 4 MiB anywhere; that is exactly the regression this test exists to catch. Fix whatever it flushes out (Task 3 should have covered it). Run: `cargo nextest run -p finstack-ai-test --test lanes document_ingest` — expect PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/finstack-ai-test
git commit -m "test(lanes): large attachment through object-backed artifact store"
```

---

### Task 11: ADR-050, register row, README, spec reconciliation

**Files:**
- Create: `docs/implementation/adrs/ADR-050-object-store-contract.md` (copy the section skeleton of `ADR-048-shared-authority-and-provider-secret.md`)
- Modify: `docs/implementation/adr-register.md` (row after ADR-049, same column format as ADR-048's row)
- Modify: `extensions/stores/README.md` (add the three crates to the store table with one-line descriptions)
- Modify: `docs/superpowers/specs/2026-08-19-media-pipeline-design.md` §4.3 and `docs/superpowers/plans/2026-08-19-media-pipeline.md` Phase E

**ADR-050 content requirements** (write fully, no placeholders): context (large unstructured data, 4 MiB artifact ceiling, two pre-scoped S3 surfaces); decision (host-supplied `ObjectStore` service; scoped fail-closed keys `{prefix}/{scope-digest-16-hex}/{key}`; frozen `object_*` codes exactly as in Task 1; per-store `ArtifactStoreLimits`; hand-rolled SigV4 with `hmac` as the only new dependency; 64 MiB adapter default / 5 GiB object default); the `ObjectError`→`ArtifactError` mapping table from Task 8; rejected alternatives (seventh port — violates the six-kind boundary; aws-sdk-s3 — minimal-graph impact; opendal — abstraction layer contrary to repo philosophy; raw unscoped keys — every consumer re-implements isolation); consequences (multipart deferred, presign unsupported on local backend); reconsideration conditions (multipart when objects exceed 5 GiB; content-addressed shared namespace when SourceStore lands).

**Media-pipeline amendment:** in the spec's §4.3, replace the `finstack-ai-store-media-s3` bullet with: "`S3MediaStore` adapts `Arc<dyn ObjectStore>` (see `2026-08-19-object-store-design.md`): `put_file` → `put(PutPayload::File)`, `materialize` → `get_to_file`, `presign_get` → `presign_get`. SigV4 lives in `finstack-ai-store-object-s3`." In the plan, mark Phase E Tasks 14–15 as superseded with a pointer to this plan (strike the tasks, add a `> Superseded 2026-08-19:` note; do not delete the text).

- [ ] **Step 1: Write all four documents/edits**
- [ ] **Step 2: Self-check** — grep the ADR for every frozen code string and confirm each matches `object.rs` exactly: `grep -o 'object_[a-z_]*' docs/implementation/adrs/ADR-050-object-store-contract.md | sort -u` vs the nine codes.
- [ ] **Step 3: Commit**

```bash
git add docs/implementation extensions/stores/README.md docs/superpowers
git commit -m "docs: ADR-050 object-store contract and media-pipeline reconciliation"
```

---

### Task 12: Full verification sweep

**Files:** none new.

- [ ] **Step 1: Workspace gates, checked by exit code**

```bash
cargo nextest run --workspace
cargo test --doc --workspace
cargo clippy --workspace --all-targets -- -D warnings
mise run check-public-api
cargo deny check licenses advisories
```

Expected: all zero exit codes. `cargo deny` validates the `hmac` addition (BSD/MIT/Apache — passes the policy; if it flags, add the exception the tool suggests to `deny.toml` ONLY after confirming the license is MIT OR Apache-2.0).

- [ ] **Step 2: Minimal-graph check** — build the CI fixture leaves to prove the new crates didn't enlarge the minimal graph:

```bash
cargo check -p extension-port-leaf -p model-port-leaf -p toolset-port-leaf
cargo tree -p finstack-ai-runtime | grep -c hmac
```

Expected: fixture checks pass; the `grep -c` prints `0` (hmac must appear only under `finstack-ai-store-object-s3`).

- [ ] **Step 3: Run `mise run ci-rust`** — expect green.

- [ ] **Step 4: Final commit (only if the sweep touched anything)**

```bash
git add -A
git commit -m "chore: verification sweep for object store service"
```

---

## Self-Review Notes (performed at plan-writing time)

- **Spec coverage:** §2 crates/wiring → Tasks 1, 5–9; §3 trait/types → Task 1; §4 S3 → Tasks 6–7; §5 local → Task 5; §6.1 limits → Tasks 2–3; §6.2 adapter → Task 8; §6.3 reconciliation → Task 11; §8 errors → Tasks 1, 7, 8; §9 testing → Tasks 4, 7, 10, 12. ADR (§2) → Task 11. No uncovered spec sections.
- **Type consistency:** `ObjectStoreLimits.max_object_bytes: u64` everywhere; `ArtifactStoreLimits.max_artifact_bytes: usize` (matches `content.len()`); `validate_staged_artifact` gains `&ArtifactStoreLimits` and Task 2 Step 3 sweeps all callers via `cargo check --workspace`.
- **Known risk:** the AWS known-answer signatures in Task 6 must be transcribed from the AWS SigV4 documentation examples at implementation time if these constants disagree with the doc — the doc is authoritative, the test comment carries the URL context.
