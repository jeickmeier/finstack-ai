# Media Generation Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an AI-plannable video generation pipeline: a `MediaStore` runtime contract with local and S3 backends, an async video-job tool trio on the OpenRouter media toolset, a declarative ffmpeg composition toolset, and a resumable `MoviePlan` pipeline driver exposed as `render_movie`/`advance_render`/`get_render_status` tools.

**Architecture:** Layer 1 adds a host-supplied `MediaStore` service trait to `finstack-ai-runtime` (sibling of `ArtifactStore`, ADR-048-style host object) plus two backend leaves under `extensions/stores/`. Layer 2 extends the completed `finstack-ai-tools-openrouter-media` crate with `openrouter_submit_video` / `openrouter_get_video_job` / `openrouter_download_video` and store-backed image results. Layer 3 is a new T1 toolset shelling out to a host-supplied ffmpeg with a declarative composition spec. Layer 4 is a tick-based pipeline driver with adapter-owned sqlite state (workflow-local cron-table precedent) wrapped in a toolset. Media bytes move by local file path and opaque digest-verified `MediaRef`s; they never pass through tool-result JSON or the journal.

**Tech Stack:** Rust workspace (edition/lints inherited), `reqwest` 0.13, `tokio`, `serde`/`serde_json`, `rusqlite` (workspace-pinned), `base64` (workspace-pinned), `tempfile` (workspace-pinned), `sha2` (workspace-pinned), one new workspace dependency `hmac = "0.12"` used only by the S3 leaf.

**Spec:** `docs/superpowers/specs/2026-08-19-media-pipeline-design.md`

## Global Constraints

- **Prerequisite:** the OpenRouter provider plan (`docs/superpowers/plans/2026-08-19-openrouter-provider.md`) has run to completion. `finstack-ai-provider-openrouter` and `finstack-ai-tools-openrouter-media` exist; `MediaResolver` exists in `finstack_ai_runtime::provider_util`. Tasks 4–5 edit the media toolset crate that plan created. Nothing is published, so its `openrouter_generate_video` tool is removed and replaced without a deprecation shim.
- Workspace layout is contractual (`AGENTS.md`, `.agents/rules/`): stores live in `extensions/stores/finstack-ai-store-media-<name>`, toolsets in `extensions/toolsets/`, workflow drivers in `extensions/workflow/`; kernel/runtime never depend on an extension crate.
- One new external dependency total: `hmac = { version = "0.12", default-features = false }` in the workspace root, consumed only by `finstack-ai-store-media-s3`. Everything else uses workspace-pinned `reqwest`, `tokio`, `serde`, `serde_json`, `futures-util`, `thiserror`, `rusqlite`, `base64`, `sha2`, `tempfile`.
- Extensions never read environment variables; credentials, endpoints, binary paths, and roots are explicit construction inputs (ADR-048). Non-loopback HTTP endpoints must be HTTPS. All `Debug` impls redact secrets (canary test mandatory). Response bodies are never surfaced in errors.
- Frozen error codes (each `&'static str`, never changed after merge):
  - runtime: `media_unavailable`, `media_not_found`, `media_scope_mismatch`, `media_integrity_failure`, `media_too_large`, `media_invalid_metadata`, `media_io_failure`
  - openrouter media additions: `openrouter_media_store_required` (existing `openrouter_media_*` codes reused)
  - compose: `video_compose_config_invalid`, `video_compose_invalid_arguments`, `video_compose_spec_invalid`, `video_compose_ffmpeg_failed`, `video_compose_timeout`, `video_compose_media_failure`, `video_compose_limit_exceeded`
  - pipeline: `media_pipeline_config_invalid`, `media_pipeline_invalid_arguments`, `media_pipeline_plan_invalid`, `media_pipeline_budget_exceeded`, `media_pipeline_store_failure`, `media_pipeline_stage_failed`, `media_pipeline_not_found`
- Frozen tool identities: `finstack.tools.openrouter_submit_video`/`openrouter_submit_video`, `finstack.tools.openrouter_get_video_job`/`openrouter_get_video_job`, `finstack.tools.openrouter_download_video`/`openrouter_download_video`, `finstack.tools.compose_video`/`compose_video`, `finstack.tools.probe_media`/`probe_media`, `finstack.tools.render_movie`/`render_movie`, `finstack.tools.advance_render`/`advance_render`, `finstack.tools.get_render_status`/`get_render_status`.
- Every new crate copies the lint header block (the `#![warn(missing_docs)]` … `#![doc(test(attr(allow(clippy::expect_used))))]` block) verbatim from `extensions/toolsets/finstack-ai-sandbox-e2b/src/lib.rs:7-26` and carries `[lints] workspace = true` in its manifest.
- Media bytes never enter tool-result JSON, the journal, or `ArtifactStore`. Tool results carry `MediaRef` JSON objects (id, hex digest, length, media type, hex scope digest) — bounded and self-verifying.
- Verification gate before claiming completion of any task: the task's listed test command passes; before claiming completion of the plan: `mise run ci-rust` (Task 16 adds `ci-python`/`ci-wasm`).
- Commit style: plain imperative sentences (match `git log`), each ending with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Wire-shape re-verification: before implementing Task 4, fetch the current OpenRouter video-generation reference (`https://openrouter.ai/docs/guides/overview/multimodal/video-generation`). The tool surface (names, bounded JSON results, error codes) is fixed by this plan; only private wire DTOs may move to match live docs.

---

## Phase A — MediaStore contract and local backend

### Task 1: Streaming blob hasher (kernel) + `MediaStore` contract (runtime) + ADR

**Files:**
- Modify: `crates/finstack-ai-kernel/src/primitives/digest.rs` (add `BlobContentHasher`)
- Modify: `crates/finstack-ai-kernel/src/lib.rs` (re-export `BlobContentHasher` next to `Digest`)
- Create: `crates/finstack-ai-runtime/src/services/media.rs`
- Modify: `crates/finstack-ai-runtime/src/services/mod.rs` (add `pub(crate) mod media;`)
- Modify: `crates/finstack-ai-runtime/src/lib.rs` (re-export, next to the `services::artifact` re-export at ~line 116)
- Create: `docs/implementation/adrs/ADR-049-media-store-contract.md`
- Modify: `docs/implementation/adr-register.md` (append ADR-049 row)

**Interfaces:**
- Produces (kernel): `BlobContentHasher::new()`, `fn update(&mut self, chunk: &[u8])`, `fn finish(self) -> Digest` — incremental equivalent of `Digest::blob_content`.
- Produces (runtime): `MediaRef` (`try_new(id, digest, length, media_type, scope_digest) -> Result<Self, MediaError>` + accessors `id()`, `digest()`, `length()`, `media_type()`, `scope_digest()`; `Serialize`/`Deserialize` with `deny_unknown_fields`), `MediaMetadata { media_type: Arc<str>, name: Option<Arc<str>>, attributes: Metadata }`, `MaterializedMedia` (`new(local_path: PathBuf)`, `with_guard(local_path, guard: Box<dyn core::any::Any + Send>)`, accessor `local_path()`), `trait MediaStore: PortObject` with `put_file`, `materialize`, `presign_get`, `delete`, helper `pub fn hash_file_blob(path: &Path) -> Result<(Digest, u64), MediaError>`, `MediaError` with `code()`, and the seven `MEDIA_*` code constants.

- [ ] **Step 1: Write the failing kernel test**

Append to the existing `#[cfg(test)] mod tests` in `crates/finstack-ai-kernel/src/primitives/digest.rs`:

```rust
    #[test]
    fn blob_content_hasher_matches_one_shot_blob_content() {
        let bytes = b"streaming blob content bytes";
        let mut hasher = BlobContentHasher::new();
        hasher.update(&bytes[..7]);
        hasher.update(&bytes[7..]);
        assert_eq!(hasher.finish(), Digest::blob_content(bytes));
        assert_eq!(
            BlobContentHasher::new().finish(),
            Digest::blob_content(b"")
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p finstack-ai-kernel blob_content_hasher`
Expected: FAIL — `BlobContentHasher` not found.

- [ ] **Step 3: Implement `BlobContentHasher`**

In `digest.rs`, find the private function that `Digest::blob_content` uses to build the domain prefix (`"finstack-ai" NUL domain NUL schema-version-u32-be NUL`, domain `DOMAIN_BLOB_CONTENT`, version `BLOB_CONTENT_DIGEST_SCHEMA_VERSION`). Add, reusing that exact prefix encoding:

```rust
/// Incremental domain-separated blob-content hasher.
///
/// Produces exactly the digest of [`Digest::blob_content`] without holding
/// the full content in memory. Used for large media files.
#[derive(Debug)]
pub struct BlobContentHasher {
    hasher: Sha256,
}

impl BlobContentHasher {
    /// Start a blob-content digest with the domain prefix absorbed.
    #[must_use]
    pub fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"finstack-ai");
        hasher.update([0]);
        hasher.update(DOMAIN_BLOB_CONTENT.as_bytes());
        hasher.update([0]);
        hasher.update(BLOB_CONTENT_DIGEST_SCHEMA_VERSION.to_be_bytes());
        hasher.update([0]);
        Self { hasher }
    }

    /// Absorb one content chunk.
    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
    }

    /// Finish and return the blob-content digest.
    #[must_use]
    pub fn finish(self) -> Digest {
        Digest(self.hasher.finalize().into())
    }
}

impl Default for BlobContentHasher {
    fn default() -> Self {
        Self::new()
    }
}
```

**Important:** open the existing `Digest::blob_content` implementation first and copy its prefix-building statements exactly (separator bytes and ordering). If it routes through a shared `domain_separated` helper, mirror that helper's byte sequence; the test in Step 1 is the equality oracle. Re-export from `crates/finstack-ai-kernel/src/lib.rs` next to `Digest`.

- [ ] **Step 4: Run kernel tests**

Run: `cargo test -p finstack-ai-kernel digest`
Expected: PASS including the new test.

- [ ] **Step 5: Write `crates/finstack-ai-runtime/src/services/media.rs`**

Model the file's tone on `services/artifact.rs`. Full content:

```rust
//! Scoped large-object media storage contract (`ArtifactStore`'s sibling).
//!
//! `ArtifactStore` stores exact bytes with a 4 MiB ceiling. `MediaStore`
//! stores large media (video clips, rendered movies, large images) by local
//! file path and hands out opaque, digest-verified [`MediaRef`]s. Backends
//! (local filesystem, S3) are host composition; agents never see or choose
//! the backend. Introduced by ADR-049 as a host-supplied object, not a
//! registered port.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use finstack_ai_kernel::BlobContentHasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Digest, Metadata, PortFuture, PortObject};

use super::artifact::ArtifactScope;

/// Stable code for an unavailable media service.
pub const MEDIA_UNAVAILABLE: &str = "media_unavailable";
/// Stable code for a missing media object.
pub const MEDIA_NOT_FOUND: &str = "media_not_found";
/// Stable code for a scope-binding failure.
pub const MEDIA_SCOPE_MISMATCH: &str = "media_scope_mismatch";
/// Stable code for a content integrity failure.
pub const MEDIA_INTEGRITY_FAILURE: &str = "media_integrity_failure";
/// Stable code for content above a backend ceiling.
pub const MEDIA_TOO_LARGE: &str = "media_too_large";
/// Stable code for malformed media metadata.
pub const MEDIA_INVALID_METADATA: &str = "media_invalid_metadata";
/// Stable code for a local filesystem failure.
pub const MEDIA_IO_FAILURE: &str = "media_io_failure";

/// Metadata mapped onto a stored media object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaMetadata {
    /// Media type, e.g. `video/mp4`.
    pub media_type: Arc<str>,
    /// Optional display name.
    pub name: Option<Arc<str>>,
    /// Bounded non-secret, non-authoritative attributes.
    pub attributes: Metadata,
}

/// Opaque, serializable, digest-verified reference to stored media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaRef {
    id: Arc<str>,
    digest: Digest,
    length: u64,
    media_type: Arc<str>,
    scope_digest: Digest,
}

impl MediaRef {
    /// Construct a validated reference.
    ///
    /// # Errors
    ///
    /// Rejects an empty or NUL-bearing id or media type, and zero length.
    pub fn try_new(
        id: impl AsRef<str>,
        digest: Digest,
        length: u64,
        media_type: impl AsRef<str>,
        scope_digest: Digest,
    ) -> Result<Self, MediaError> {
        let id = id.as_ref();
        let media_type = media_type.as_ref();
        if id.is_empty()
            || id.as_bytes().contains(&0)
            || media_type.is_empty()
            || media_type.as_bytes().contains(&0)
            || length == 0
        {
            return Err(MediaError::InvalidMetadata {
                message: Arc::from("media_ref_field_invalid"),
            });
        }
        Ok(Self {
            id: Arc::from(id),
            digest,
            length,
            media_type: Arc::from(media_type),
            scope_digest,
        })
    }

    /// Backend-scoped object key. Never a raw path or URL.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Blob-content digest of the exact stored bytes.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Exact content length in bytes.
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    /// Stored media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Frozen [`ArtifactScope`] binding digest.
    #[must_use]
    pub const fn scope_digest(&self) -> Digest {
        self.scope_digest
    }
}

/// A readable local file for one materialized media object.
///
/// The optional guard owns backend cleanup (e.g. a temp download) and runs
/// on drop. Keep the value alive while reading `local_path`.
pub struct MaterializedMedia {
    local_path: PathBuf,
    _guard: Option<Box<dyn core::any::Any + Send>>,
}

impl core::fmt::Debug for MaterializedMedia {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MaterializedMedia")
            .field("local_path", &self.local_path)
            .finish_non_exhaustive()
    }
}

impl MaterializedMedia {
    /// A stored file readable in place (no cleanup guard).
    #[must_use]
    pub const fn new(local_path: PathBuf) -> Self {
        Self {
            local_path,
            _guard: None,
        }
    }

    /// A temporary file whose guard cleans up on drop.
    #[must_use]
    pub fn with_guard(local_path: PathBuf, guard: Box<dyn core::any::Any + Send>) -> Self {
        Self {
            local_path,
            _guard: Some(guard),
        }
    }

    /// The readable local file path.
    #[must_use]
    pub fn local_path(&self) -> &Path {
        &self.local_path
    }
}

/// Scoped large-object media service. Host-supplied; not a port.
pub trait MediaStore: PortObject {
    /// Ingest a local file's exact bytes into the store.
    fn put_file(
        &self,
        scope: ArtifactScope,
        local_path: PathBuf,
        metadata: MediaMetadata,
    ) -> PortFuture<Result<MediaRef, MediaError>>;

    /// Produce a readable, digest-verified local file for a reference.
    fn materialize(
        &self,
        scope: ArtifactScope,
        media: &MediaRef,
    ) -> PortFuture<Result<MaterializedMedia, MediaError>>;

    /// Time-limited HTTPS GET URL when the backend supports presigning.
    ///
    /// `Ok(None)` means the backend cannot presign (e.g. local filesystem);
    /// callers needing an outbound URL fall back to a bounded `data:` URI
    /// built from [`Self::materialize`].
    fn presign_get(
        &self,
        scope: ArtifactScope,
        media: &MediaRef,
    ) -> PortFuture<Result<Option<Arc<str>>, MediaError>>;

    /// Permanently remove stored content.
    fn delete(
        &self,
        scope: ArtifactScope,
        media: &MediaRef,
    ) -> PortFuture<Result<(), MediaError>>;
}

/// Stream-hash a local file into `(blob-content digest, length)`.
///
/// # Errors
///
/// Returns [`MediaError::Io`] when the file cannot be opened or read.
pub fn hash_file_blob(path: &Path) -> Result<(Digest, u64), MediaError> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| MediaError::Io {
        message: Arc::from("media_file_open_failed"),
    })?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = BlobContentHasher::new();
    let mut length: u64 = 0;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| MediaError::Io {
            message: Arc::from("media_file_read_failed"),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or(&[]));
        length = length.saturating_add(read as u64);
    }
    Ok((hasher.finish(), length))
}

/// Verify a scope binding and a materialized file against a reference.
///
/// # Errors
///
/// Returns [`MediaError::ScopeMismatch`] for a foreign scope and
/// [`MediaError::Integrity`] for a digest or length mismatch.
pub fn verify_materialized(
    scope: &ArtifactScope,
    media: &MediaRef,
    local_path: &Path,
) -> Result<(), MediaError> {
    let expected_scope = scope.digest().map_err(|_| MediaError::InvalidMetadata {
        message: Arc::from("media_scope_invalid"),
    })?;
    if media.scope_digest() != expected_scope {
        return Err(MediaError::ScopeMismatch {
            expected: expected_scope,
            actual: media.scope_digest(),
        });
    }
    let (digest, length) = hash_file_blob(local_path)?;
    if digest != media.digest() || length != media.length() {
        return Err(MediaError::Integrity {
            message: Arc::from("media_content_mismatch"),
        });
    }
    Ok(())
}

/// Media service or integrity failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MediaError {
    /// Service unavailable.
    #[error("{}: {message}", MEDIA_UNAVAILABLE)]
    Unavailable {
        /// Bounded diagnostic.
        message: Arc<str>,
    },
    /// Referenced media is missing.
    #[error("{}: media object is missing", MEDIA_NOT_FOUND)]
    NotFound,
    /// Requested scope differs from the reference's frozen binding.
    #[error("{}: expected scope {expected}, actual scope {actual}", MEDIA_SCOPE_MISMATCH)]
    ScopeMismatch {
        /// Requested scope digest.
        expected: Digest,
        /// Reference scope digest.
        actual: Digest,
    },
    /// Content digest or length mismatch.
    #[error("{}: {message}", MEDIA_INTEGRITY_FAILURE)]
    Integrity {
        /// Stable diagnostic.
        message: Arc<str>,
    },
    /// Content exceeds a backend ceiling.
    #[error("{}: media has {len} bytes; maximum is {max}", MEDIA_TOO_LARGE)]
    TooLarge {
        /// Submitted bytes.
        len: u64,
        /// Maximum bytes.
        max: u64,
    },
    /// Metadata or reference field is malformed.
    #[error("{}: {message}", MEDIA_INVALID_METADATA)]
    InvalidMetadata {
        /// Stable diagnostic.
        message: Arc<str>,
    },
    /// Local filesystem failure.
    #[error("{}: {message}", MEDIA_IO_FAILURE)]
    Io {
        /// Stable diagnostic.
        message: Arc<str>,
    },
}

impl MediaError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => MEDIA_UNAVAILABLE,
            Self::NotFound => MEDIA_NOT_FOUND,
            Self::ScopeMismatch { .. } => MEDIA_SCOPE_MISMATCH,
            Self::Integrity { .. } => MEDIA_INTEGRITY_FAILURE,
            Self::TooLarge { .. } => MEDIA_TOO_LARGE,
            Self::InvalidMetadata { .. } => MEDIA_INVALID_METADATA,
            Self::Io { .. } => MEDIA_IO_FAILURE,
        }
    }
}
```

