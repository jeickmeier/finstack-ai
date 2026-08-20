//! Bounded, validated memory record model.
//!
//! Every type here is a plain value: construction that can fail goes through
//! a `parse`/`try_new` constructor, and cross-field invariants on
//! [`MemoryRecord`] are checked by [`MemoryRecord::validate`]. Nothing here
//! performs I/O.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use finstack_ai_kernel::{ArtifactRef, Sensitivity, Timestamp};

/// Maximum byte length of a [`MemoryId`].
pub const MEMORY_ID_MAX_BYTES: usize = 256;
/// Maximum byte length of a [`MemoryRecord::preview`].
pub const PREVIEW_MAX_BYTES: usize = 256;
/// Maximum byte length of a single keyword.
pub const KEYWORD_MAX_BYTES: usize = 128;
/// Maximum number of keywords on a record.
pub const KEYWORDS_MAX_COUNT: usize = 64;
/// Maximum accepted provenance confidence value.
pub const CONFIDENCE_MAX: u8 = 100;
/// Inline-vs-blob threshold for a [`MemoryBody`], in bytes.
///
/// A body at or under this size is stored as [`MemoryBody::Inline`]; a
/// larger one belongs in blob storage as [`MemoryBody::Blob`]. Defined here
/// rather than on a single writer so every capture path — the tool surface
/// and the observer alike — enforces the same ceiling.
pub const INLINE_BODY_MAX_BYTES: usize = 4096;

/// Errors raised while constructing or validating memory types.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MemoryError {
    /// Configuration is malformed.
    #[error("memory_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// A memory record (or one of its fields) failed validation.
    #[error("memory_record_invalid: {reason}")]
    InvalidRecord {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Bounded, validated memory identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(Arc<str>);

impl MemoryId {
    /// Parse a memory identifier, rejecting empty, NUL-containing, or
    /// overlong (> [`MEMORY_ID_MAX_BYTES`] bytes) input.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidRecord`] when `value` is empty,
    /// contains a NUL byte, or exceeds [`MEMORY_ID_MAX_BYTES`] bytes.
    pub fn parse(value: &str) -> Result<Self, MemoryError> {
        if value.is_empty() || value.len() > MEMORY_ID_MAX_BYTES || value.as_bytes().contains(&0) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_memory_id",
            });
        }
        Ok(Self(Arc::from(value)))
    }

    /// Borrow the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Scope filter/binding for a memory record: tenant is required, the rest
/// are optional narrowing dimensions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryScope {
    /// Tenant identifier. Always required and always matched exactly.
    pub tenant: Arc<str>,
    /// Optional user identifier filter/binding.
    pub user: Option<Arc<str>>,
    /// Optional agent identifier filter/binding.
    pub agent: Option<Arc<str>>,
    /// Optional workspace identifier filter/binding.
    pub workspace: Option<Arc<str>>,
}

impl MemoryScope {
    /// Construct a scope for `tenant`, rejecting empty or NUL-containing
    /// tenant identifiers.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidRecord`] when `tenant` is empty or
    /// contains a NUL byte.
    pub fn try_new(tenant: &str) -> Result<Self, MemoryError> {
        if tenant.is_empty() || tenant.as_bytes().contains(&0) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_scope_tenant",
            });
        }
        Ok(Self {
            tenant: Arc::from(tenant),
            user: None,
            agent: None,
            workspace: None,
        })
    }

    /// Borrow the tenant identifier.
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// Return a copy of this scope narrowed to `user`.
    #[must_use]
    pub fn with_user(mut self, user: &str) -> Self {
        self.user = Some(Arc::from(user));
        self
    }

    /// Return a copy of this scope narrowed to `agent`.
    #[must_use]
    pub fn with_agent(mut self, agent: &str) -> Self {
        self.agent = Some(Arc::from(agent));
        self
    }

    /// Return a copy of this scope narrowed to `workspace`.
    #[must_use]
    pub fn with_workspace(mut self, workspace: &str) -> Self {
        self.workspace = Some(Arc::from(workspace));
        self
    }

    /// Check whether `self`, used as a caller's scope filter, permits
    /// access to a record scoped as `record_scope`.
    ///
    /// The tenant must match exactly. Each optional field on `self` acts as
    /// a filter only when set: an unset field on `self` permits any value
    /// (including `None`) on `record_scope`, while a set field requires an
    /// exact match.
    #[must_use]
    pub fn permits(&self, record_scope: &MemoryScope) -> bool {
        if self.tenant.as_ref() != record_scope.tenant.as_ref() {
            return false;
        }

        scope_field_permits(self.user.as_deref(), record_scope.user.as_deref())
            && scope_field_permits(self.agent.as_deref(), record_scope.agent.as_deref())
            && scope_field_permits(self.workspace.as_deref(), record_scope.workspace.as_deref())
    }
}

/// Check one optional scope dimension: an unset `filter` permits anything,
/// a set `filter` requires `value` to match it exactly.
fn scope_field_permits(filter: Option<&str>, value: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(want) => value == Some(want),
    }
}

