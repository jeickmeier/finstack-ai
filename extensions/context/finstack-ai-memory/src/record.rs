//! Bounded, validated memory record model.
//!
//! Every type here is a plain value: construction that can fail goes through
//! a `parse`/`try_new` constructor, and cross-field invariants on
//! [`MemoryRecord`] are checked by [`MemoryRecord::validate`]. Nothing here
//! performs I/O.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use finstack_ai_kernel::{ArtifactRef, Duration, Sensitivity, Timestamp};
use finstack_ai_runtime::{ArtifactScope, validate_artifact_scope};

/// Maximum byte length of a [`MemoryId`].
pub const MEMORY_ID_MAX_BYTES: usize = 256;
/// Maximum byte length of every memory scope dimension.
pub const MEMORY_SCOPE_FIELD_MAX_BYTES: usize = 256;
/// Maximum byte length of an optional provenance reference.
pub const MEMORY_PROVENANCE_FIELD_MAX_BYTES: usize = 512;
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryScope {
    /// Tenant identifier. Always required and always matched exactly.
    pub(crate) tenant: Arc<str>,
    /// Optional user identifier filter/binding.
    pub(crate) user: Option<Arc<str>>,
    /// Optional agent identifier filter/binding.
    pub(crate) agent: Option<Arc<str>>,
    /// Optional workspace identifier filter/binding.
    pub(crate) workspace: Option<Arc<str>>,
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
        if !valid_bounded_text(tenant, MEMORY_SCOPE_FIELD_MAX_BYTES) {
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

    /// Validate all scope dimensions.
    ///
    /// # Errors
    ///
    /// Rejects empty, NUL-bearing, or overlong populated fields.
    pub fn validate(&self) -> Result<(), MemoryError> {
        if !valid_bounded_text(&self.tenant, MEMORY_SCOPE_FIELD_MAX_BYTES) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_scope_tenant",
            });
        }
        for value in [
            self.user.as_deref(),
            self.agent.as_deref(),
            self.workspace.as_deref(),
        ] {
            if value.is_some_and(|value| !valid_bounded_text(value, MEMORY_SCOPE_FIELD_MAX_BYTES)) {
                return Err(MemoryError::InvalidRecord {
                    reason: "invalid_scope_dimension",
                });
            }
        }
        Ok(())
    }

    /// Canonical digest over every scope dimension.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidRecord`] when the scope is invalid or
    /// canonical encoding fails.
    pub fn digest(&self) -> Result<finstack_ai_kernel::Digest, MemoryError> {
        self.validate()?;
        let encoded =
            serde_json_canonicalizer::to_vec(self).map_err(|_| MemoryError::InvalidRecord {
                reason: "invalid_memory_scope",
            })?;
        finstack_ai_kernel::Digest::domain_separated("memory-scope", 1, &encoded).map_err(|_| {
            MemoryError::InvalidRecord {
                reason: "invalid_memory_scope",
            }
        })
    }

    /// Borrow the tenant identifier.
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// Return this scope narrowed to a validated `user`.
    ///
    /// # Errors
    ///
    /// Rejects an empty, NUL-bearing, or overlong user identifier.
    pub fn try_with_user(mut self, user: &str) -> Result<Self, MemoryError> {
        if !valid_bounded_text(user, MEMORY_SCOPE_FIELD_MAX_BYTES) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_scope_dimension",
            });
        }
        self.user = Some(Arc::from(user));
        Ok(self)
    }

    /// Return this scope narrowed to a validated `agent`.
    ///
    /// # Errors
    ///
    /// Rejects an empty, NUL-bearing, or overlong agent identifier.
    pub fn try_with_agent(mut self, agent: &str) -> Result<Self, MemoryError> {
        if !valid_bounded_text(agent, MEMORY_SCOPE_FIELD_MAX_BYTES) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_scope_dimension",
            });
        }
        self.agent = Some(Arc::from(agent));
        Ok(self)
    }

    /// Return this scope narrowed to a validated `workspace`.
    ///
    /// # Errors
    ///
    /// Rejects an empty, NUL-bearing, or overlong workspace identifier.
    pub fn try_with_workspace(mut self, workspace: &str) -> Result<Self, MemoryError> {
        if !valid_bounded_text(workspace, MEMORY_SCOPE_FIELD_MAX_BYTES) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_scope_dimension",
            });
        }
        self.workspace = Some(Arc::from(workspace));
        Ok(self)
    }

    /// Optional user dimension.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Optional agent dimension.
    #[must_use]
    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// Optional workspace dimension.
    #[must_use]
    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }

    /// Check whether `record_scope` is the exact same complete scope.
    #[must_use]
    pub fn permits(&self, record_scope: &MemoryScope) -> bool {
        self == record_scope
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
    Blob {
        /// Exact artifact scope required to retrieve and manage ownership.
        scope: ArtifactScope,
        /// Exact staged artifact reference.
        artifact: ArtifactRef,
    },
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

        self.scope.validate()?;

        if !valid_bounded_text(&self.preview, PREVIEW_MAX_BYTES) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_preview",
            });
        }

        if self.keywords.len() > KEYWORDS_MAX_COUNT {
            return Err(MemoryError::InvalidRecord {
                reason: "too_many_keywords",
            });
        }

        for keyword in self.keywords.iter() {
            if !valid_bounded_text(keyword, KEYWORD_MAX_BYTES) {
                return Err(MemoryError::InvalidRecord {
                    reason: "invalid_keyword",
                });
            }
        }

        for (index, keyword) in self.keywords.iter().enumerate() {
            if self.keywords[..index]
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(keyword))
            {
                return Err(MemoryError::InvalidRecord {
                    reason: "duplicate_keyword",
                });
            }
        }

        if let MemoryBody::Inline(body) = &self.body
            && !valid_bounded_text(body, INLINE_BODY_MAX_BYTES)
        {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_inline_body",
            });
        }
        if let MemoryBody::Blob { scope, artifact } = &self.body {
            validate_artifact_scope(scope, artifact).map_err(|_| MemoryError::InvalidRecord {
                reason: "invalid_artifact_reference",
            })?;
        }

        if self.provenance.confidence > CONFIDENCE_MAX {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_confidence",
            });
        }

        for value in [
            self.provenance.source_session.as_deref(),
            self.provenance.source_run.as_deref(),
            self.provenance.source_ref.as_deref(),
        ] {
            if value
                .is_some_and(|value| !valid_bounded_text(value, MEMORY_PROVENANCE_FIELD_MAX_BYTES))
            {
                return Err(MemoryError::InvalidRecord {
                    reason: "invalid_provenance",
                });
            }
        }

        if self.last_confirmed_at < self.created_at {
            return Err(MemoryError::InvalidRecord {
                reason: "last_confirmed_before_created",
            });
        }
        if self.supersedes.as_ref() == Some(&self.id)
            || self.superseded_by.as_ref() == Some(&self.id)
        {
            return Err(MemoryError::InvalidRecord {
                reason: "memory_self_supersession",
            });
        }
        if matches!(self.retention, RetentionPolicy::ExpireAfterMs(0)) {
            return Err(MemoryError::InvalidRecord {
                reason: "invalid_retention",
            });
        }
        if let RetentionPolicy::ExpireAfterMs(duration_ms) = self.retention {
            self.created_at
                .checked_add(Duration::from_millis(duration_ms))
                .map_err(|_| MemoryError::InvalidRecord {
                    reason: "retention_overflow",
                })?;
        }

        Ok(())
    }

    /// Whether this record is unavailable at `now` under its hard-retention policy.
    #[must_use]
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        match self.retention {
            RetentionPolicy::KeepUntilDeleted => false,
            RetentionPolicy::ExpireAfterMs(duration_ms) => self
                .created_at
                .checked_add(Duration::from_millis(duration_ms))
                .map_or(true, |expires_at| now >= expires_at),
        }
    }
}

fn valid_bounded_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.as_bytes().contains(&0)
}

/// Build a record preview from `body`, truncated to [`PREVIEW_MAX_BYTES`] at
/// a character boundary.
///
/// The bound is the same byte budget [`MemoryRecord::validate`] enforces, so
/// every writer that builds a preview this way passes validation regardless
/// of how many bytes each character occupies.
#[must_use]
pub fn preview_of(body: &str) -> Arc<str> {
    if body.len() <= PREVIEW_MAX_BYTES {
        return Arc::from(body);
    }
    let mut end = PREVIEW_MAX_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    Arc::from(&body[..end])
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