Note: if `Digest` in `MediaError`'s `#[error]` strings does not implement `Display`, format with `expected.to_hex()`/`actual.to_hex()` instead — check how `ArtifactError::ScopeMismatch` renders and copy that.

Add the inline test module:

```rust
#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{RunId, SessionId, Sensitivity};

    use super::*;

    fn scope() -> ArtifactScope {
        ArtifactScope {
            tenant_scope: Arc::from("tenant-a"),
            session_id: SessionId::from_bytes([1; 16]),
            run_id: Some(RunId::from_bytes([2; 16])),
            sensitivity: Sensitivity::Confidential,
        }
    }

    #[test]
    fn media_ref_rejects_invalid_fields() {
        let digest = Digest::raw_json(b"{}");
        for (id, media_type, length) in [
            ("", "video/mp4", 1_u64),
            ("key", "", 1),
            ("key", "video/mp4", 0),
            ("k\0ey", "video/mp4", 1),
        ] {
            let error = MediaRef::try_new(id, digest, length, media_type, digest)
                .expect_err("invalid field");
            assert_eq!(error.code(), MEDIA_INVALID_METADATA);
        }
    }

    #[test]
    fn media_ref_round_trips_through_serde() {
        let digest = Digest::raw_json(b"{}");
        let media =
            MediaRef::try_new("clips/abc", digest, 42, "video/mp4", digest).expect("media ref");
        let json = serde_json::to_string(&media).expect("serialize");
        let back: MediaRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, media);
        assert!(
            serde_json::from_str::<MediaRef>(&json.replace('}', r#","extra":1}"#)).is_err(),
            "unknown fields must be rejected"
        );
    }

    #[test]
    fn hashing_and_verification_agree_and_fail_closed() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("clip.bin");
        std::fs::write(&path, b"clip bytes").expect("write");
        let (digest, length) = hash_file_blob(&path).expect("hash");
        assert_eq!(digest, finstack_ai_kernel::Digest::blob_content(b"clip bytes"));
        assert_eq!(length, 10);

        let scope = scope();
        let scope_digest = scope.digest().expect("scope digest");
        let media = MediaRef::try_new("k", digest, length, "video/mp4", scope_digest)
            .expect("media ref");
        verify_materialized(&scope, &media, &path).expect("verify");

        std::fs::write(&path, b"tampered!!").expect("tamper");
        let error = verify_materialized(&scope, &media, &path).expect_err("tampered");
        assert_eq!(error.code(), MEDIA_INTEGRITY_FAILURE);

        let foreign = ArtifactScope {
            tenant_scope: Arc::from("tenant-b"),
            ..scope
        };
        let error = verify_materialized(&foreign, &media, &path).expect_err("foreign scope");
        assert_eq!(error.code(), MEDIA_SCOPE_MISMATCH);
    }
}
```

Add `tempfile = { workspace = true }` to `crates/finstack-ai-runtime/Cargo.toml` `[dev-dependencies]` if not already present.

- [ ] **Step 6: Wire the module and exports**

In `crates/finstack-ai-runtime/src/services/mod.rs`, add `pub(crate) mod media;` after `pub(crate) mod interaction;` (alphabetical). In `crates/finstack-ai-runtime/src/lib.rs`, next to the `services::artifact` re-export (~line 116), add:

```rust
pub use services::media::{
    MEDIA_INTEGRITY_FAILURE, MEDIA_INVALID_METADATA, MEDIA_IO_FAILURE, MEDIA_NOT_FOUND,
    MEDIA_SCOPE_MISMATCH, MEDIA_TOO_LARGE, MEDIA_UNAVAILABLE, MaterializedMedia, MediaError,
    MediaMetadata, MediaRef, MediaStore, hash_file_blob, verify_materialized,
};
```

- [ ] **Step 7: Run runtime tests**

Run: `cargo test -p finstack-ai-runtime media`
Expected: PASS (all three new tests).

- [ ] **Step 8: Write ADR-049**

Create `docs/implementation/adrs/ADR-049-media-store-contract.md` following the section shape of `docs/implementation/adrs/ADR-048-shared-authority-and-provider-secret.md` (status/date/owners, context, decision, consequences, rejected alternatives, compatibility classification, affected requirements, supersession metadata, approval links). Content requirements: decision is the `MediaStore` host-supplied contract as specified in spec §4 (streaming-by-path, `ArtifactScope` binding, digest-verified materialize, `presign_get` optionality); rejected alternatives are (a) raising `ArtifactStore`'s 4 MiB ceiling, (b) pointer-manifest artifacts without a contract, (c) native external-blob refs inside `ArtifactStore`; consequences include the one-new-dependency note (`hmac` for the S3 leaf) and that backends are extension leaves. Append the ADR-049 row to the table in `docs/implementation/adr-register.md`, matching the ADR-048 row's column format with owner `me@jeickmeier.com` and today's date.

- [ ] **Step 9: Commit**

```bash
git add crates/finstack-ai-kernel/src crates/finstack-ai-runtime docs/implementation/adrs/ADR-049-media-store-contract.md docs/implementation/adr-register.md
git commit -m "Add the MediaStore runtime contract with streaming blob hashing"
```

---

### Task 2: Fake `MediaStore` in `finstack-ai-test`

**Files:**
- Create: `crates/finstack-ai-test/src/fakes/media.rs`
- Modify: `crates/finstack-ai-test/src/fakes/mod.rs` (declare and re-export)
- Modify: `crates/finstack-ai-test/src/lib.rs` (extend the `pub use fakes::{...}` list)
- Modify: `crates/finstack-ai-test/Cargo.toml` (add `tempfile = { workspace = true }` to `[dependencies]` if absent)

**Interfaces:**
- Consumes: Task 1's `MediaStore`, `MediaRef`, `MediaMetadata`, `MediaError`, `MaterializedMedia`, `ArtifactScope`, `hash_file_blob`.
- Produces: `pub struct FakeMediaStore` with `pub fn new() -> Self` (tempdir-backed), `pub fn with_presign_base(self, base: &str) -> Self` (makes `presign_get` return `Some("{base}/{id}")`), `pub fn object_count(&self) -> usize`, `pub fn corrupt(&self, media: &MediaRef)` (rewrites stored bytes so materialize fails), and `impl MediaStore`.

- [ ] **Step 1: Write the failing test**

Inline `#[cfg(test)] mod tests` in the new `fakes/media.rs`:

```rust
    #[tokio::test]
    async fn fake_store_round_trips_and_fails_closed() {
        let store = FakeMediaStore::new();
        let scope = ArtifactScope {
            tenant_scope: Arc::from("tenant-a"),
            session_id: SessionId::from_bytes([1; 16]),
            run_id: None,
            sensitivity: Sensitivity::Confidential,
        };
        let dir = tempfile::tempdir().expect("dir");
        let source = dir.path().join("in.mp4");
        std::fs::write(&source, b"fake mp4 bytes").expect("write");
        let media = store
            .put_file(
                scope.clone(),
                source,
                MediaMetadata {
                    media_type: Arc::from("video/mp4"),
                    name: None,
                    attributes: Metadata::empty(),
                },
            )
            .await
            .expect("put");
        assert_eq!(media.length(), 14);
        let materialized = store.materialize(scope.clone(), &media).await.expect("get");
        assert_eq!(
            std::fs::read(materialized.local_path()).expect("read"),
            b"fake mp4 bytes"
        );
        assert!(
            store
                .presign_get(scope.clone(), &media)
                .await
                .expect("presign")
                .is_none()
        );

        let foreign = ArtifactScope {
            tenant_scope: Arc::from("tenant-b"),
            ..scope.clone()
        };
        let error = store
            .materialize(foreign, &media)
            .await
            .expect_err("foreign scope");
        assert_eq!(error.code(), MEDIA_SCOPE_MISMATCH);

        store.corrupt(&media);
        let error = store
            .materialize(scope.clone(), &media)
            .await
            .expect_err("corrupt");
        assert_eq!(error.code(), MEDIA_INTEGRITY_FAILURE);

        let presigning = FakeMediaStore::new().with_presign_base("https://cdn.test");
        let source2 = dir.path().join("in2.png");
        std::fs::write(&source2, b"png").expect("write");
        let image = presigning
            .put_file(
                scope.clone(),
                source2,
                MediaMetadata {
                    media_type: Arc::from("image/png"),
                    name: None,
                    attributes: Metadata::empty(),
                },
            )
            .await
            .expect("put");
        let url = presigning
            .presign_get(scope, &image)
            .await
            .expect("presign")
            .expect("some url");
        assert!(url.starts_with("https://cdn.test/"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-test fake_store_round_trips`
Expected: FAIL — `FakeMediaStore` not found. (If `finstack-ai-test` lacks `tokio` with `macros`/`rt` in dev-dependencies, add `tokio = { workspace = true, features = ["macros", "rt"] }`.)

- [ ] **Step 3: Implement `FakeMediaStore`**

```rust
//! Deterministic in-process [`MediaStore`] fake backed by a tempdir.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use finstack_ai_runtime::{
    ArtifactScope, MaterializedMedia, MediaError, MediaMetadata, MediaRef, MediaStore, PortFuture,
    hash_file_blob,
};

struct StoredMedia {
    path: PathBuf,
    media_type: Arc<str>,
}

/// Tempdir-backed [`MediaStore`] fake for toolset and driver tests.
pub struct FakeMediaStore {
    root: tempfile::TempDir,
    objects: Mutex<BTreeMap<Arc<str>, StoredMedia>>,
    next_id: Mutex<u64>,
    presign_base: Option<Arc<str>>,
}

impl FakeMediaStore {
    /// Empty fake store.
    ///
    /// # Panics
    ///
    /// Panics when a tempdir cannot be created (test-only type).
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("fake media store tempdir"),
            objects: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(0),
            presign_base: None,
        }
    }

    /// Make `presign_get` return `Some("{base}/{id}")`.
    #[must_use]
    pub fn with_presign_base(mut self, base: &str) -> Self {
        self.presign_base = Some(Arc::from(base.trim_end_matches('/')));
        self
    }

    /// Number of stored objects.
    ///
    /// # Panics
    ///
    /// Panics on a poisoned lock (test-only type).
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn object_count(&self) -> usize {
        self.objects.lock().expect("objects lock").len()
    }

    /// Rewrite one object's bytes so the next materialize fails closed.
    ///
    /// # Panics
    ///
    /// Panics when the object is unknown (test-only type).
    #[allow(clippy::expect_used)]
    pub fn corrupt(&self, media: &MediaRef) {
        let objects = self.objects.lock().expect("objects lock");
        let stored = objects.get(media.id()).expect("known object");
        std::fs::write(&stored.path, b"corrupt").expect("corrupt write");
    }
}

impl Default for FakeMediaStore {
    fn default() -> Self {
        Self::new()
    }
}

impl finstack_ai_runtime::PortObject for FakeMediaStore {}

impl MediaStore for FakeMediaStore {
    fn put_file(
        &self,
        scope: ArtifactScope,
        local_path: PathBuf,
        metadata: MediaMetadata,
    ) -> PortFuture<Result<MediaRef, MediaError>> {
        let result = (|| {
            let scope_digest = scope.digest().map_err(|_| MediaError::InvalidMetadata {
                message: Arc::from("scope_invalid"),
            })?;
            let (digest, length) = hash_file_blob(&local_path)?;
            let id: Arc<str> = {
                let mut next = self.next_id.lock().map_err(|_| MediaError::Unavailable {
                    message: Arc::from("lock_poisoned"),
                })?;
                *next += 1;
                Arc::from(format!("fake/{next}"))
            };
            let stored_path = self.root.path().join(format!("obj-{}", digest.to_hex()));
            std::fs::copy(&local_path, &stored_path).map_err(|_| MediaError::Io {
                message: Arc::from("fake_copy_failed"),
            })?;
            let media = MediaRef::try_new(
                id.as_ref(),
                digest,
                length,
                metadata.media_type.as_ref(),
                scope_digest,
            )?;
            self.objects
                .lock()
                .map_err(|_| MediaError::Unavailable {
                    message: Arc::from("lock_poisoned"),
                })?
                .insert(
                    id,
                    StoredMedia {
                        path: stored_path,
                        media_type: metadata.media_type,
                    },
                );
            Ok(media)
        })();
        Box::pin(async move { result })
    }

    fn materialize(
        &self,
        scope: ArtifactScope,
        media: &MediaRef,
    ) -> PortFuture<Result<MaterializedMedia, MediaError>> {
        let media = media.clone();
        let result = (|| {
            let objects = self.objects.lock().map_err(|_| MediaError::Unavailable {
                message: Arc::from("lock_poisoned"),
            })?;
            let stored = objects.get(media.id()).ok_or(MediaError::NotFound)?;
            let _ = &stored.media_type;
            finstack_ai_runtime::verify_materialized(&scope, &media, &stored.path)?;
            Ok(MaterializedMedia::new(stored.path.clone()))
        })();
        Box::pin(async move { result })
    }

    fn presign_get(
        &self,
        scope: ArtifactScope,
        media: &MediaRef,
    ) -> PortFuture<Result<Option<Arc<str>>, MediaError>> {
        let result = (|| {
            let expected = scope.digest().map_err(|_| MediaError::InvalidMetadata {
                message: Arc::from("scope_invalid"),
            })?;
            if media.scope_digest() != expected {
                return Err(MediaError::ScopeMismatch {
                    expected,
                    actual: media.scope_digest(),
                });
            }
            Ok(self
                .presign_base
                .as_ref()
                .map(|base| Arc::from(format!("{base}/{}", media.id()))))
        })();
        Box::pin(async move { result })
    }

    fn delete(
        &self,
        _scope: ArtifactScope,
        media: &MediaRef,
    ) -> PortFuture<Result<(), MediaError>> {
        let result = (|| {
            let mut objects = self.objects.lock().map_err(|_| MediaError::Unavailable {
                message: Arc::from("lock_poisoned"),
            })?;
            let stored = objects.remove(media.id()).ok_or(MediaError::NotFound)?;
            let _ = std::fs::remove_file(stored.path);
            Ok(())
        })();
        Box::pin(async move { result })
    }
}
```

Note: `PortObject` may be a blanket-implemented marker (check `ports/mod.rs:16` — `pub trait PortObject: Send + Sync + 'static {}`). If a blanket impl exists, delete the explicit `impl finstack_ai_runtime::PortObject for FakeMediaStore {}` line; if not, keep it. Wire `pub(crate) mod media;` (or `mod media;` matching how `fakes/mod.rs` organizes submodules — if `fakes/mod.rs` is a single flat file, create the `media.rs` submodule and declare it) and re-export `FakeMediaStore` from `fakes/mod.rs` and the crate's `lib.rs` `pub use fakes::{...}` list.

- [ ] **Step 4: Run**

Run: `cargo test -p finstack-ai-test fake_store_round_trips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai-test
git commit -m "Add a tempdir-backed fake MediaStore for extension tests"
```

---

### Task 3: `finstack-ai-store-media-local`

**Files:**
- Modify: `Cargo.toml` (workspace root — members + workspace deps)
- Create: `extensions/stores/finstack-ai-store-media-local/Cargo.toml`
- Create: `extensions/stores/finstack-ai-store-media-local/README.md`
- Create: `extensions/stores/finstack-ai-store-media-local/src/lib.rs`

**Interfaces:**
- Consumes: Task 1's runtime exports.
- Produces: `pub struct LocalMediaStoreConfig { pub root: PathBuf, pub max_total_bytes: Option<u64> }`, `pub struct LocalMediaStore` with `pub fn try_new(LocalMediaStoreConfig) -> Result<Self, MediaError>` and `impl MediaStore`. Object ids are `"{scope-digest-16-hex-prefix}/{content-digest-hex}"`; `presign_get` always `Ok(None)`; `materialize` returns the stored path in place (no guard) after digest verification.

- [ ] **Step 1: Register the crate**

Root `Cargo.toml`: add `"extensions/stores/finstack-ai-store-media-local",` to `[workspace] members` (create the `extensions/stores/` grouping next to the toolset members) and to `[workspace.dependencies]`:

```toml
finstack-ai-store-media-local = { path = "extensions/stores/finstack-ai-store-media-local", version = "1.0.0" }
```

Crate manifest:

```toml
[package]
name = "finstack-ai-store-media-local"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Local-filesystem MediaStore backend for finstack-ai"
readme = "README.md"

[dependencies]
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }

[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt"] }

[lints]
workspace = true
```

README: one paragraph — local content-addressed `MediaStore` backend, explicit root, atomic write-then-rename, optional quota, T1 native leaf, `presign_get` is always `None`.

- [ ] **Step 2: Write the failing tests**

In `src/lib.rs`'s `#[cfg(test)] mod tests` (helper `scope()` identical to Task 1's test helper):