/// How a memory record's content entered the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMethod {
    /// The caller explicitly wrote this memory.
    Explicit,
    /// A tool call wrote this memory as a side effect.
    ToolWrite,
    /// An observer captured this memory from a run.
    ObserverCapture,
    /// This memory was imported from an external source.
    Imported,
}

/// Provenance metadata for a memory record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProvenance {
    /// Session that produced this memory, if known.
    pub source_session: Option<Arc<str>>,
    /// Run that produced this memory, if known.
    pub source_run: Option<Arc<str>>,
    /// Opaque reference to the originating source, if known.
    pub source_ref: Option<Arc<str>>,
    /// How this memory's content was extracted.
    pub extraction: ExtractionMethod,
    /// Confidence in this memory, `0..=100`. Validated by
    /// [`MemoryRecord::validate`].
    pub confidence: u8,
}

/// Memory content: either inline text or a reference to blob storage.
///
/// `ArtifactRef` is kept unboxed to match the interface every later task
/// (store, provider, toolset, observer) builds against; the size disparity
/// between variants is accepted deliberately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum MemoryBody {
    /// Content stored inline as text.
    Inline(Arc<str>),
    /// Content stored out-of-line, referenced by an artifact.
    Blob(ArtifactRef),
}

/// How long a memory record is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
    /// Retained indefinitely until explicitly deleted.
    KeepUntilDeleted,
    /// Retained for a bounded duration (milliseconds) after creation.
    ExpireAfterMs(u64),
}

/// A single memory record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecord {
    /// Bounded, validated identity.
    pub id: MemoryId,
    /// Scope this record is bound to.
    pub scope: MemoryScope,
    /// Search/filter keywords. Validated by [`MemoryRecord::validate`].
    pub keywords: Arc<[Arc<str>]>,
    /// Memory content.
    pub body: MemoryBody,
    /// Short human-readable preview of `body`. Validated by
    /// [`MemoryRecord::validate`].
    pub preview: Arc<str>,
    /// Sensitivity classification of this record.
    pub sensitivity: Sensitivity,
    /// Provenance of this record.
    pub provenance: MemoryProvenance,
    /// When this record was created.
    pub created_at: Timestamp,
    /// When this record was last confirmed still accurate.
    pub last_confirmed_at: Timestamp,
    /// Identifier of the record this one supersedes, if any.
    pub supersedes: Option<MemoryId>,
    /// Identifier of the record that supersedes this one, if any.
    pub superseded_by: Option<MemoryId>,
    /// Retention policy for this record.
    pub retention: RetentionPolicy,
    /// Whether this record has been tombstoned (soft-deleted).
    pub tombstoned: bool,
}

impl MemoryRecord {
    /// Validate cross-field invariants that construction alone does not
    /// enforce: `id` is valid, `preview` is at most
    /// [`PREVIEW_MAX_BYTES`] bytes, `keywords` each non-empty and at most
    /// [`KEYWORD_MAX_BYTES`] bytes with at most [`KEYWORDS_MAX_COUNT`]
    /// entries, and `provenance.confidence` is at most [`CONFIDENCE_MAX`].
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidRecord`] when any invariant above is
    /// violated.
    pub fn validate(&self) -> Result<(), MemoryError> {
        if self.id.as_str().is_empty()
            || self.id.as_str().len() > MEMORY_ID_MAX_BYTES
            || self.id.as_str().as_bytes().contains(&0)
        {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_memory_id",
            });
        }

        if self.preview.len() > PREVIEW_MAX_BYTES {
            return Err(MemoryError::InvalidRecord {
                reason: "preview_too_long",
            });
        }

        if self.keywords.len() > KEYWORDS_MAX_COUNT {
            return Err(MemoryError::InvalidRecord {
                reason: "too_many_keywords",
            });
        }

        for keyword in self.keywords.iter() {
            if keyword.is_empty() || keyword.len() > KEYWORD_MAX_BYTES {
                return Err(MemoryError::InvalidRecord {
                    reason: "invalid_keyword",
                });
            }
        }

        if self.provenance.confidence > CONFIDENCE_MAX {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_confidence",
            });
        }

        Ok(())
    }
}

/// A source of timestamps for memory operations.
///
/// Injected rather than read from ambient time directly so tests and
/// non-native targets can supply deterministic or platform-appropriate
/// clocks.
pub type MemoryClock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// Construct a [`MemoryClock`] backed by [`std::time::SystemTime`].
///
/// Clamped to [`finstack_ai_kernel::UNIX_EPOCH`] if the system clock is set
/// before the Unix epoch. Not available on `wasm32` targets; wasm callers
/// must inject their own clock.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn system_clock() -> MemoryClock {
    Arc::new(|| {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| {
                i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
            });
        Timestamp::from_unix_ms(millis).unwrap_or(finstack_ai_kernel::UNIX_EPOCH)
    })
}