```rust
    #[tokio::test]
    async fn put_materialize_round_trip_is_content_addressed_and_idempotent() {
        let dir = tempfile::tempdir().expect("dir");
        let store = LocalMediaStore::try_new(LocalMediaStoreConfig {
            root: dir.path().join("media"),
            max_total_bytes: None,
        })
        .expect("store");
        let source = dir.path().join("clip.mp4");
        std::fs::write(&source, b"movie clip bytes").expect("write");
        let metadata = MediaMetadata {
            media_type: Arc::from("video/mp4"),
            name: None,
            attributes: Metadata::empty(),
        };
        let first = store
            .put_file(scope(), source.clone(), metadata.clone())
            .await
            .expect("first put");
        let second = store
            .put_file(scope(), source, metadata)
            .await
            .expect("second put");
        assert_eq!(first, second, "same scope + bytes must dedupe to one ref");
        let materialized = store.materialize(scope(), &first).await.expect("get");
        assert_eq!(
            std::fs::read(materialized.local_path()).expect("read"),
            b"movie clip bytes"
        );
        assert!(store.presign_get(scope(), &first).await.expect("presign").is_none());
    }

    #[tokio::test]
    async fn tampered_content_and_foreign_scope_fail_closed() {
        let dir = tempfile::tempdir().expect("dir");
        let store = LocalMediaStore::try_new(LocalMediaStoreConfig {
            root: dir.path().join("media"),
            max_total_bytes: None,
        })
        .expect("store");
        let source = dir.path().join("clip.mp4");
        std::fs::write(&source, b"movie clip bytes").expect("write");
        let media = store
            .put_file(
                scope(),
                source,
                MediaMetadata {
                    media_type: Arc::from("video/mp4"),
                    name: None,
                    attributes: Metadata::empty(),
                },
            )
            .await
            .expect("put");

        let foreign = ArtifactScope {
            tenant_scope: Arc::from("tenant-b"),
            ..scope()
        };
        let error = store
            .materialize(foreign, &media)
            .await
            .expect_err("foreign scope");
        assert_eq!(error.code(), MEDIA_SCOPE_MISMATCH);

        let stored = store.object_path(&media).expect("path");
        std::fs::write(stored, b"tampered content!").expect("tamper");
        let error = store.materialize(scope(), &media).await.expect_err("tampered");
        assert_eq!(error.code(), MEDIA_INTEGRITY_FAILURE);
    }

    #[tokio::test]
    async fn quota_rejects_before_writing() {
        let dir = tempfile::tempdir().expect("dir");
        let store = LocalMediaStore::try_new(LocalMediaStoreConfig {
            root: dir.path().join("media"),
            max_total_bytes: Some(10),
        })
        .expect("store");
        let source = dir.path().join("big.mp4");
        std::fs::write(&source, vec![0_u8; 32]).expect("write");
        let error = store
            .put_file(
                scope(),
                source,
                MediaMetadata {
                    media_type: Arc::from("video/mp4"),
                    name: None,
                    attributes: Metadata::empty(),
                },
            )
            .await
            .expect_err("quota");
        assert_eq!(error.code(), MEDIA_TOO_LARGE);
    }

    #[tokio::test]
    async fn delete_removes_the_object() {
        let dir = tempfile::tempdir().expect("dir");
        let store = LocalMediaStore::try_new(LocalMediaStoreConfig {
            root: dir.path().join("media"),
            max_total_bytes: None,
        })
        .expect("store");
        let source = dir.path().join("clip.mp4");
        std::fs::write(&source, b"bytes to delete").expect("write");
        let media = store
            .put_file(
                scope(),
                source,
                MediaMetadata {
                    media_type: Arc::from("video/mp4"),
                    name: None,
                    attributes: Metadata::empty(),
                },
            )
            .await
            .expect("put");
        store.delete(scope(), &media).await.expect("delete");
        let error = store.materialize(scope(), &media).await.expect_err("gone");
        assert_eq!(error.code(), MEDIA_NOT_FOUND);
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p finstack-ai-store-media-local`
Expected: FAIL to compile — types not defined.

- [ ] **Step 4: Implement**

`src/lib.rs` (after the standard lint header block and module doc `//! Local-filesystem MediaStore backend: explicit root, content-addressed layout, atomic ingest.`):

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use finstack_ai_runtime::{
    ArtifactScope, MaterializedMedia, MediaError, MediaMetadata, MediaRef, MediaStore, PortFuture,
    hash_file_blob, verify_materialized,
};

/// Explicit local store configuration. Never populated from the environment.
#[derive(Debug, Clone)]
pub struct LocalMediaStoreConfig {
    /// Root directory. Created if absent.
    pub root: PathBuf,
    /// Optional total-bytes quota across all stored objects.
    pub max_total_bytes: Option<u64>,
}

/// Content-addressed local-filesystem [`MediaStore`].
#[derive(Debug)]
pub struct LocalMediaStore {
    root: PathBuf,
    max_total_bytes: Option<u64>,
}

impl LocalMediaStore {
    /// Create the root directory and validate the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::Io`] when the root cannot be created.
    pub fn try_new(config: LocalMediaStoreConfig) -> Result<Self, MediaError> {
        std::fs::create_dir_all(&config.root).map_err(|_| MediaError::Io {
            message: Arc::from("local_media_root_create_failed"),
        })?;
        Ok(Self {
            root: config.root,
            max_total_bytes: config.max_total_bytes,
        })
    }

    /// Stored path for one reference (`{root}/{id}` with the id's one slash).
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidMetadata`] for a malformed id.
    pub fn object_path(&self, media: &MediaRef) -> Result<PathBuf, MediaError> {
        let id = media.id();
        let Some((prefix, digest_hex)) = id.split_once('/') else {
            return Err(MediaError::InvalidMetadata {
                message: Arc::from("local_media_id_invalid"),
            });
        };
        if prefix.len() != 16
            || !prefix.bytes().all(|b| b.is_ascii_hexdigit())
            || digest_hex.len() != 64
            || !digest_hex.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(MediaError::InvalidMetadata {
                message: Arc::from("local_media_id_invalid"),
            });
        }
        Ok(self.root.join(prefix).join(digest_hex))
    }

    fn total_bytes(&self) -> u64 {
        fn walk(dir: &Path, total: &mut u64) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, total);
                } else if let Ok(meta) = entry.metadata() {
                    *total = total.saturating_add(meta.len());
                }
            }
        }
        let mut total = 0;
        walk(&self.root, &mut total);
        total
    }
}
```

`impl MediaStore for LocalMediaStore` (same synchronous-body-wrapped-in-`Box::pin(async move { result })` shape as the Task 2 fake):

- `put_file`: compute `scope.digest()` (map error to `InvalidMetadata`); `hash_file_blob(&local_path)`; if `max_total_bytes` is `Some(max)` and `self.total_bytes().saturating_add(length) > max`, return `MediaError::TooLarge { len: length, max }` **before** writing; id is `format!("{}/{}", &scope_digest.to_hex()[..16], digest.to_hex())` (use `.get(..16)` + `ok_or` instead of slicing, to satisfy `clippy::indexing_slicing`); target path via the same join as `object_path`; `create_dir_all` the parent; if the target already exists, skip the copy (content-addressed dedupe); else copy to `target.with_extension("tmp-{digest-first-8-hex}")` then `std::fs::rename` into place (atomic on one filesystem); build `MediaRef::try_new(id, digest, length, metadata.media_type.as_ref(), scope_digest)`.
- `materialize`: `object_path(&media)`; missing file → `MediaError::NotFound`; `verify_materialized(&scope, &media, &path)?` (covers scope and digest); return `MaterializedMedia::new(path)`.
- `presign_get`: verify the scope digest matches (same check as the fake), then `Ok(None)`.
- `delete`: `object_path`; verify scope digest matches `media.scope_digest()`; `std::fs::remove_file` (missing → `NotFound`).

Add the crate to nothing else — no SDK wiring until Task 16.

- [ ] **Step 5: Run**

Run: `cargo test -p finstack-ai-store-media-local`
Expected: PASS (all four tests).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/stores/finstack-ai-store-media-local
git commit -m "Add the content-addressed local MediaStore backend"
```

---

## Phase B — OpenRouter async video tools and store-backed images

Both tasks edit `extensions/toolsets/finstack-ai-tools-openrouter-media/src/lib.rs` as created by the OpenRouter provider plan (Tasks 11–13). That file is modeled on the E2B toolset: one `lib.rs` with config, error codes, `try_new` building `ToolSpec`s, a `Toolset::call` dispatch, and helpers `read_bounded_json`, `send_json`, `deadline_elapsed`, `wait_deadline`, `timeout_error`, `tool_error`, `validate_endpoint`. Reuse those helpers; do not duplicate them.

### Task 4: `openrouter_submit_video` (replaces `openrouter_generate_video`)

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/Cargo.toml` (add `base64 = { workspace = true }` if the crate does not already carry it; add `tempfile = { workspace = true }` to `[dev-dependencies]`)
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/lib.rs`

**Interfaces:**
- Consumes: `finstack_ai_runtime::{ArtifactScope, MediaRef, MediaStore, MaterializedMedia}` (Task 1), `finstack_ai_test::FakeMediaStore` (Task 2, dev-dependency), the crate's existing helpers and error codes.
- Produces: config fields `pub media_store: Option<Arc<dyn MediaStore>>` and `pub media_sensitivity: Sensitivity` on `OpenRouterMediaConfig`; tool `openrouter_submit_video` (id `finstack.tools.openrouter_submit_video`); crate-internal `fn artifact_scope(sensitivity: Sensitivity, ctx: &ToolCallContext) -> Result<ArtifactScope, ToolError>` and `async fn resolve_image_input(store, scope, input: &ImageInput) -> Result<String, ToolError>` reused by Task 5; `pub const OPENROUTER_MEDIA_STORE_REQUIRED: &str = "openrouter_media_store_required";`; input type `ImageInput { url: Option<String>, media_ref: Option<MediaRef> }`.

- [ ] **Step 0: Re-verify the live wire shape**

Fetch `https://openrouter.ai/docs/guides/overview/multimodal/video-generation`. Confirm: `POST /api/v1/videos` fields `model`, `prompt`, `duration`, `resolution`, `aspect_ratio`, `size`, `seed`, `generate_audio`, `frame_images` (items `{"type":"image_url","image_url":{"url":...},"frame_type":"first_frame"|"last_frame"}`), `input_references` (same, no `frame_type`); 202 response `{id, polling_url, status}`. If a field moved, adjust the private wire body construction only — the tool input schema below is fixed.

- [ ] **Step 1: Extend the config**

Add to `OpenRouterMediaConfig` (and its `Debug` impl, which keeps redacting `api_key` — `media_store` renders as `self.media_store.is_some()` under the field name `"media_store_configured"`):

```rust
    /// Optional large-object media store. Absent keeps inline-bounded results.
    pub media_store: Option<Arc<dyn MediaStore>>,
    /// Sensitivity classification for media staged from tool calls.
    pub media_sensitivity: Sensitivity,
```

`OpenRouterMediaToolset` gains matching fields, populated in `try_new`. Every existing construction site in tests gains `media_store: None, media_sensitivity: Sensitivity::Confidential,`.

Add the new error code next to the existing ones:

```rust
/// Stable missing-media-store code.
pub const OPENROUTER_MEDIA_STORE_REQUIRED: &str = "openrouter_media_store_required";
```

- [ ] **Step 2: Remove the one-shot video tool and add the submit tool spec**

Delete the `openrouter_generate_video` constants, `ToolSpec`, `VideoArguments`/`VideoResponse` DTOs, and its dispatch arm. Add:

```rust
const VIDEO_SUBMIT_TOOL_ID: &str = "finstack.tools.openrouter_submit_video";
const VIDEO_SUBMIT_TOOL_NAME: &str = "openrouter_submit_video";
/// Ceiling on a frame image inlined as a data URI (pre-base64 bytes).
const MAX_INLINE_IMAGE_BYTES: u64 = 8 * 1_048_576;
```

Tool spec (same metadata as the other paid tools: `Sequential`, `NonIdempotentWrite`, `AtMostOnce`, approval `Policy` with reason `"paid OpenRouter media generation"`, `max_result_bytes: 262_144`, `deferral: Never`). Description: `"Submit one asynchronous OpenRouter video generation job. Returns a job id to poll with openrouter_get_video_job."` Schemas:

```rust
// input
br#"{"additionalProperties":false,"$defs":{"image_input":{"additionalProperties":false,"properties":{"media_ref":{"additionalProperties":false,"properties":{"digest":{"type":"string"},"id":{"type":"string"},"length":{"type":"integer"},"media_type":{"type":"string"},"scope_digest":{"type":"string"}},"required":["id","digest","length","media_type","scope_digest"],"type":"object"},"url":{"minLength":1,"type":"string"}},"type":"object"}},"properties":{"aspect_ratio":{"type":"string"},"duration_s":{"minimum":1,"type":"integer"},"first_frame":{"$ref":"#/$defs/image_input"},"generate_audio":{"type":"boolean"},"last_frame":{"$ref":"#/$defs/image_input"},"model":{"minLength":1,"type":"string"},"prompt":{"minLength":1,"type":"string"},"reference_images":{"items":{"$ref":"#/$defs/image_input"},"maxItems":4,"type":"array"},"resolution":{"type":"string"},"seed":{"type":"integer"},"size":{"type":"string"}},"required":["model","prompt"],"type":"object"}"#
// output
br#"{"additionalProperties":false,"properties":{"job_id":{"minLength":1,"type":"string"},"polling_url":{"type":"string"},"status":{"type":"string"}},"required":["job_id","status"],"type":"object"}"#
```

If `ToolSpec::validate()` rejects `$defs`/`$ref` schemas, inline the `image_input` object at all four use sites instead — the wire behavior is identical.

- [ ] **Step 3: Argument DTOs and image-input resolution**

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageInput {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    media_ref: Option<MediaRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitVideoArguments {
    model: String,
    prompt: String,
    #[serde(default)]
    duration_s: Option<u32>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    aspect_ratio: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    seed: Option<i64>,
    #[serde(default)]
    generate_audio: Option<bool>,
    #[serde(default)]
    first_frame: Option<ImageInput>,
    #[serde(default)]
    last_frame: Option<ImageInput>,
    #[serde(default)]
    reference_images: Option<Vec<ImageInput>>,
}

#[derive(Deserialize)]
struct SubmitVideoResponse {
    id: String,
    #[serde(default)]
    polling_url: Option<String>,
    #[serde(default)]
    status: Option<String>,
}
```

Scope construction from the call context (this is why no scope lives in config — it follows the calling run):

```rust
fn artifact_scope(
    sensitivity: Sensitivity,
    ctx: &ToolCallContext,
) -> Result<ArtifactScope, ToolError> {
    let locator = &ctx.run.locator;
    Ok(ArtifactScope {
        tenant_scope: Arc::from(locator.tenant_scope()),
        session_id: locator.session_id(),
        run_id: Some(locator.run_id()),
        sensitivity,
    })
}
```

(Use the actual `OperationLocator` accessor names — open `crates/finstack-ai-kernel/src/primitives/` and check; if `run_id`/`session_id` return references, clone/copy accordingly.)

Image-input resolution — exactly one of `url`/`media_ref` must be set; `media_ref` prefers a presigned URL and falls back to a bounded data URI:

```rust
async fn resolve_image_input(
    store: Option<&Arc<dyn MediaStore>>,
    scope: &ArtifactScope,
    input: &ImageInput,
) -> Result<String, ToolError> {
    match (&input.url, &input.media_ref) {
        (Some(url), None) => Ok(url.clone()),
        (None, Some(media)) => {
            let store = store.ok_or_else(|| {
                tool_error(
                    OPENROUTER_MEDIA_STORE_REQUIRED,
                    ErrorCategory::Configuration,
                    "media_ref inputs require a configured media store",
                )
            })?;
            if let Some(url) = store
                .presign_get(scope.clone(), media)
                .await
                .map_err(media_tool_error)?
            {
                return Ok(url.to_string());
            }
            if media.length() > MAX_INLINE_IMAGE_BYTES {
                return Err(tool_error(
                    OPENROUTER_MEDIA_LIMIT_EXCEEDED,
                    ErrorCategory::Limit,
                    "frame image exceeds the inline data-URI ceiling",
                ));
            }
            let materialized = store
                .materialize(scope.clone(), media)
                .await
                .map_err(media_tool_error)?;
            let bytes = std::fs::read(materialized.local_path()).map_err(|_| {
                tool_error(
                    OPENROUTER_MEDIA_TRANSPORT_FAILED,
                    ErrorCategory::Tool,
                    "materialized frame image could not be read",
                )
            })?;
            use base64::Engine as _;
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            Ok(format!("data:{};base64,{encoded}", media.media_type()))
        }
        _ => Err(tool_error(
            OPENROUTER_MEDIA_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "image input must set exactly one of url or media_ref",
        )),
    }
}

fn media_tool_error(error: finstack_ai_runtime::MediaError) -> ToolError {
    let code: &'static str = match error {
        finstack_ai_runtime::MediaError::TooLarge { .. } => OPENROUTER_MEDIA_LIMIT_EXCEEDED,
        _ => OPENROUTER_MEDIA_TRANSPORT_FAILED,
    };
    tool_error(code, ErrorCategory::Tool, "media store operation failed")
}
```

- [ ] **Step 4: Dispatch arm**

In `Toolset::call`, add the `VIDEO_SUBMIT_TOOL_NAME` arm: parse `SubmitVideoArguments` (parse failure → `OPENROUTER_MEDIA_INVALID_ARGUMENTS`); build the wire body:

```rust
let scope = artifact_scope(sensitivity, &ctx)?;
let mut frame_images = Vec::new();
for (input, frame_type) in [
    (arguments.first_frame.as_ref(), "first_frame"),
    (arguments.last_frame.as_ref(), "last_frame"),
] {
    if let Some(input) = input {
        let url = resolve_image_input(media_store.as_ref(), &scope, input).await?;
        frame_images.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": url },
            "frame_type": frame_type,
        }));
    }
}
let mut input_references = Vec::new();
for input in arguments.reference_images.iter().flatten() {
    let url = resolve_image_input(media_store.as_ref(), &scope, input).await?;
    input_references.push(serde_json::json!({
        "type": "image_url",
        "image_url": { "url": url },
    }));
}
let mut body = serde_json::json!({ "model": arguments.model, "prompt": arguments.prompt });
let object = body.as_object_mut().unwrap_or_else(|| unreachable!());
if let Some(v) = arguments.duration_s { object.insert("duration".into(), v.into()); }
if let Some(v) = &arguments.resolution { object.insert("resolution".into(), v.as_str().into()); }
if let Some(v) = &arguments.aspect_ratio { object.insert("aspect_ratio".into(), v.as_str().into()); }
if let Some(v) = &arguments.size { object.insert("size".into(), v.as_str().into()); }
if let Some(v) = arguments.seed { object.insert("seed".into(), v.into()); }
if let Some(v) = arguments.generate_audio { object.insert("generate_audio".into(), v.into()); }
if !frame_images.is_empty() { object.insert("frame_images".into(), frame_images.into()); }
if !input_references.is_empty() { object.insert("input_references".into(), input_references.into()); }
```

(The lint header denies `unreachable!` outside tests — replace the `unwrap_or_else(|| unreachable!())` with a `let Some(object) = body.as_object_mut() else { return Err(tool_error(OPENROUTER_MEDIA_INVALID_ARGUMENTS, ErrorCategory::Internal, "body must be an object")) };` guard.)

POST via the existing `send_json` helper to `{endpoint}/api/v1/videos`, accepting 200 and 202 as success (adjust `send_json`'s status check to `status.is_success()` if it is stricter). Deserialize `SubmitVideoResponse`; empty `id` → `OPENROUTER_MEDIA_TRANSPORT_FAILED`. Result JSON: `{"job_id": id, "status": status.unwrap_or_else(|| "pending".into()), "polling_url"?}` — same serialize/`RawJson::parse`/`ToolStreamItem::Completed` tail as the other arms.

- [ ] **Step 5: Tests**

Using the crate's existing scripted-fixture helpers (`serve_scripted`-style `respond`, `tool_context()`, `ValidatedToolCall` builder — copied from the E2B test module by the OpenRouter plan):

1. `submit_video_maps_optional_fields_and_frame_images` — construct with `media_store: Some(Arc::new(FakeMediaStore::new().with_presign_base("https://cdn.test")))`; put a small PNG file into the fake store first (via `put_file` with the same scope the `tool_context()` locator produces — tenant `tenant-a`, session `[1;16]`, run `[3;16]`, `Sensitivity::Confidential`); call the tool with `model`, `prompt`, `duration_s: 8`, `resolution: "1080p"`, `seed: 42`, `first_frame: {media_ref}`, `last_frame: {url: "https://example.test/last.png"}`; scripted 202 `{"id":"vid-1","polling_url":"https://openrouter.test/api/v1/videos/vid-1","status":"pending"}`. Assert the captured request line contains `post /api/v1/videos`; the captured body JSON has `duration == 8`, `resolution == "1080p"`, `seed == 42`, `frame_images[0].frame_type == "first_frame"`, `frame_images[0].image_url.url` starting `https://cdn.test/`, `frame_images[1].frame_type == "last_frame"`; and the result JSON is `{"job_id":"vid-1","status":"pending","polling_url":...}`.
2. `submit_video_inlines_local_media_as_data_uri` — same but `FakeMediaStore::new()` without presign base; assert `frame_images[0].image_url.url` starts with `"data:image/png;base64,"`.
3. `submit_video_without_store_rejects_media_refs` — `media_store: None`, `first_frame: {media_ref: ...}` (build a `MediaRef` by hand); expect error code `openrouter_media_store_required`; no HTTP reaches the fixture.
4. `submit_video_rejects_ambiguous_image_input` — `first_frame: {"url": "...", "media_ref": {...}}` → `openrouter_media_invalid_arguments`.
5. `oversized_frame_image_fails_closed` — fake-store object with `length() > MAX_INLINE_IMAGE_BYTES` (write an over-limit sparse file, or construct the `MediaRef` by hand against a store with no presign) → `openrouter_media_limit_exceeded`.

- [ ] **Step 6: Run**

Run: `cargo test -p finstack-ai-tools-openrouter-media`
Expected: PASS — new tests green, and every pre-existing test still green after the `media_store`/`media_sensitivity` construction-site updates; no test references `openrouter_generate_video` any more.

- [ ] **Step 7: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-openrouter-media
git commit -m "Replace one-shot video generation with an async submit tool carrying frame images"
```

---

### Task 5: `openrouter_get_video_job`, `openrouter_download_video`, store-backed images

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/lib.rs`

**Interfaces:**
- Consumes: Task 4's `artifact_scope`, `media_tool_error`, config fields; Task 1's `hash_file_blob` (indirectly via `put_file`).
- Produces: tools `openrouter_get_video_job` and `openrouter_download_video`; `openrouter_generate_image` result gains store-backed `{media_ref}` output when a store is configured. Result shapes relied on by the Task 11 driver: get_video_job → `{"job_id","status","unsigned_urls"?,"cost"?}`; download_video → `{"media_ref","media_type","length"}`; generate_image (store-backed) → `{"media_ref","media_type","url"?}`.

- [ ] **Step 1: Tool specs**

```rust
const VIDEO_JOB_TOOL_ID: &str = "finstack.tools.openrouter_get_video_job";
const VIDEO_JOB_TOOL_NAME: &str = "openrouter_get_video_job";
const VIDEO_DOWNLOAD_TOOL_ID: &str = "finstack.tools.openrouter_download_video";
const VIDEO_DOWNLOAD_TOOL_NAME: &str = "openrouter_download_video";
/// Ceiling on one downloaded video (bytes).
const MAX_VIDEO_BYTES: u64 = 2 * 1_024 * 1_024 * 1_024;
```

`openrouter_get_video_job`: description `"Poll one OpenRouter video generation job."`; `execution: Sequential`, `side_effect: SideEffectClass::Read` (use the crate's existing read-classification variant — check `SideEffectClass`'s variants and pick the idempotent-read one), `retry_safety: RetrySafety::Idempotent` (same check), approval `ApprovalRequirement::Never` with reason `None`. Schemas:

```rust
// input
br#"{"additionalProperties":false,"properties":{"job_id":{"minLength":1,"type":"string"}},"required":["job_id"],"type":"object"}"#
// output
br#"{"additionalProperties":false,"properties":{"cost":{"type":"number"},"job_id":{"type":"string"},"status":{"type":"string"},"unsigned_urls":{"items":{"type":"string"},"type":"array"}},"required":["job_id","status"],"type":"object"}"#
```

`openrouter_download_video`: description `"Download one completed OpenRouter video into the configured media store."`; paid-tool metadata like submit (`NonIdempotentWrite`, `AtMostOnce`, approval `Policy`, reason `"paid OpenRouter media generation"`). Schemas:

```rust
// input
br#"{"additionalProperties":false,"properties":{"job_id":{"minLength":1,"type":"string"}},"required":["job_id"],"type":"object"}"#
// output
br#"{"additionalProperties":false,"properties":{"length":{"type":"integer"},"media_ref":{"additionalProperties":false,"properties":{"digest":{"type":"string"},"id":{"type":"string"},"length":{"type":"integer"},"media_type":{"type":"string"},"scope_digest":{"type":"string"}},"required":["id","digest","length","media_type","scope_digest"],"type":"object"},"media_type":{"type":"string"}},"required":["media_ref","media_type","length"],"type":"object"}"#
```

Job-id path safety (both tools): reject a `job_id` containing anything other than ASCII alphanumerics, `-`, `_` with `OPENROUTER_MEDIA_INVALID_ARGUMENTS` before building the URL.

- [ ] **Step 2: Poll dispatch arm**

```rust
#[derive(Deserialize)]
struct VideoJobResponse {
    id: String,
    status: String,
    #[serde(default)]
    unsigned_urls: Vec<String>,
    #[serde(default)]
    usage: Option<VideoJobUsage>,
}

#[derive(Deserialize)]
struct VideoJobUsage {
    #[serde(default)]
    cost: Option<f64>,
}
```

GET `{endpoint}/api/v1/videos/{job_id}` — add a `send_get` helper next to `send_json` (same headers, cancellation select, bounded read; no body). Map to result JSON `{"job_id": id, "status": status, "unsigned_urls"?: ..., "cost"?: usage.cost}` (omit empty/absent fields). A `status == "failed"` poll is still a **successful tool result** (`is_error: false`) — the caller decides policy.

- [ ] **Step 3: Download dispatch arm**

Requires the store: `media_store` absent → `OPENROUTER_MEDIA_STORE_REQUIRED` (Configuration). GET `{endpoint}/api/v1/videos/{job_id}/content` with the auth headers; **stream** the body to a `tempfile::NamedTempFile` (created with `tempfile::Builder::new().prefix("or-video-").tempfile()` in the OS temp dir), enforcing `MAX_VIDEO_BYTES` incrementally (over-limit → `OPENROUTER_MEDIA_LIMIT_EXCEEDED`); honor cancellation between chunks (`ctx.run.cancellation.is_cancelled()` → `timeout_error()`). Media type from the response `Content-Type` header, defaulting to `"video/mp4"`. Then:

```rust
let scope = artifact_scope(sensitivity, &ctx)?;
let media = store
    .put_file(
        scope,
        temp.path().to_path_buf(),
        MediaMetadata {
            media_type: Arc::from(media_type.as_str()),
            name: None,
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(media_tool_error)?;
```

Result JSON: `{"media_ref": media, "media_type": ..., "length": media.length()}` (`MediaRef` is `Serialize`; embed with `serde_json::to_value(&media)`).

- [ ] **Step 4: Store-backed image results**

In the existing `openrouter_generate_image` arm: when the store is configured and the response carries `b64_json`, decode (decode failure → `OPENROUTER_MEDIA_TRANSPORT_FAILED`), write to a `NamedTempFile`, `put_file` with media type `"image/png"` unless the API reported one, and return `{"media_ref", "media_type", "url"?}`. When the store is absent, keep the existing inline behavior byte-for-byte. Update the image tool's output schema to add the optional `media_ref` object property (same inline `media_ref` schema object as above).

- [ ] **Step 5: Tests**

1. `get_video_job_maps_status_and_urls` — scripted 200 `{"id":"vid-1","status":"completed","unsigned_urls":["https://openrouter.test/api/v1/videos/vid-1/content?index=0"],"usage":{"cost":0.25,"is_byok":false}}`; assert request line contains `get /api/v1/videos/vid-1`, result has `status == "completed"`, one URL, `cost == 0.25`.
2. `get_video_job_reports_failed_without_erroring` — scripted `{"id":"vid-1","status":"failed"}`; assert `is_error == false` and `status == "failed"`.
3. `download_video_streams_into_the_store` — store-backed toolset; scripted 200 response with `Content-Type: video/mp4` and a 1 KiB binary body; assert result `media_type == "video/mp4"`, `length == 1024`, and `media_ref.digest` equals `Digest::blob_content(body).to_hex()`; then `materialize` through the same fake store and assert the bytes round-trip.
4. `download_video_without_store_fails_closed` — `media_store: None` → `openrouter_media_store_required`, no HTTP reaches the fixture.
5. `download_video_bounds_the_body` — patch the test constant path: scripted body larger than a test-shrunk limit is impractical at 2 GiB, so instead assert the incremental guard: add `#[cfg(test)] fn max_video_bytes_for_tests() -> u64 { 4_096 }` and have the arm read the limit through a small `fn video_byte_limit() -> u64` that returns the test value under `cfg(test)`; scripted 8 KiB body → `openrouter_media_limit_exceeded`.
6. `generate_image_stores_bytes_when_a_store_is_configured` — scripted image response `{"data":[{"b64_json":"<base64 of 16 bytes>"}]}`; assert result has `media_ref` and no `b64_json`.
7. `job_id_with_path_characters_is_rejected` — `{"job_id":"../etc"}` → `openrouter_media_invalid_arguments`, no HTTP.

- [ ] **Step 6: Run**

Run: `cargo test -p finstack-ai-tools-openrouter-media`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-openrouter-media
git commit -m "Add video job polling and store-backed download and image tools"
```

---

## Phase C — `finstack-ai-tools-video-compose`

### Task 6: Crate scaffolding, composition-spec types, validation

**Files:**
- Modify: `Cargo.toml` (workspace root — members + workspace deps entry `finstack-ai-tools-video-compose = { path = "extensions/toolsets/finstack-ai-tools-video-compose", version = "1.0.0" }`)
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/Cargo.toml`
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/README.md`
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/lib.rs`
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/spec.rs`

**Interfaces:**
- Consumes: Task 1 runtime media exports.
- Produces (spec.rs, all `pub(crate)` except where noted): `pub struct CompositionSpec { pub version: u32, pub clips: Vec<ClipSpec>, pub transitions: Option<Vec<TransitionSpec>>, pub audio: Option<AudioSpec>, pub subtitles: Option<SubtitlesSpec>, pub output: OutputSpec }`, `pub struct SubtitlesSpec { pub media_ref: MediaRef, pub mode: SubtitleMode, pub style: Option<SubtitleStyle> }`, `pub enum SubtitleMode { BurnIn, Mux }`, `pub struct SubtitleStyle { pub font_size: Option<u32>, pub margin_v: Option<u32> }`, `pub struct ClipSpec { pub media_ref: MediaRef, pub trim: Option<TrimSpec> }`, `pub struct TrimSpec { pub start_s: f64, pub end_s: f64 }`, `pub enum TransitionKind { Cut, Crossfade, FadeToBlack }`, `pub struct TransitionSpec { pub kind: TransitionKind, pub duration_s: Option<f64> }`, `pub struct AudioSpec { pub media_ref: MediaRef, pub mode: AudioMode, pub gain_db: Option<f64> }`, `pub enum AudioMode { Replace, Mix }`, `pub struct OutputSpec { pub container: Container, pub resolution: Option<String>, pub fps: Option<u32> }`, `pub enum Container { Mp4, Webm }`, and `pub fn validate_spec(&CompositionSpec) -> Result<(), &'static str>` (message is the bounded diagnostic). Serde: `deny_unknown_fields` everywhere, enums `#[serde(rename_all = "snake_case")]`, `TransitionSpec.kind` serialized as field name `type` via `#[serde(rename = "type")]`.
- Produces (lib.rs): the seven `VIDEO_COMPOSE_*` error-code constants (Global Constraints), `pub struct VideoComposeConfig { pub ffmpeg_path: PathBuf, pub ffprobe_path: PathBuf, pub media_store: Arc<dyn MediaStore>, pub media_sensitivity: Sensitivity, pub scratch_dir: PathBuf, pub render_timeout: Duration }`, `pub enum VideoComposeError { ConfigInvalid { reason: &'static str } }`, `pub struct VideoComposeToolset` with `try_new(VideoComposeConfig) -> Result<Self, VideoComposeError>` publishing two `ToolSpec`s (`compose_video`, `probe_media`). `Toolset::call` arrives in Task 8.

- [ ] **Step 1: Scaffolding**

Manifest: copy the E2B manifest dependency set, name `finstack-ai-tools-video-compose`, description `"Declarative ffmpeg composition toolset for finstack-ai"`; drop `reqwest` (no HTTP); ensure `tokio` carries `features = ["process", "io-util", "time"]` in `[dependencies]`; dev-dependencies add `finstack-ai-test = { workspace = true }` and `tempfile = { workspace = true }`. README: declarative composition spec → host-supplied ffmpeg; agents can never pass flags or paths; requires a `MediaStore`; v1 transitions cut/crossfade/fade_to_black. Register in the workspace root. Lint header block verbatim.

- [ ] **Step 2: Failing spec tests**

In `src/spec.rs`'s test module:

```rust
    fn media_ref() -> MediaRef {
        let digest = Digest::raw_json(b"{}");
        MediaRef::try_new("fake/1", digest, 10, "video/mp4", digest).expect("media ref")
    }

    fn minimal(clips: usize, transitions: Option<Vec<TransitionSpec>>) -> CompositionSpec {
        CompositionSpec {
            version: 1,
            clips: (0..clips)
                .map(|_| ClipSpec { media_ref: media_ref(), trim: None })
                .collect(),
            transitions,
            audio: None,
            subtitles: None,
            output: OutputSpec { container: Container::Mp4, resolution: None, fps: None },
        }
    }

    #[test]
    fn spec_json_round_trips_with_snake_case_enums() {
        let json = br#"{"version":1,"clips":[{"media_ref":{"id":"fake/1","digest":"<64 zeros>","length":10,"media_type":"video/mp4","scope_digest":"<64 zeros>"}}],"transitions":[],"output":{"container":"mp4"}}"#;
        // Replace <64 zeros> with a literal 64-char hex string when writing the test.
        let spec: CompositionSpec = serde_json::from_slice(json).expect("parse");
        assert_eq!(spec.version, 1);
        assert!(serde_json::from_slice::<CompositionSpec>(
            br#"{"version":1,"clips":[],"output":{"container":"mp4"},"extra":1}"#
        )
        .is_err());
    }

    #[test]
    fn validation_enforces_counts_bounds_and_version() {
        assert!(validate_spec(&minimal(2, None)).is_ok());
        assert!(validate_spec(&minimal(0, None)).is_err(), "needs >= 1 clip");
        let mut wrong_version = minimal(1, None);
        wrong_version.version = 2;
        assert!(validate_spec(&wrong_version).is_err());
        let mismatched = minimal(
            3,
            Some(vec![TransitionSpec { kind: TransitionKind::Cut, duration_s: None }]),
        );
        assert!(validate_spec(&mismatched).is_err(), "transitions must be clips-1");
        let ok = minimal(
            2,
            Some(vec![TransitionSpec {
                kind: TransitionKind::Crossfade,
                duration_s: Some(0.5),
            }]),
        );
        assert!(validate_spec(&ok).is_ok());
        let bad_duration = minimal(
            2,
            Some(vec![TransitionSpec {
                kind: TransitionKind::Crossfade,
                duration_s: Some(30.0),
            }]),
        );
        assert!(validate_spec(&bad_duration).is_err(), "transition <= 5s");
        let mut bad_trim = minimal(1, None);
        bad_trim.clips[0].trim = Some(TrimSpec { start_s: 5.0, end_s: 2.0 });
        assert!(validate_spec(&bad_trim).is_err());
        let mut bad_gain = minimal(1, None);
        bad_gain.audio = Some(AudioSpec {
            media_ref: media_ref(),
            mode: AudioMode::Mix,
            gain_db: Some(-100.0),
        });
        assert!(validate_spec(&bad_gain).is_err(), "gain in [-60, 12]");
        let mut bad_res = minimal(1, None);
        bad_res.output.resolution = Some("bogus".into());
        assert!(validate_spec(&bad_res).is_err(), "resolution is WIDTHxHEIGHT");
        let mut bad_fps = minimal(1, None);
        bad_fps.output.fps = Some(500);
        assert!(validate_spec(&bad_fps).is_err(), "fps in [1, 120]");
        let mut bad_style = minimal(1, None);
        bad_style.subtitles = Some(SubtitlesSpec {
            media_ref: media_ref(),
            mode: SubtitleMode::BurnIn,
            style: Some(SubtitleStyle { font_size: Some(4), margin_v: None }),
        });
        assert!(validate_spec(&bad_style).is_err(), "font_size in [8, 96]");
        let mut bad_mux = minimal(1, None);
        bad_mux.output.container = Container::Webm;
        bad_mux.subtitles = Some(SubtitlesSpec {
            media_ref: media_ref(),
            mode: SubtitleMode::Mux,
            style: None,
        });
        assert!(validate_spec(&bad_mux).is_err(), "mux is mp4-only");
    }
```

- [ ] **Step 3: Run to verify failure, then implement `spec.rs`**

Run: `cargo test -p finstack-ai-tools-video-compose spec` — FAIL to compile.

Implement the types exactly as the Interfaces block defines, plus:

```rust
/// Validate one version-1 composition spec.
pub(crate) fn validate_spec(spec: &CompositionSpec) -> Result<(), &'static str> {
    if spec.version != 1 {
        return Err("composition spec version must be 1");
    }
    if spec.clips.is_empty() || spec.clips.len() > 64 {
        return Err("composition needs between 1 and 64 clips");
    }
    if let Some(transitions) = &spec.transitions {
        if !transitions.is_empty() && transitions.len() != spec.clips.len().saturating_sub(1) {
            return Err("transition count must be clip count minus one");
        }
        for transition in transitions {
            let duration = transition.duration_s.unwrap_or(0.5);
            let needs_duration = !matches!(transition.kind, TransitionKind::Cut);
            if needs_duration && !(0.05..=5.0).contains(&duration) {
                return Err("transition duration must be between 0.05 and 5 seconds");
            }
        }
    }
    for clip in &spec.clips {
        if let Some(trim) = &clip.trim {
            if !trim.start_s.is_finite()
                || !trim.end_s.is_finite()
                || trim.start_s < 0.0
                || trim.end_s <= trim.start_s
            {
                return Err("clip trim must satisfy 0 <= start < end");
            }
        }
    }
    if let Some(audio) = &spec.audio {
        if let Some(gain) = audio.gain_db {
            if !gain.is_finite() || !(-60.0..=12.0).contains(&gain) {
                return Err("audio gain must be between -60 and 12 dB");
            }
        }
    }
    if let Some(subtitles) = &spec.subtitles {
        if matches!(subtitles.mode, SubtitleMode::Mux) && !matches!(spec.output.container, Container::Mp4) {
            return Err("muxed subtitles require the mp4 container");
        }
        if let Some(style) = &subtitles.style {
            if style.font_size.is_some_and(|v| !(8..=96).contains(&v))
                || style.margin_v.is_some_and(|v| v > 400)
            {
                return Err("subtitle style values are out of bounds");
            }
        }
    }
    if let Some(resolution) = &spec.output.resolution {
        let valid = resolution.split_once('x').is_some_and(|(w, h)| {
            w.parse::<u32>().is_ok_and(|w| (16..=7_680).contains(&w))
                && h.parse::<u32>().is_ok_and(|h| (16..=4_320).contains(&h))
        });
        if !valid {
            return Err("output resolution must be WIDTHxHEIGHT within 16..7680x4320");
        }
    }
    if let Some(fps) = spec.output.fps {
        if !(1..=120).contains(&fps) {
            return Err("output fps must be between 1 and 120");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: `lib.rs` config, error codes, tool catalog**

Module doc: `//! T1 declarative ffmpeg composition Toolset. Agents submit a bounded spec; the toolset owns every ffmpeg argument.` Then the seven code constants, `VideoComposeConfig` (plain `Debug` derive — nothing secret), `VideoComposeError::ConfigInvalid { reason: &'static str }` with `#[error("{VIDEO_COMPOSE_CONFIG_INVALID}: {reason}")]`, and `VideoComposeToolset::try_new` which validates: `ffmpeg_path`/`ffprobe_path` are absolute (`path.is_absolute()`, else `ConfigInvalid { reason: "binary paths must be absolute" }`), `render_timeout` in `(0, 1 hour]`, creates `scratch_dir` (`create_dir_all`, failure → `ConfigInvalid { reason: "scratch directory is unavailable" }`), and builds the two `ToolSpec`s:

`compose_video` — description `"Merge stored clips into one movie with declarative transitions and audio."`; `Sequential`, `NonIdempotentWrite`, `AtMostOnce`, approval `Policy` reason `"local media rendering"`, `max_result_bytes: 262_144`, `deferral: Never`. Input schema is the composition spec (write the JSON schema mirroring the serde types: required `version`, `clips`, `output`; `clips` items require `media_ref`; transitions items require `type` with `enum ["cut","crossfade","fade_to_black"]`; audio requires `media_ref` and `mode` with `enum ["replace","mix"]`; subtitles requires `media_ref` and `mode` with `enum ["burn_in","mux"]` plus optional `style` (`font_size`, `margin_v` integers); output requires `container` with `enum ["mp4","webm"]`; `additionalProperties: false` at every level; inline the same `media_ref` object schema used in Task 4). Output schema:

```rust
br#"{"additionalProperties":false,"properties":{"duration_s":{"type":"number"},"length":{"type":"integer"},"media_ref":{"additionalProperties":false,"properties":{"digest":{"type":"string"},"id":{"type":"string"},"length":{"type":"integer"},"media_type":{"type":"string"},"scope_digest":{"type":"string"}},"required":["id","digest","length","media_type","scope_digest"],"type":"object"}},"required":["media_ref","duration_s","length"],"type":"object"}"#
```

`probe_media` — description `"Probe duration, dimensions, and streams of one stored media object."`; `Sequential`, read-classified side effect, `Idempotent` retry safety, approval `Never`, `max_result_bytes: 65_536`. Input `{"media_ref": <object>}` required; output:

```rust
br#"{"additionalProperties":false,"properties":{"duration_s":{"type":"number"},"fps":{"type":"number"},"has_audio":{"type":"boolean"},"height":{"type":"integer"},"media_type":{"type":"string"},"width":{"type":"integer"}},"required":["duration_s","has_audio"],"type":"object"}"#
```

Until Task 8, give `Toolset::call` a skeleton returning `VIDEO_COMPOSE_INVALID_ARGUMENTS` for every call so the crate compiles.

- [ ] **Step 5: Run and commit**

Run: `cargo test -p finstack-ai-tools-video-compose`
Expected: PASS (spec tests; construction compiles).

```bash
git add Cargo.toml Cargo.lock extensions/toolsets/finstack-ai-tools-video-compose
git commit -m "Scaffold the video compose toolset with a validated composition spec"
```

---

### Task 7: Pure ffmpeg argument builder

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/graph.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-video-compose/src/lib.rs` (add `mod graph;`)

**Interfaces:**
- Consumes: Task 6's spec types.
- Produces: `pub(crate) struct ClipInput { pub path: PathBuf, pub duration_s: f64 }` and `pub(crate) fn build_ffmpeg_args(spec: &CompositionSpec, clips: &[ClipInput], audio: Option<&Path>, subtitles: Option<&Path>, output: &Path) -> Result<Vec<std::ffi::OsString>, &'static str>`. Pure — no I/O, fully unit-testable. Task 8 supplies real durations from ffprobe.

- [ ] **Step 1: Failing tests**

```rust
    // helpers: spec builders reuse Task 6's test helpers via a small local copy;
    // clip(path, dur) -> ClipInput.

    fn rendered(args: &[std::ffi::OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn cut_only_composition_uses_concat() {
        let spec = minimal(2, None);
        let clips = [clip("/in/a.mp4", 4.0), clip("/in/b.mp4", 6.0)];
        let args = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/out/movie.mp4"))
            .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.starts_with("-nostdin -y"));
        assert!(text.contains("-i /in/a.mp4"));
        assert!(text.contains("-i /in/b.mp4"));
        assert!(text.contains("concat=n=2:v=1:a=1"));
        assert!(text.ends_with("/out/movie.mp4"));
        assert!(!text.contains("xfade"));
    }

    #[test]
    fn crossfade_composition_chains_xfade_with_running_offsets() {
        let spec = minimal(
            3,
            Some(vec![
                TransitionSpec { kind: TransitionKind::Crossfade, duration_s: Some(1.0) },
                TransitionSpec { kind: TransitionKind::Crossfade, duration_s: Some(0.5) },
            ]),
        );
        let clips = [
            clip("/in/a.mp4", 4.0),
            clip("/in/b.mp4", 6.0),
            clip("/in/c.mp4", 5.0),
        ];
        let args = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/out/movie.mp4"))
            .expect("args");
        let text = rendered(&args).join(" ");
        // first xfade offset = 4.0 - 1.0; second = (4.0 + 6.0 - 1.0) - 0.5
        assert!(text.contains("xfade=transition=fade:duration=1:offset=3"));
        assert!(text.contains("xfade=transition=fade:duration=0.5:offset=8.5"));
        assert!(text.contains("acrossfade=d=1"));
    }

    #[test]
    fn trims_scale_fps_and_audio_are_encoded() {
        let mut spec = minimal(1, None);
        spec.clips[0].trim = Some(TrimSpec { start_s: 1.0, end_s: 3.5 });
        spec.output.resolution = Some("1280x720".into());
        spec.output.fps = Some(24);
        spec.audio = Some(AudioSpec {
            media_ref: media_ref(),
            mode: AudioMode::Replace,
            gain_db: Some(-6.0),
        });
        let clips = [clip("/in/a.mp4", 10.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            Some(Path::new("/in/music.mp3")),
            None,
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("trim=start=1:end=3.5"));
        assert!(text.contains("scale=1280:720"));
        assert!(text.contains("fps=24"));
        assert!(text.contains("-i /in/music.mp3"));
        assert!(text.contains("volume=-6dB"));
        assert!(text.contains("-shortest"));
    }

    #[test]
    fn burned_in_subtitles_apply_the_clamped_style() {
        let mut spec = minimal(1, None);
        spec.subtitles = Some(SubtitlesSpec {
            media_ref: media_ref(),
            mode: SubtitleMode::BurnIn,
            style: Some(SubtitleStyle { font_size: Some(42), margin_v: Some(80) }),
        });
        let clips = [clip("/in/a.mp4", 4.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            Some(Path::new("/scratch/subs.srt")),
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("subtitles=/scratch/subs.srt:force_style='FontSize=42,MarginV=80'"));
    }

    #[test]
    fn muxed_subtitles_add_an_input_and_the_mov_text_codec() {
        let mut spec = minimal(1, None);
        spec.subtitles = Some(SubtitlesSpec {
            media_ref: media_ref(),
            mode: SubtitleMode::Mux,
            style: None,
        });
        let clips = [clip("/in/a.mp4", 4.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            Some(Path::new("/scratch/subs.srt")),
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("-i /scratch/subs.srt"));
        assert!(text.contains("-c:s mov_text"));
        assert!(!text.contains("subtitles="));
    }

    #[test]
    fn clip_count_and_duration_mismatch_is_rejected() {
        let spec = minimal(2, None);
        assert!(build_ffmpeg_args(&spec, &[clip("/in/a.mp4", 4.0)], None, None, Path::new("/o.mp4")).is_err());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-tools-video-compose graph` — FAIL.

- [ ] **Step 3: Implement**

Build order (all args as `OsString`; numeric formatting via a `fn fmt_f64(v: f64) -> String` that trims trailing zeros: `format!("{v}")` on f64 already renders `3` for `3.0`? No — it renders `3`. Verify in a doctest; if not, use `format!("{}", v)` and strip `.0` suffix):

1. Global flags: `-nostdin -y -hide_banner -loglevel error`.
2. One `-i <clip.path>` per clip, then `-i <audio>` when present (audio input index = `clips.len()`).
3. Filtergraph assembled into one `-filter_complex` string:
   - Per clip `i`: `[i:v]trim=start=..:end=..,setpts=PTS-STARTPTS,` (only when trimmed) + `scale=W:H,` (when resolution set) + `fps=N,` (when set) + `setsar=1[v{i}]`; audio lane `[i:a]atrim=...,asetpts=PTS-STARTPTS[a{i}]` (atrim only when trimmed).
   - Effective per-clip duration = `trim.end_s - trim.start_s` when trimmed else `ClipInput.duration_s`.
   - No transitions or all `cut`: `[v0][a0][v1][a1]...concat=n=N:v=1:a=1[vc][ac]`.
   - Any non-cut transition: fold pairwise. Maintain `offset = duration(0)`; for join `k` (clips `k` and `k+1`, transition `t_k`): `crossfade` → `xfade=transition=fade:duration=D:offset={offset - D}` and `acrossfade=d=D`; `fade_to_black` → `xfade=transition=fadeblack:duration=D:offset={offset - D}` and `acrossfade=d=D`; `cut` mixed in → treat as `xfade` with `duration=0.01` (documented v1 simplification, keeps the fold uniform); after each join `offset += duration(k+1) - D`. Label intermediates `[vx{k}]`/`[ax{k}]`, final `[vc]`/`[ac]`.
   - Audio `replace`: map `[N:a]` (optionally `volume=GdB`) as `[music]`, output maps `-map "[vc]" -map "[music]"` plus `-shortest`. Audio `mix`: `[ac][music]amix=inputs=2:duration=first[am]`, map `[am]`. No audio spec: map `[ac]`.
   - Subtitles: when `spec.subtitles` is `Some` and `subtitles` (the path) is `None`, return `Err("subtitles path is required by the spec")`. `BurnIn`: append `[vc]subtitles={path}:force_style='FontSize={fs},MarginV={mv}'[vs]` to the filtergraph (style clause only when a style is set; escape `'` and `:` in the path with a backslash) and map `[vs]` instead of `[vc]`. `Mux`: no filter — add `-i {path}` as the last input and `-c:s mov_text` to the output flags (validation already pinned mp4).
4. Output flags: mp4 → `-c:v libx264 -pix_fmt yuv420p -c:a aac -movflags +faststart`; webm → `-c:v libvpx-vp9 -c:a libopus`. Then the output path.

Return `Err("clip inputs must match the spec")` when `clips.len() != spec.clips.len()` or any duration is not finite/positive.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-tools-video-compose`
Expected: PASS.

```bash
git add extensions/toolsets/finstack-ai-tools-video-compose
git commit -m "Build ffmpeg filtergraph arguments from validated composition specs"
```

---

### Task 8: Process execution, `probe_media`, `Toolset::call`

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/exec.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-video-compose/src/lib.rs` (real `Toolset::call`, `mod exec;`)

**Interfaces:**
- Consumes: Tasks 6–7; Task 2's `FakeMediaStore`.
- Produces: `pub(crate) async fn run_bounded(binary: &Path, args: &[OsString], timeout: Duration, cancellation: &CancellationSignal) -> Result<std::process::Output, ToolError>` (spawns via `tokio::process::Command`, `kill_on_drop(true)`, `tokio::select!` over child wait / cancellation / `tokio::time::sleep(timeout)`; timeout or cancel → kill + `VIDEO_COMPOSE_TIMEOUT`; non-zero exit → `VIDEO_COMPOSE_FFMPEG_FAILED` carrying the last 2 KiB of stderr, lossily UTF-8, as the bounded diagnostic); `pub(crate) struct ProbeResult { pub duration_s: f64, pub width: Option<u32>, pub height: Option<u32>, pub fps: Option<f64>, pub has_audio: bool }` and `pub(crate) async fn probe(ffprobe: &Path, target: &Path, timeout, cancellation) -> Result<ProbeResult, ToolError>`.
- Tool results: `compose_video` → `{"media_ref","duration_s","length"}`; `probe_media` → `{"duration_s","width"?,"height"?,"fps"?,"has_audio","media_type"?}`.

- [ ] **Step 1: Implement `exec.rs`**

`probe` runs `ffprobe -v error -print_format json -show_format -show_streams <target>` through `run_bounded` and parses:

```rust
#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    format: Option<FfprobeFormat>,
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
}
```

`duration_s` parses `format.duration` (missing/unparseable → `VIDEO_COMPOSE_MEDIA_FAILURE`); `fps` parses `"num/den"` (den 0 → `None`); `has_audio` = any stream with `codec_type == "audio"`.

- [ ] **Step 2: Implement `Toolset::call`**

Both arms start with `verify_authority(&ctx)?` and the identity check (as E2B). Shared helper `artifact_scope` — copy the Task 4 function into this crate verbatim (crate-private duplication is acceptable across extension leaves; they share no common crate besides the runtime).

`probe_media` arm: parse `{media_ref: MediaRef}` (`deny_unknown_fields`); `materialize` through the store (map `MediaError` → `VIDEO_COMPOSE_MEDIA_FAILURE`); `probe(...)`; serialize the result plus `"media_type": media_ref.media_type()`.

`compose_video` arm:
1. Parse `CompositionSpec`; `validate_spec` failure → `VIDEO_COMPOSE_SPEC_INVALID` with the returned reason.
2. Materialize every clip, the audio ref, and the subtitles ref when present (hold the `MaterializedMedia` values in a `Vec` so guards stay alive through the render).
3. `probe` each clip for duration → `ClipInput { path, duration_s }`.
4. Output path: `scratch_dir.join(format!("render-{}.{}", ctx.tool_call_id-derived hex or effect id hex, ext))` where ext matches the container. Derive the unique component from `ctx.run.effect_id` (`Debug`/hex form) — never from wall time.
5. `build_ffmpeg_args` with the materialized subtitles path (error → `VIDEO_COMPOSE_SPEC_INVALID`); `run_bounded(ffmpeg, ...)`.
6. `probe` the output for its real duration; `put_file` the output into the store (media type `video/mp4` or `video/webm`); best-effort `std::fs::remove_file` the scratch output; result `{"media_ref", "duration_s", "length": media.length()}`.

- [ ] **Step 3: Tests (unix-only stub binaries)**

Test helper writing executable stubs (`#[cfg(unix)]`, using `std::os::unix::fs::PermissionsExt` mode `0o755`):

```rust
    fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("stub");
        let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod");
        path
    }
```

- ffprobe stub: `echo '{"format":{"duration":"4.0"},"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"24/1"},{"codec_type":"audio"}]}'`.
- ffmpeg stub: `printf '%s\n' "$@" > "$CAPTURE_FILE"` is unavailable (no env) — instead have the stub write its args next to its own location: `d=$(dirname "$0"); printf '%s\n' "$@" > "$d/ffmpeg-args.txt"; last=$(eval echo \\${$#}); : > "$last"` (writes the args file and creates the empty output file named by the final argument). Note the eval trick extracts the last positional argument in POSIX sh.

Tests:
1. `compose_renders_through_the_stub_and_stores_the_output` — fake store with two tiny stored "clips"; spec with one crossfade; call `compose_video`; assert the stub's `ffmpeg-args.txt` contains `xfade=transition=fade` and both materialized clip paths; assert the result carries a `media_ref` whose bytes round-trip through the fake store (empty file → but `MediaRef::try_new` rejects zero length, so the ffmpeg stub writes one byte: use `printf 'x' > "$last"`).
2. `ffmpeg_failure_surfaces_bounded_stderr` — ffmpeg stub: `echo "boom: filter parse error" >&2; exit 1` → error code `video_compose_ffmpeg_failed`; the error's message/metadata contains `filter parse error` and is shorter than 2 KiB.
3. `render_timeout_kills_the_child` — ffmpeg stub `sleep 30`; config `render_timeout: Duration::from_millis(200)` → `video_compose_timeout` in well under 30 s.
4. `probe_media_maps_ffprobe_json` — assert `duration_s == 4.0`, `width == 640`, `fps == 24.0`, `has_audio == true`.
5. `burned_in_subtitles_reach_ffmpeg` — store a small SRT file in the fake store; spec with `subtitles: {media_ref, mode: burn_in}`; assert `ffmpeg-args.txt` contains `subtitles=` and the materialized SRT path.
6. `invalid_spec_is_rejected_before_any_process_runs` — 3 clips + 1 transition; ffmpeg stub writes a marker file when invoked; assert `video_compose_spec_invalid` and no marker file.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-tools-video-compose`
Expected: PASS.

```bash
git add extensions/toolsets/finstack-ai-tools-video-compose
git commit -m "Execute bounded ffmpeg renders and probes behind the compose tools"
```

---

## Phase D — MoviePlan and the pipeline driver

### Task 9: MoviePlan schema, Rust types, fixtures

**Files:**
- Create: `schemas/movie-plan/movie-plan.v1.json`
- Modify: `schemas/schema-families.toml` (add the `movie-plan` family)
- Modify: `Cargo.toml` (workspace root — members + deps entry `finstack-ai-workflow-media-pipeline = { path = "extensions/workflow/finstack-ai-workflow-media-pipeline", version = "1.0.0" }`)
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/Cargo.toml`
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/README.md`
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs`
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/plan.rs`
- Create: `fixtures/compatibility/movie-plan/valid-two-scene.json`
- Create: `fixtures/compatibility/movie-plan/invalid-missing-prompt.json`
- Create: `fixtures/compatibility/movie-plan/invalid-ambiguous-frame.json`

**Interfaces:**
- Produces (plan.rs): `pub struct MoviePlan { pub version: u32, pub title: Option<String>, pub defaults: PlanDefaults, pub scenes: Vec<SceneSpec>, pub transitions: Option<Vec<PlanTransition>>, pub audio: Option<PlanAudio>, pub output: PlanOutput }`, `pub struct PlanDefaults { pub image_model: String, pub video_model: String, pub resolution: Option<String>, pub aspect_ratio: Option<String>, pub scene_duration_s: u32 }`, `pub struct SceneSpec { pub id: String, pub video_prompt: String, pub start_frame: FrameSource, pub end_frame: Option<FrameSource>, pub reference_images: Option<Vec<FrameSource>>, pub duration_s: Option<u32>, pub seed: Option<i64>, pub captions: Option<Vec<CaptionCue>>, pub overrides: Option<SceneOverrides> }`, `pub struct CaptionCue { pub text: String, pub start_s: f64, pub end_s: f64 }`, `pub struct SceneOverrides { pub image_model: Option<String>, pub video_model: Option<String>, pub resolution: Option<String> }`, `pub enum FrameSource { Prompt { prompt: String }, MediaRef { media_ref: MediaRef }, Url { url: String } }` (serde untagged, each variant a single-key object, `deny_unknown_fields` on inner structs), `pub struct PlanTransition { pub after: String, pub kind: TransitionKindName, pub duration_s: Option<f64> }` (`kind` renamed `type`, values `cut`/`crossfade`/`fade_to_black`), `pub struct PlanAudio { pub media_ref: MediaRef, pub mode: String }`, `pub struct PlanOutput { pub container: String, pub fps: Option<u32>, pub captions: Option<CaptionsMode> }`, `pub enum CaptionsMode { None, Sidecar, BurnIn }` (serde `snake_case`).
- Produces: `pub struct PlanLimits { pub max_scenes: usize, pub max_total_video_s: u64, pub max_concurrent_jobs: usize }` and `pub fn validate_plan(plan: &MoviePlan, limits: &PlanLimits) -> Result<PlanBudget, &'static str>` where `pub struct PlanBudget { pub scene_count: usize, pub total_video_s: u64 }`. Scene ids must be unique, non-empty, `[a-z0-9-]{1,64}`; every `transitions[].after` names an existing non-final scene; `version == 1`; per-scene duration = `duration_s.unwrap_or(defaults.scene_duration_s)`, all within `1..=60`; totals within limits.

- [ ] **Step 1: Schema family registration and JSON schema**

Append to `schemas/schema-families.toml` (matching the existing entry format):

```toml
[families."movie-plan"]
owner = "me@jeickmeier.com"
reviewer = "me@jeickmeier.com"
source_root = "schemas/movie-plan/"
format = "json-schema-2020-12"
stability = "candidate-v1"
compatibility_profile = "strict-reject-unknown"
fixture_root = "fixtures/compatibility/movie-plan/"
status = "active"
```

Write `schemas/movie-plan/movie-plan.v1.json` as a JSON Schema 2020-12 document mirroring the Rust types above exactly: `$id: "https://finstack.ai/schemas/movie-plan/v1"`, top-level required `["version","defaults","scenes","output"]`, `version` `const: 1`, `scenes` `minItems: 1`, `additionalProperties: false` at every object level, `frame_source` in `$defs` as a `oneOf` of the three single-required-key objects, `media_ref` in `$defs` (id/digest/length/media_type/scope_digest as in Task 4), scene `id` with `pattern: "^[a-z0-9-]{1,64}$"`, transition `type` enum, audio `mode` enum `["replace","mix"]`, output `container` enum `["mp4","webm"]`, output `captions` enum `["none","sidecar","burn_in"]`, scene `captions` as an array (`maxItems: 32`) of required `{text, start_s, end_s}` with `text` `maxLength: 200`.

- [ ] **Step 2: Crate scaffolding**

Manifest (workflow-driver crate, no HTTP of its own):

```toml
[package]
name = "finstack-ai-workflow-media-pipeline"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "MoviePlan media pipeline driver and tools for finstack-ai"
readme = "README.md"

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
futures-util = { workspace = true }
rusqlite = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tempfile = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt"] }

[lints]
workspace = true
```

README: MoviePlan pipeline — tick-based, resumable, adapter-owned sqlite state in the workflow-local mold; tools `render_movie`/`advance_render`/`get_render_status`; hosts bound spend via `PlanLimits`; T1 native, not isolated.

- [ ] **Step 3: Failing plan tests**

In `src/plan.rs` tests:

```rust
    fn limits() -> PlanLimits {
        PlanLimits { max_scenes: 10, max_total_video_s: 120, max_concurrent_jobs: 2 }
    }

    #[test]
    fn valid_fixture_parses_and_budgets() {
        let bytes = std::fs::read(
            finstack_ai_test::repo_root().join("fixtures/compatibility/movie-plan/valid-two-scene.json"),
        )
        .expect("fixture");
        let plan: MoviePlan = serde_json::from_slice(&bytes).expect("parse");
        let budget = validate_plan(&plan, &limits()).expect("valid");
        assert_eq!(budget.scene_count, 2);
        assert_eq!(budget.total_video_s, 14); // scene-01 explicit 8s + scene-02 default 6s
    }

    #[test]
    fn invalid_fixtures_are_rejected() {
        for name in ["invalid-missing-prompt.json", "invalid-ambiguous-frame.json"] {
            let bytes = std::fs::read(
                finstack_ai_test::repo_root().join("fixtures/compatibility/movie-plan").join(name),
            )
            .expect("fixture");
            assert!(
                serde_json::from_slice::<MoviePlan>(&bytes).is_err(),
                "{name} must fail to parse"
            );
        }
    }

    #[test]
    fn budget_and_reference_violations_fail_closed() {
        let mut plan = two_scene_plan(); // helper mirroring the valid fixture in Rust
        plan.scenes[1].id = plan.scenes[0].id.clone();
        assert!(validate_plan(&plan, &limits()).is_err(), "duplicate ids");

        let mut plan = two_scene_plan();
        plan.transitions = Some(vec![PlanTransition {
            after: "scene-99".into(),
            kind: TransitionKindName::Crossfade,
            duration_s: Some(0.5),
        }]);
        assert!(validate_plan(&plan, &limits()).is_err(), "unknown after");

        let plan = two_scene_plan();
        let tight = PlanLimits { max_scenes: 1, ..limits() };
        assert!(validate_plan(&plan, &tight).is_err(), "scene ceiling");
        let tight = PlanLimits { max_total_video_s: 5, ..limits() };
        assert!(validate_plan(&plan, &tight).is_err(), "seconds ceiling");

        let mut plan = two_scene_plan();
        plan.scenes[0].captions = Some(vec![CaptionCue {
            text: "x".repeat(300),
            start_s: 0.0,
            end_s: 2.0,
        }]);
        assert!(validate_plan(&plan, &limits()).is_err(), "caption text bound");

        let mut plan = two_scene_plan();
        plan.scenes[0].captions = Some(vec![
            CaptionCue { text: "one".into(), start_s: 0.0, end_s: 4.0 },
            CaptionCue { text: "two".into(), start_s: 3.0, end_s: 6.0 },
        ]);
        assert!(validate_plan(&plan, &limits()).is_err(), "overlapping cues");
    }
```

`valid-two-scene.json` fixture content (use a literal 64-char lowercase hex string for both digests):

```json
{
  "version": 1,
  "title": "Two scene test",
  "defaults": {
    "image_model": "test/image-model",
    "video_model": "test/video-model",
    "resolution": "720p",
    "scene_duration_s": 6
  },
  "scenes": [
    {
      "id": "scene-01",
      "video_prompt": "camera glides across a harbor at dawn",
      "start_frame": { "prompt": "wide shot of a harbor at dawn, golden light" },
      "end_frame": { "prompt": "close-up of a moored fishing boat" },
      "duration_s": 8,
      "seed": 42,
      "captions": [
        { "text": "Harbors wake up slowly.", "start_s": 0.0, "end_s": 3.5 },
        { "text": "Then all at once.", "start_s": 3.5, "end_s": 7.5 }
      ]
    },
    {
      "id": "scene-02",
      "video_prompt": "gulls lift off the pier",
      "start_frame": { "url": "https://example.test/pier.png" }
    }
  ],
  "transitions": [
    { "after": "scene-01", "type": "crossfade", "duration_s": 0.5 }
  ],
  "output": { "container": "mp4", "fps": 24, "captions": "burn_in" }
}
```

`invalid-missing-prompt.json`: same but scene-01's `start_frame` is `{}` (matches no `FrameSource` variant). `invalid-ambiguous-frame.json`: scene-01's `start_frame` is `{"prompt": "x", "url": "https://example.test/x.png"}` — with `deny_unknown_fields` on each untagged variant this matches none. **Check:** serde `untagged` + `deny_unknown_fields` interaction; if the two-key object accidentally parses, switch `FrameSource` to a manual `Deserialize` impl that inspects the object's key set and rejects anything but exactly one known key — the fixture test is the oracle.

- [ ] **Step 4: Implement `plan.rs` and register the crate**

Types as specified; `validate_plan`:

```rust
pub fn validate_plan(plan: &MoviePlan, limits: &PlanLimits) -> Result<PlanBudget, &'static str> {
    if plan.version != 1 {
        return Err("movie plan version must be 1");
    }
    if plan.scenes.is_empty() {
        return Err("movie plan needs at least one scene");
    }
    if plan.scenes.len() > limits.max_scenes {
        return Err("movie plan exceeds the scene ceiling");
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut total_video_s: u64 = 0;
    for scene in &plan.scenes {
        let valid_id = !scene.id.is_empty()
            && scene.id.len() <= 64
            && scene.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid_id {
            return Err("scene id must match [a-z0-9-]{1,64}");
        }
        if !seen.insert(scene.id.as_str()) {
            return Err("scene ids must be unique");
        }
        if scene.video_prompt.is_empty() {
            return Err("scene video prompt must not be empty");
        }
        let duration = u64::from(scene.duration_s.unwrap_or(plan.defaults.scene_duration_s));
        if !(1..=60).contains(&duration) {
            return Err("scene duration must be between 1 and 60 seconds");
        }
        if let Some(cues) = &scene.captions {
            if cues.len() > 32 {
                return Err("scene has more than 32 caption cues");
            }
            let mut previous_end = 0.0_f64;
            for cue in cues {
                if cue.text.is_empty() || cue.text.chars().count() > 200 {
                    return Err("caption text must be 1 to 200 characters");
                }
                let in_order = cue.start_s.is_finite()
                    && cue.end_s.is_finite()
                    && cue.start_s >= previous_end
                    && cue.end_s > cue.start_s
                    && cue.end_s <= duration as f64;
                if !in_order {
                    return Err("caption cues must be ordered and inside the scene duration");
                }
                previous_end = cue.end_s;
            }
        }
        total_video_s = total_video_s.saturating_add(duration);
    }
    if total_video_s > limits.max_total_video_s {
        return Err("movie plan exceeds the total video seconds ceiling");
    }
    let last_id = plan.scenes.last().map(|scene| scene.id.as_str());
    for transition in plan.transitions.iter().flatten() {
        if !seen.contains(transition.after.as_str()) {
            return Err("transition names an unknown scene");
        }
        if Some(transition.after.as_str()) == last_id {
            return Err("transition cannot follow the final scene");
        }
    }
    Ok(PlanBudget { scene_count: plan.scenes.len(), total_video_s })
}
```

`lib.rs` starts with the lint header, module doc `//! MoviePlan pipeline driver: adapter-journaled, tick-based, resumable.`, `mod plan;` and `pub use plan::{...}`. Register the crate in the workspace root.

- [ ] **Step 5: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline`
Expected: PASS.

```bash
git add Cargo.toml Cargo.lock schemas extensions/workflow/finstack-ai-workflow-media-pipeline fixtures/compatibility/movie-plan
git commit -m "Add the MoviePlan v1 schema, fixtures, and validated plan types"
```

---

### Task 10: Render state and its stores

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/state.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod state;` + re-exports)

**Interfaces:**
- Consumes: Task 9's plan types.
- Produces:

```rust
pub enum SceneStage { PendingStartFrame, PendingEndFrame, PendingSubmit, Polling, PendingDownload, Done, Failed }
pub struct SceneState {
    pub scene_id: String,
    pub stage: SceneStage,
    pub start_frame_ref: Option<MediaRef>,   // resolved or generated
    pub start_frame_url: Option<String>,     // when the plan pinned a URL
    pub end_frame_ref: Option<MediaRef>,
    pub end_frame_url: Option<String>,
    pub job_id: Option<String>,
    pub clip_ref: Option<MediaRef>,
    pub failure: Option<String>,             // bounded diagnostic
    pub resubmitted: bool,
}
pub enum RenderStatus { Validating, Running, Composing, Completed, Failed }
pub struct RenderState {
    pub tenant_scope: Arc<str>,
    pub render_id: Arc<str>,                 // "render-" + first 24 hex chars of the plan digest (deterministic; resubmission = resume)
    pub plan_json: String,                   // canonical plan bytes, replayed on resume
    pub plan_digest: Digest,
    pub status: RenderStatus,
    pub scenes: Vec<SceneState>,
    pub final_ref: Option<MediaRef>,
    pub transcript_srt_ref: Option<MediaRef>,
    pub transcript_vtt_ref: Option<MediaRef>,
    pub revision: u64,                       // optimistic-concurrency counter
}
pub trait RenderStateStore: Send + Sync {
    fn insert(&self, state: &RenderState) -> Result<(), StateError>;          // fails if render_id exists
    fn load(&self, tenant_scope: &str, render_id: &str) -> Result<Option<RenderState>, StateError>;
    fn update(&self, state: &RenderState) -> Result<bool, StateError>;        // CAS on revision; increments; false = lost race
}
pub struct MemoryRenderStateStore;   // BTreeMap<(tenant, render_id), RenderState> under Mutex
pub struct SqliteRenderStateStore;   // fn open(path) -> Result<Self, StateError>
pub enum StateError { Unavailable { code: &'static str }, Integrity { code: &'static str } }
```

`SceneStage`, `SceneState`, `RenderStatus`, `RenderState` all derive `Serialize`/`Deserialize` (`deny_unknown_fields`, enums `snake_case`) — sqlite persists one JSON blob per render.

- [ ] **Step 1: Failing tests**

```rust
    #[test]
    fn sqlite_store_round_trips_and_cas_guards_updates() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("journal.sqlite");
        let store = SqliteRenderStateStore::open(&path).expect("open");
        let mut state = sample_state(); // helper: one running render, revision 0
        store.insert(&state).expect("insert");
        assert!(store.insert(&state).is_err(), "duplicate render_id");
        let loaded = store
            .load("tenant-a", state.render_id.as_ref())
            .expect("load")
            .expect("present");
        assert_eq!(loaded.revision, 0);
        state.status = RenderStatus::Composing;
        assert!(store.update(&state).expect("update"), "first CAS wins");
        assert!(!store.update(&state).expect("update"), "stale revision loses");
        let reloaded = store
            .load("tenant-a", state.render_id.as_ref())
            .expect("load")
            .expect("present");
        assert_eq!(reloaded.revision, 1);
        assert!(matches!(reloaded.status, RenderStatus::Composing));
        assert!(
            store.load("tenant-b", state.render_id.as_ref()).expect("load").is_none(),
            "tenant isolation"
        );
    }

    #[test]
    fn memory_store_matches_sqlite_semantics() {
        // same assertions against MemoryRenderStateStore
    }

    #[test]
    fn two_sqlite_handles_share_one_table() {
        // open two SqliteRenderStateStore on the same path; insert via first,
        // load via second; update via second, assert first sees revision 1.
    }
```

- [ ] **Step 2: Implement**

Mirror `extensions/workflow/finstack-ai-workflow-local/src/store.rs` structurally (Mutex<Connection>, busy_timeout 1s, WAL off `:memory:`, `TransactionBehavior::Immediate` for CAS). DDL:

```sql
CREATE TABLE IF NOT EXISTS finstack_workflow_media_pipeline (
  tenant_scope TEXT NOT NULL,
  render_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  state_json TEXT NOT NULL,
  PRIMARY KEY (tenant_scope, render_id)
);
```

`insert`: `INSERT` (constraint violation → `StateError::Integrity { code: "render_exists" }`). `update`: `UPDATE ... SET revision = ?stored_revision + 1, state_json = ?json WHERE tenant_scope = ? AND render_id = ? AND revision = ?expected` inside an immediate transaction; `changes() == 1` is the CAS verdict; on success the serialized `state_json` must carry the **incremented** revision — set `state.revision + 1` into a clone before serializing. `load` deserializes `state_json` (corrupt → `Integrity { code: "state_json_invalid" }`).

- [ ] **Step 3: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline state`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Persist render state with optimistic concurrency in adapter-owned sqlite"
```

---

### Task 10b: Caption timeline and SRT/VTT serialization

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/subtitles.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod subtitles;`)

**Interfaces:**
- Consumes: Task 9's plan types.
- Produces (all `pub(crate)`): `struct TimedCue { text: String, start_s: f64, end_s: f64 }` (absolute movie-timeline seconds), `fn cue_timeline(plan: &MoviePlan) -> Vec<TimedCue>`, `fn to_srt(cues: &[TimedCue]) -> String`, `fn to_vtt(cues: &[TimedCue]) -> String`.

- [ ] **Step 1: Failing tests**

```rust
    #[test]
    fn timeline_offsets_scene_cues_and_subtracts_crossfade_overlap() {
        // two_scene_plan(): scene-01 8s with two cues, scene-02 6s (default),
        // crossfade 0.5s after scene-01. Give scene-02 one cue 0..2s.
        let mut plan = two_scene_plan();
        plan.scenes[1].captions = Some(vec![CaptionCue {
            text: "Gulls, incoming.".into(),
            start_s: 0.0,
            end_s: 2.0,
        }]);
        let cues = cue_timeline(&plan);
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].start_s, 0.0);
        assert_eq!(cues[1].end_s, 7.5);
        // scene-02 offset = 8.0 - 0.5 crossfade overlap
        assert_eq!(cues[2].start_s, 7.5);
        assert_eq!(cues[2].end_s, 9.5);
    }

    #[test]
    fn srt_and_vtt_render_known_answers() {
        let cues = vec![
            TimedCue { text: "Harbors wake up slowly.".into(), start_s: 0.0, end_s: 3.5 },
            TimedCue { text: "Then all at once.".into(), start_s: 3.5, end_s: 7.5 },
        ];
        assert_eq!(
            to_srt(&cues),
            "1\n00:00:00,000 --> 00:00:03,500\nHarbors wake up slowly.\n\n2\n00:00:03,500 --> 00:00:07,500\nThen all at once.\n\n"
        );
        assert_eq!(
            to_vtt(&cues),
            "WEBVTT\n\n00:00:00.000 --> 00:00:03.500\nHarbors wake up slowly.\n\n00:00:03.500 --> 00:00:07.500\nThen all at once.\n\n"
        );
    }

    #[test]
    fn hour_rollover_formats_correctly() {
        let cues = vec![TimedCue { text: "late".into(), start_s: 3_661.25, end_s: 3_662.0 }];
        assert!(to_srt(&cues).contains("01:01:01,250 --> 01:01:02,000"));
    }
```

(`two_scene_plan()` is a local copy of the plan.rs test helper — test-module duplication is fine. Write the expected strings with real newlines in the test file — the `\n` above marks them for this plan document.)

- [ ] **Step 2: Implement**

`cue_timeline`: walk scenes in order, `offset` starts at 0.0; per scene, emit each cue at `offset + start_s` / `offset + end_s`; after scene k, `offset += effective_duration(k) - overlap(k)` where `effective_duration` is `duration_s.unwrap_or(defaults.scene_duration_s)` as f64 and `overlap(k)` is the plan transition after scene k's `duration_s.unwrap_or(0.5)` for `crossfade`/`fade_to_black` and `0.0` for `cut`/absent. This mirrors the compose filtergraph's xfade offset math over *planned* durations; actual generated clips may drift slightly — documented v1 tolerance. Timestamp formatting: `fn stamp(seconds: f64, decimal: char) -> String` producing `HH:MM:SS{sep}mmm` from `(seconds * 1000.0).round() as u64`.

- [ ] **Step 3: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline subtitles`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Flatten plan-authored caption cues onto the movie timeline as SRT and VTT"
```

---

### Task 11: `MediaPipelineDriver` — submit and tick

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/driver.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod driver;` + re-exports)

**Interfaces:**
- Consumes: Tasks 9–10; the Layer 2/3 toolsets purely through `finstack_ai_runtime::Toolset`; Task 2's `FakeMediaStore` and `finstack_ai_test::ScriptedToolset` (dev).
- Produces:

```rust
pub struct MediaPipelineConfig {
    pub media_tools: Arc<dyn Toolset>,     // finstack-ai-tools-openrouter-media
    pub compose_tools: Arc<dyn Toolset>,   // finstack-ai-tools-video-compose
    pub state: Arc<dyn RenderStateStore>,
    pub media_store: Option<Arc<dyn MediaStore>>, // required by plans that use captions
    pub limits: PlanLimits,
}
pub struct MediaPipelineDriver { /* fields above */ }
impl MediaPipelineDriver {
    pub fn try_new(config: MediaPipelineConfig) -> Result<Self, PipelineError>;
    pub async fn submit_plan(&self, ctx: &ToolCallContext, plan_json: &[u8]) -> Result<RenderState, ToolError>;
    pub async fn advance(&self, ctx: &ToolCallContext, render_id: &str) -> Result<RenderState, ToolError>;
    pub fn status(&self, tenant_scope: &str, render_id: &str) -> Result<RenderState, ToolError>;
}
pub enum PipelineError { ConfigInvalid { reason: &'static str } }
```

plus crate-internal `async fn invoke_tool(toolset: &Arc<dyn Toolset>, tool_name: &str, arguments: serde_json::Value, ctx: &ToolCallContext) -> Result<serde_json::Value, ToolError>`.

- [ ] **Step 1: Implement `invoke_tool`**

First copy the E2B `tool_error` helper into this crate verbatim (`fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError`, from `extensions/toolsets/finstack-ai-sandbox-e2b/src/lib.rs:464`) and add:

```rust
fn stage_error(message: &'static str) -> ToolError {
    tool_error(MEDIA_PIPELINE_STAGE_FAILED, ErrorCategory::Tool, message)
}
```

(Declare the seven `MEDIA_PIPELINE_*` code constants from Global Constraints at the top of `lib.rs` in this task if not already present.) Then look the tool up in `toolset.tools()` by `model_name == tool_name` (unknown → `MEDIA_PIPELINE_STAGE_FAILED`), then build the call exactly as the E2B test module does:

```rust
let call = ValidatedToolCall {
    call: ToolCallBlock::try_new(
        ctx.tool_call_id,
        tool_name,
        RawJson::parse(serde_json::to_vec(&arguments).map_err(|_| stage_error("arguments"))?)
            .map_err(|_| stage_error("arguments"))?,
    )
    .map_err(|_| stage_error("arguments"))?,
    tool_id: spec.id.clone(),
    component: None,
    output_contract: EffectOutputContract {
        kind: EffectOutputKind::ToolResult,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"{}"),
    },
    retry_safety: spec.retry_safety,
    deadline: ctx.run.deadline,
    execution: spec.execution,
    failure_policy: ToolFailurePolicy::ReturnToModel,
};
let mut stream = toolset.call(ctx.clone(), call).await?;
```

(If `ToolCallContext` is not `Clone`, rebuild it field-by-field from `ctx` — every field is `Clone`.) Drain the stream to the `Completed` item; `is_error: true` results map to `MEDIA_PIPELINE_STAGE_FAILED` carrying the tool's code; parse `result.output.as_bytes()` into `serde_json::Value`. This composes the leaf tools in-process: the kernel-journaled effect is the wrapping `render_movie`/`advance_render` call, and per-stage durability is the adapter state store (workflow-local cron precedent).

- [ ] **Step 2: Implement `submit_plan`**

1. Parse `MoviePlan` (parse failure → `MEDIA_PIPELINE_PLAN_INVALID`); when any scene has caption cues or `output.captions` is `sidecar`/`burn_in`, require `config.media_store` (absent → `MEDIA_PIPELINE_PLAN_INVALID`, message `"captions require a media store"`); `validate_plan` against `limits` (violation → `MEDIA_PIPELINE_BUDGET_EXCEEDED` for the two ceiling messages, `MEDIA_PIPELINE_PLAN_INVALID` otherwise — match on the message constants).
2. `plan_digest = Digest::raw_json(plan_json)`; `render_id = format!("render-{}", &plan_digest.to_hex()[..24])` (via `.get(..24)`); tenant from `ctx.run.locator`.
3. Build initial `SceneState` per scene: pinned `FrameSource::MediaRef`/`Url` prefill `*_frame_ref`/`*_frame_url`; stage starts at `PendingStartFrame` when `start_frame` is a prompt, else `PendingEndFrame` when `end_frame` is a prompt, else `PendingSubmit`.
4. `state.insert(...)`; an existing render with the same id (same plan bytes) is **not** an error — return the loaded state instead (idempotent resubmission = resume).
5. Return the state; the caller (Task 12) runs one `advance` immediately after.

- [ ] **Step 3: Implement `advance` (one bounded tick)**

Load state (missing → `MEDIA_PIPELINE_NOT_FOUND`); a terminal state returns as-is. Parse `plan_json` back into `MoviePlan`. Then one pass over scenes, doing **at most one tool call per scene per tick** and respecting `limits.max_concurrent_jobs` (count scenes in `Polling`):

- `PendingStartFrame` / `PendingEndFrame`: `invoke_tool(media_tools, "openrouter_generate_image", {"model": scene-or-default image_model, "prompt": <frame prompt>}, ctx)`; store the returned `media_ref` (absent in the result → stage failure: the pipeline requires a store-backed media toolset); advance to the next pending stage.
- `PendingSubmit` (only while `polling_count < limits.max_concurrent_jobs`): build `openrouter_submit_video` arguments from the scene + defaults + overrides (`first_frame`/`last_frame` as `{"media_ref": ...}` or `{"url": ...}` from the stored fields; `duration_s`, `resolution`, `seed`, reference images); store `job_id`; stage → `Polling`.
- `Polling`: `invoke_tool(media_tools, "openrouter_get_video_job", {"job_id"}, ctx)`; `completed` → `PendingDownload`; `failed` → if `!resubmitted`, set `resubmitted = true`, clear `job_id`, stage → `PendingSubmit` (one retry); else stage → `Failed` with `failure = Some("video job failed")`; `pending`/`in_progress` → no change.
- `PendingDownload`: `invoke_tool(media_tools, "openrouter_download_video", {"job_id"}, ctx)`; store `clip_ref`; stage → `Done`.

A per-scene tool error marks that scene `Failed` (bounded `failure` message = the tool error's code) instead of aborting the tick; other scenes proceed. After the pass:

- any scene `Failed` and none in-flight → `RenderStatus::Failed`;
- all `Done` → **first**, when any scene has caption cues: build `cue_timeline(&plan)`, render `to_srt`/`to_vtt`, write each to a `tempfile::NamedTempFile`, `put_file` both into `media_store` (media types `application/x-subrip` and `text/vtt`), and record `transcript_srt_ref`/`transcript_vtt_ref` (persist this via `state.update` before composing, so a crash between transcript and compose resumes without regenerating). **Then** build the `compose_video` spec from the plan (clips in scene order via `clip_ref`, plan transitions mapped positionally — a plan transition `after: scene-k` becomes the spec's transition at index k; gaps filled with `cut`; audio/output copied; when `output.captions` is `burn_in` set `subtitles: {media_ref: transcript_srt_ref, mode: burn_in}`, when `sidecar` on mp4 set `mode: mux`, when `sidecar` on webm attach nothing — the refs alone are the deliverable), `invoke_tool(compose_tools, ...)`, store `final_ref`, → `Completed` (compose failure → `Failed`);
- otherwise `Running`.

Persist via `state.update(...)`; a lost CAS (`false`) → reload and return the newer state without retrying the tick (`MEDIA_PIPELINE_STORE_FAILURE` only on store errors). Honor `ctx.run.cancellation` between scenes (cancelled → persist progress so far, return current state).

**Resume-verification rule:** at tick start, for every scene ref recorded as produced (`clip_ref`, generated frame refs), nothing is re-verified here — verification happens where files are consumed (`materialize` inside compose/submit fails closed on digest mismatch and the scene's stage is then re-run by the recovery path below). On a `MEDIA_PIPELINE_STAGE_FAILED` whose underlying code is `video_compose_media_failure` during compose, demote every scene whose `clip_ref` fails a `materialize` probe back to `PendingDownload` if it still has a `job_id`, else to `PendingSubmit`, and set status back to `Running` — this is the at-least-once re-run.

- [ ] **Step 4: Driver tests (scripted toolsets)**

Use `finstack_ai_test::ScriptedToolset` if its plan/action vocabulary can express "return this JSON result for the next call" — read `crates/finstack-ai-test/src/scripted/toolset.rs` first; if it cannot, write a local `QueueToolset` test double (a `Toolset` whose `tools()` returns hand-built specs for the five tool names and whose `call` pops `(expected_tool_name, result_json)` from a `Mutex<VecDeque>`, panicking on mismatch — ~60 lines, test-module only). Tests (all `#[tokio::test]`, `tool_context()` copied from the E2B test module):

1. `happy_path_two_scene_render_reaches_completed` — queue: image gen (scene-01 start), image gen (scene-01 end), submit (scene-01), image-gen-free scene-02 (pinned URL start frame) submit, poll×2 pending, poll×2 completed, download×2, compose. Drive `submit_plan` + repeated `advance` until `Completed`; assert stage progression, `final_ref` present, `transcript_srt_ref`/`transcript_vtt_ref` present (the fixture plan has cues and `captions: "burn_in"`), the stored SRT bytes match the Task 10b known answer for the plan's cues, and the compose call's spec JSON carried 2 clips, 1 crossfade, and `subtitles.mode == "burn_in"` referencing the SRT ref.
2. `concurrency_cap_holds_back_submissions` — 3-scene plan, `max_concurrent_jobs: 1`; after two ticks assert exactly one scene has a `job_id`.
3. `failed_job_is_resubmitted_once_then_fails_the_scene` — poll returns `failed` twice around a resubmit; assert `resubmitted == true`, final scene stage `Failed`, render `Failed`.
4. `budget_violation_rejects_before_any_tool_call` — 2-scene plan against `max_scenes: 1`; assert error code `media_pipeline_budget_exceeded` and the queue untouched.
5. `resubmitting_the_same_plan_resumes_instead_of_duplicating` — `submit_plan` twice with identical bytes; assert one state row and the second call returns the persisted state.
6. `caption_plan_without_a_store_is_rejected_at_submit` — driver built with `media_store: None`; the fixture plan (which has cues) → `media_pipeline_plan_invalid`; queue untouched.

- [ ] **Step 5: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Drive MoviePlan renders through media tools with bounded resumable ticks"
```

---

### Task 12: Pipeline toolset — `render_movie`, `advance_render`, `get_render_status`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/tools.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod tools;` + `pub use tools::MediaPipelineToolset;`)

**Interfaces:**
- Consumes: Task 11's driver.
- Produces: `pub struct MediaPipelineToolset` (`try_new(driver: Arc<MediaPipelineDriver>) -> Result<Self, PipelineError>`, `impl Toolset`, descriptor name `finstack-media-pipeline`). Result JSON for all three tools:

```json
{"render_id":"...","status":"running","per_scene":[{"id":"scene-01","stage":"polling","job_id":"vid-1","clip_ref":{...}}],"final_media_ref":{...},"transcript_srt_ref":{...},"transcript_vtt_ref":{...}}
```

- [ ] **Step 1: Tool specs**

`render_movie` — description `"Validate and start one MoviePlan render, then run one pipeline tick."`; `Sequential`, `NonIdempotentWrite`, `AtMostOnce`, approval `Policy` reason `"paid multi-scene media generation"`, `max_result_bytes: 262_144`. Input schema: `{"additionalProperties":false,"properties":{"plan":{"type":"object"}},"required":["plan"],"type":"object"}` — the plan object itself is validated by `validate_plan`, not by the tool schema (the full MoviePlan schema lives in `schemas/movie-plan/`; embedding it in the ToolSpec would drift).
`advance_render` — description `"Run one bounded tick of a submitted render (submit due jobs, poll, download, compose)."`; same paid metadata; input `{"render_id": string}` required.
`get_render_status` — read-only (`Idempotent`, approval `Never`); input `{"render_id": string}`.
Shared output schema:

```rust
br#"{"additionalProperties":false,"properties":{"final_media_ref":{"type":"object"},"transcript_srt_ref":{"type":"object"},"transcript_vtt_ref":{"type":"object"},"per_scene":{"items":{"additionalProperties":false,"properties":{"clip_ref":{"type":"object"},"failure":{"type":"string"},"id":{"type":"string"},"job_id":{"type":"string"},"stage":{"type":"string"}},"required":["id","stage"],"type":"object"},"type":"array"},"render_id":{"type":"string"},"status":{"type":"string"}},"required":["render_id","status","per_scene"],"type":"object"}"#
```

- [ ] **Step 2: Dispatch**

`render_movie`: `verify_authority`; re-serialize the `plan` argument object to canonical bytes (`serde_json::to_vec`); `driver.submit_plan(&ctx, &bytes)` then one `driver.advance(&ctx, render_id)`; render the state. `advance_render`: `driver.advance`. `get_render_status`: `driver.status` (tenant from ctx locator). State→JSON rendering is one shared `fn render_state_json(state: &RenderState) -> serde_json::Value` (stage/status names via their serde `snake_case` serialization).

- [ ] **Step 3: Tests**

1. `render_movie_submits_and_ticks_once` — queued toolsets from Task 11's tests; call `render_movie` with the valid fixture plan inline; assert result `status == "running"` and `per_scene[0].stage` reflects one tick of progress.
2. `advance_render_progresses_to_completion` — loop `advance_render` until `status == "completed"`; assert `final_media_ref`, `transcript_srt_ref`, and `transcript_vtt_ref` present.
3. `get_render_status_is_read_only` — after completion, call it twice; assert the queue is untouched and results are identical.
4. `unknown_render_id_maps_to_not_found` — `advance_render` on `"render-missing"` → error code `media_pipeline_not_found`.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Expose the media pipeline as render, advance, and status tools"
```

---

### Task 13: Resume goldens

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/tests/resume.rs`

**Interfaces:**
- Consumes: everything from Tasks 9–12 plus `FakeMediaStore`.

- [ ] **Step 1: Write the golden tests**

1. `killed_after_scene_one_download_resumes_at_scene_two` — sqlite state store in a tempdir; drive a 2-scene render until scene-01 is `Done` and scene-02 is `Polling`; **drop the driver and toolsets** (simulated crash); build a fresh driver over the same sqlite path and fresh queued toolsets seeded only with scene-02's remaining calls (poll completed, download, compose); `advance` to completion; assert scene-01's `clip_ref` survived unchanged (same digest) and no scene-01 tool call was consumed from the new queue.
2. `corrupted_clip_fails_closed_then_reruns_the_scene` — complete both downloads against a `FakeMediaStore`; `store.corrupt(&scene01_clip_ref)`; make the compose queue return the `video_compose_media_failure`-coded error the real compose toolset produces on materialize failure; `advance`; assert scene-01 demoted to `PendingDownload` and status back to `Running`; seed a re-download + compose; assert `Completed`.
3. `resubmitted_plan_after_crash_returns_the_persisted_render` — `submit_plan`, crash, fresh driver, `submit_plan` same bytes; assert the returned state carries the pre-crash progress.

- [ ] **Step 2: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline --test resume`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Prove pipeline resume and fail-closed re-run with golden tests"
```

---

## Phase E — `finstack-ai-store-media-s3`

### Task 14: SigV4 signer

**Files:**
- Modify: `Cargo.toml` (workspace root — members entry, deps entry `finstack-ai-store-media-s3 = { path = "extensions/stores/finstack-ai-store-media-s3", version = "1.0.0" }`, and the one new dependency `hmac = { version = "0.12", default-features = false }`)
- Create: `extensions/stores/finstack-ai-store-media-s3/Cargo.toml`
- Create: `extensions/stores/finstack-ai-store-media-s3/README.md`
- Create: `extensions/stores/finstack-ai-store-media-s3/src/lib.rs` (skeleton: lint header, module doc, `mod sigv4;`)
- Create: `extensions/stores/finstack-ai-store-media-s3/src/sigv4.rs`

**Interfaces:**
- Produces (`sigv4.rs`, all `pub(crate)`):

```rust
pub(crate) struct SigningInputs<'a> {
    pub method: &'a str,               // "GET" | "PUT"
    pub host: &'a str,                 // "{bucket}.s3.{region}.amazonaws.com" or custom
    pub canonical_uri: &'a str,        // percent-encoded object path, leading '/'
    pub region: &'a str,
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub unix_ms: i64,                  // caller-supplied; never wall-clock inside
    pub payload_sha256_hex: &'a str,   // "UNSIGNED-PAYLOAD" for presigned GETs
}
pub(crate) fn amz_date(unix_ms: i64) -> (String, String);       // ("YYYYMMDD", "YYYYMMDDTHHMMSSZ")
pub(crate) fn uri_encode(input: &str, encode_slash: bool) -> String;  // AWS canonical encoding
pub(crate) fn presign_get_url(inputs: &SigningInputs<'_>, expires_s: u32) -> String;
pub(crate) fn authorization_header(inputs: &SigningInputs<'_>, extra_headers: &[(&str, &str)]) -> (String, String);
    // returns (x-amz-date value, Authorization header value) for header-signed PUT/GET
```

- [ ] **Step 1: Scaffolding**

Manifest: deps `finstack-ai-runtime` (native-tokio), `futures-util`, `hmac = { workspace = true }`, `reqwest`, `serde`, `serde_json`, `sha2 = { workspace = true }`, `thiserror`, `tokio`; dev-deps `tempfile`, `tokio` (macros, rt, io-util, net), `finstack-ai-test`. Description `"S3 MediaStore backend for finstack-ai"`. README: SigV4-signed S3 backend, explicit credentials (never env), presigned GET URLs, streaming PUT/GET, the one `hmac` dependency, tested against loopback fixtures — never live AWS.

- [ ] **Step 2: Failing tests**

```rust
    #[test]
    fn amz_date_formats_utc_from_unix_ms() {
        // 2026-08-19T12:34:56Z
        let (date, datetime) = amz_date(1_786_797_296_000);
        assert_eq!(date, "20260819");
        assert_eq!(datetime, "20260819T123456Z");
        // Recompute 1_786_797_296_000 when writing the test:
        // days_from_civil(2026,8,19) * 86_400 + 12*3600 + 34*60 + 56, times 1000.
        // Assert both against an independent `date -u -d @<secs>` check.
    }

    #[test]
    fn uri_encoding_matches_aws_canonical_rules() {
        assert_eq!(uri_encode("media/clip 1.mp4", false), "media/clip%201.mp4");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        assert_eq!(uri_encode("tilde~dash-dot.us_", false), "tilde~dash-dot.us_");
        assert_eq!(uri_encode("ü", false), "%C3%BC");
    }

    #[test]
    fn presigned_url_carries_the_canonical_query_and_a_stable_signature() {
        let inputs = SigningInputs {
            method: "GET",
            host: "bucket.s3.us-east-1.amazonaws.com",
            canonical_uri: "/media/clip.mp4",
            region: "us-east-1",
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            unix_ms: 1_786_797_296_000,
            payload_sha256_hex: "UNSIGNED-PAYLOAD",
        };
        let url = presign_get_url(&inputs, 900);
        assert!(url.starts_with("https://bucket.s3.us-east-1.amazonaws.com/media/clip.mp4?"));
        for needle in [
            "X-Amz-Algorithm=AWS4-HMAC-SHA256",
            "X-Amz-Credential=AKIDEXAMPLE%2F20260819%2Fus-east-1%2Fs3%2Faws4_request",
            "X-Amz-Date=20260819T123456Z",
            "X-Amz-Expires=900",
            "X-Amz-SignedHeaders=host",
            "X-Amz-Signature=",
        ] {
            assert!(url.contains(needle), "missing {needle}");
        }
        // Golden stability pin: compute once with a hand-verified implementation,
        // then freeze. Replace <SIG> after the first verified run.
        assert!(url.ends_with("X-Amz-Signature=<SIG>"));
    }
```

Additionally embed one known-answer vector from AWS's published SigV4 documentation: fetch the worked "example: GET object" signature walkthrough from `https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html` at implementation time and add a test asserting `authorization_header` reproduces the documented signature for the documented inputs (documented example key `AKIAIOSFODNN7EXAMPLE` / documented secret / documented date). This pins the implementation to AWS's vector rather than to itself.

- [ ] **Step 3: Implement**

`amz_date` via the standard days-from-civil algorithm (Howard Hinnant's `civil_from_days`, integer-only):

```rust
pub(crate) fn amz_date(unix_ms: i64) -> (String, String) {
    let secs = unix_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    let date = format!("{y:04}{mo:02}{d:02}");
    let datetime = format!("{date}T{h:02}{m:02}{s:02}Z");
    (date, datetime)
}
```

`uri_encode`: unreserved `A-Za-z0-9-._~` pass through; `/` passes through unless `encode_slash`; everything else `%XX` uppercase per UTF-8 byte. Signing core (shared by both public functions):

```rust
type HmacSha256 = hmac::Hmac<sha2::Sha256>;

fn hmac_bytes(key: &[u8], data: &str) -> [u8; 32] {
    use hmac::Mac as _;
    let mut mac = <HmacSha256 as hmac::Mac>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("hmac accepts any key length"));
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().into()
}

fn sha256_hex(data: &str) -> String { /* sha2, hex-encode lowercase */ }

fn signing_key(secret: &str, date: &str, region: &str) -> [u8; 32] {
    let k = hmac_bytes(format!("AWS4{secret}").as_bytes(), date);
    let k = hmac_bytes(&k, region);
    let k = hmac_bytes(&k, "s3");
    hmac_bytes(&k, "aws4_request")
}
```

(The lint header denies `unreachable!` — use `.expect("hmac accepts any key length")` guarded by the crate-level allow, or return a `MediaError`; pick the crate's existing convention: for `new_from_slice` with Hmac the error is impossible, so a `#[allow(clippy::expect_used)]` on this one function with a comment is acceptable — mirror how other crates handle infallible constructors, e.g. E2B's `tool_error` uses `unwrap_or_else(Into::into)`.)

`presign_get_url`: canonical query (sorted): `X-Amz-Algorithm`, `X-Amz-Credential` (`{key}/{date}/{region}/s3/aws4_request`, uri-encoded), `X-Amz-Date`, `X-Amz-Expires`, `X-Amz-SignedHeaders=host`; canonical request `GET\n{canonical_uri}\n{canonical_query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD`; string-to-sign `AWS4-HMAC-SHA256\n{datetime}\n{date}/{region}/s3/aws4_request\n{sha256_hex(canonical_request)}`; signature = hex(hmac(signing_key, string_to_sign)); final URL appends `&X-Amz-Signature={sig}`.

`authorization_header`: header-signed variant — signed headers `host;x-amz-content-sha256;x-amz-date` plus sorted lowercase `extra_headers`; canonical request uses the real `payload_sha256_hex`; returns the `x-amz-date` value and `AWS4-HMAC-SHA256 Credential=..., SignedHeaders=..., Signature=...`.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-store-media-s3`
Expected: PASS (after replacing the `<SIG>` pin with the first verified value and embedding the AWS doc vector).

```bash
git add Cargo.toml Cargo.lock extensions/stores/finstack-ai-store-media-s3
git commit -m "Add a dependency-light SigV4 signer for the S3 media backend"
```

---

### Task 15: `S3MediaStore`

**Files:**
- Modify: `extensions/stores/finstack-ai-store-media-s3/src/lib.rs`

**Interfaces:**
- Consumes: Task 14's signer; Task 1's runtime contract.
- Produces: `pub struct S3MediaStoreConfig { pub endpoint: String, pub bucket: String, pub region: String, pub key_prefix: String, pub access_key_id: String, pub secret_access_key: String, pub presign_expiry: Duration, pub scratch_dir: PathBuf, pub max_object_bytes: Option<u64> }`, `pub struct S3MediaStore` with `pub fn try_new(S3MediaStoreConfig) -> Result<Self, MediaError>` and `impl MediaStore`. Object ids: `"{key_prefix}/{scope-digest-16-hex}/{content-digest-hex}"`. `endpoint` empty selects `https://{bucket}.s3.{region}.amazonaws.com` (virtual-hosted); non-empty is used verbatim for fixtures (loopback HTTP allowed, same `validate_endpoint` rules as the toolsets — copy that helper in).

- [ ] **Step 1: Failing tests (scripted loopback fixtures)**

Reuse the E2B test module's `respond`/listener scaffolding, adapted to also capture raw request bytes for header assertions:

1. `debug_and_errors_never_leak_the_secret_key` — canary secret `"s3-media-secret-canary-047"`; assert `Debug` on config and store, and every error string from a failed `try_new`, exclude the canary.
2. `put_file_streams_a_signed_put_and_returns_a_verified_ref` — fixture returns `200`; write a 64 KiB temp file; assert the captured request starts `put /media/`, carries `authorization: AWS4-HMAC-SHA256 Credential=...`, `x-amz-content-sha256` equal to the lowercase hex SHA-256 of the body, and a `content-length: 65536`; returned `MediaRef` has `length == 65_536` and the blob-content digest of the file.
3. `materialize_downloads_verifies_and_cleans_up` — fixture serves the same bytes on `get`; assert the materialized path is inside `scratch_dir`, bytes match, and the file disappears after dropping the `MaterializedMedia` (temp guard).
4. `materialize_rejects_tampered_bytes` — fixture serves different bytes → `MEDIA_INTEGRITY_FAILURE`, and the temp file is gone.
5. `presign_get_returns_a_signed_https_url` — no fixture needed against the default AWS endpoint form: construct with `endpoint: String::new()`, call `presign_get`, assert scheme `https`, host `bucket.s3.us-east-1.amazonaws.com`, and the five `X-Amz-*` parameters present. For a loopback endpoint, `presign_get` still signs (host = fixture host).
6. `foreign_scope_fails_closed_without_network` — `materialize` with a mismatched scope digest → `MEDIA_SCOPE_MISMATCH`; no request reaches the fixture.

- [ ] **Step 2: Implement**

- `try_new`: non-empty credentials (empty → `MediaError::InvalidMetadata { message: "s3_credentials_required" }` — configuration errors reuse `InvalidMetadata`... **no**: add none — use `MediaError::Unavailable { message }` for missing credentials/invalid endpoint to avoid overloading metadata; pick `Unavailable` and note it in the README), endpoint validation, `create_dir_all(scratch_dir)`, `reqwest::Client` with `http1_only` + 120 s timeout, key_prefix trimmed of slashes.
- **Clock:** SigV4 needs current time; the store takes it from `std::time::SystemTime::now()` at the call site (this is a leaf talking to a real external service — the determinism rules bind kernel state, not leaf transport; same posture as E2B's `now_unix_ms`). Factor as `fn now_unix_ms() -> i64` copied from E2B.
- `put_file`: `hash_file_blob`; optional `max_object_bytes` ceiling → `TooLarge`; compute the object key and canonical URI (`uri_encode(key, false)` with leading slash); stream the file as the PUT body via `reqwest::Body::wrap_stream(tokio_util ...)` — `tokio-util` is not a dependency; instead use `reqwest::Body::from(tokio::fs::File)` if available in reqwest 0.13, and if not, fall back to reading the file into memory **only when** `length <= 64 MiB` and rejecting larger files with `TooLarge` plus a README note (check reqwest 0.13's `Body` constructors at implementation time; `Body::from_stream` with a `tokio_util::io::ReaderStream` would need the `tokio-util` workspace pin — if reqwest offers neither, pin `tokio-util = { version = "0.7", default-features = false, features = ["io"] }` and record it as a second new dependency in the ADR-049 note). Sign with `authorization_header` including `("x-amz-content-sha256", &payload_hex)` where `payload_hex` is the plain SHA-256 (not blob-domain) of the body — computed in the same streaming pass as `hash_file_blob` by extending that pass locally (run both hashers over the file in one read loop, local to this crate).
- `materialize`: scope check first (no network on mismatch); GET the object streaming to `tempfile::Builder::new().prefix("s3-media-").tempfile_in(&scratch_dir)`; enforce `media.length()` as the byte ceiling while streaming; `verify_materialized`; return `MaterializedMedia::with_guard(path, Box::new(named_temp_file))` — keeping the `NamedTempFile` alive as the guard deletes on drop (`into_temp_path()` kept in the box).
- `presign_get`: scope check; build via `presign_get_url` with the configured expiry (`u32` seconds, clamp to `1..=604_800`).
- `delete`: signed `DELETE`; 204/200 → ok; 404 → `NotFound`.
- HTTP non-success → `MediaError::Unavailable { message: "s3_request_rejected" }` (never the body).

- [ ] **Step 3: Run and commit**

Run: `cargo test -p finstack-ai-store-media-s3`
Expected: PASS.

```bash
git add extensions/stores/finstack-ai-store-media-s3 Cargo.toml Cargo.lock
git commit -m "Implement the SigV4-signed S3 MediaStore backend with streaming transfers"
```

---

## Phase F — SDK wiring, docs, CI

### Task 16: Composition surface, bindings, docs, full CI

**Files:**
- Modify: `crates/finstack-ai/Cargo.toml` (optional deps: `finstack-ai-store-media-local`, `finstack-ai-tools-video-compose`, `finstack-ai-workflow-media-pipeline` under the `native-tokio` feature, mirroring the `finstack-ai-sandbox-e2b` entry exactly; `finstack-ai-store-media-s3` is **not** SDK-wired — hosts construct it directly, keeping the AWS-shaped leaf off the default graph)
- Modify: `crates/finstack-ai/src/agent/linked.rs` (spec structs on the linked constructors, mirroring how the OpenRouter plan's Task 13 added `OpenRouterMediaToolsSpec`: add `LocalMediaStoreSpec { root, max_total_bytes }`, `VideoComposeSpec { ffmpeg_path, ffprobe_path, scratch_dir }`, and a `media_pipeline: bool` flag that wires `MediaPipelineToolset` over the constructed media/compose toolsets and a `SqliteRenderStateStore` colocated with the sqlite journal path — follow the file's existing wiring idioms; read the E2B and openrouter-media blocks first and copy their shape)
- Modify: `scripts/wasm_package/check.py` (add the three new native crates to `FORBIDDEN_WASM`, next to the E2B entry)
- Modify: `bindings/finstack-ai-python/src/agent.rs` + `bindings/finstack-ai-python/python/finstack_ai/_finstack_ai.pyi` (kwargs mirroring the new spec structs on the same factories the OpenRouter plan touched)
- Modify: `bindings/finstack-ai-wasm/src/agent/agent.rs` (stubs reject the new fields with the existing stable "native only" error, mirroring the E2B handling)
- Modify: `docs/site/README.md` (add a "Media pipeline" pointer) and create `docs/site/media-pipeline.md` (usage guide: construct store → toolsets → pipeline; a full Rust snippet composing `LocalMediaStore` + `OpenRouterMediaToolset` + `VideoComposeToolset` + `MediaPipelineToolset`; the MoviePlan authoring contract with a link to `schemas/movie-plan/movie-plan.v1.json`; the scene-prompt authoring guidance — one section instructing text models how to write start/end-frame image prompts and motion prompts: concrete nouns, camera language, consistent style tokens across a scene's two frames, motion described relative to the start frame; and caption authoring guidance: short cues of at most ~7 words for short-form video, cue timing aligned to the scene beats, `output.captions: "burn_in"` recommended for muted autoplay platforms)
- Modify: `CHANGELOG.md` (one entry per crate under Unreleased/1.x per the file's convention)

**Interfaces:**
- Consumes: everything prior.
- Produces: the finished, CI-green feature.

- [ ] **Step 1: SDK + packaging wiring**

Follow the OpenRouter plan Task 13's file list and idioms exactly; every new optional dependency sits behind `native-tokio` with `dep:` syntax, and the wasm packaging check names all three crates. Keep the e2b-style guard test (`*_is_not_a_wasm_host_sdk_dependency`) — add one such test in `finstack-ai-tools-video-compose` asserting the compose crate stays off the `wasm-host` feature line.

- [ ] **Step 2: Bindings**

Python: kwargs `media_store_root`, `media_store_max_total_bytes`, `ffmpeg_path`, `ffprobe_path`, `compose_scratch_dir`, `media_pipeline` on the four linked factories; `.pyi` updated to match; a pytest asserting construction with `media_pipeline=True` plus the required paths succeeds and that `media_pipeline=True` **without** `ffmpeg_path` raises the mapped configuration error. WASM: new fields present in the stub structs and rejected with the stable native-only error; the existing wasm test pattern for E2B shows the assertion shape.

- [ ] **Step 3: Docs and changelog**

Write `docs/site/media-pipeline.md` per the file list above. Keep every code snippet compiling under `cargo test --doc` conventions if the site uses doctested snippets (check how `docs/site/rust.md` marks snippets; mirror it).

- [ ] **Step 4: Full verification gate**

Run, in order, and fix anything they surface:

```bash
mise run ci-rust
mise run ci-python
mise run ci-wasm
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai scripts/wasm_package/check.py bindings docs/site CHANGELOG.md
git commit -m "Wire the media pipeline stack through the SDK, bindings, and docs"
```

---

## Execution order and independence

Tasks are strictly ordered 1 → 16 except: Task 3 (local store) and Task 2 (fake) both depend only on Task 1; Tasks 14–15 (S3) depend only on Task 1 and may run any time after it; Tasks 4–5 additionally require the completed OpenRouter provider plan. Every task ends green and committed; the plan's layers are independently shippable in the spec's delivery order (spec §11).
