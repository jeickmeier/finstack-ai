//! `MemoryStore` port: the trait memory-backed extensions implement, plus
//! query/result types and an in-process reference implementation.
//!
//! Nothing here is opinionated about persistence; [`InProcessMemoryStore`]
//! is a reference/test implementation, and the `sqlite` feature (a later
//! task) adds a durable one.

use std::sync::Arc;

use thiserror::Error;

use finstack_ai_kernel::{ArtifactRef, Digest, Timestamp};
use finstack_ai_runtime::artifact::{ArtifactOwnerId, ArtifactScope, ArtifactStore};
use finstack_ai_runtime::ports::{PortFuture, PortObject};

use crate::record::{MemoryId, MemoryRecord, MemoryScope};

mod in_process;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
mod sqlite;

pub use in_process::{InProcessArtifactStore, InProcessMemoryStore};
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub use sqlite::SqliteMemoryStore;

/// Default maximum records retained by a bounded memory store.
pub const MAX_MEMORY_RECORDS: usize = 4_096;
/// Default maximum durable idempotency receipts.
pub const MAX_MEMORY_IDEMPOTENCY_KEYS: usize = 16_384;
/// Default aggregate inline-text byte ceiling.
pub const MAX_MEMORY_INLINE_BYTES: u64 = 16 * 1024 * 1024;
/// Maximum accepted search result count.
pub const MAX_MEMORY_SEARCH_RESULTS: usize = 256;
/// Maximum accepted list page size.
pub const MAX_MEMORY_PAGE_SIZE: usize = 256;
/// Default maximum pending artifact ownership actions.
pub const MAX_MEMORY_ARTIFACT_ACTIONS: usize = 16_384;
/// Maximum byte length of an idempotency key.
pub const MEMORY_IDEMPOTENCY_KEY_MAX_BYTES: usize = 256;

/// Effective finite limits for a memory store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStoreLimits {
    /// Maximum retained records, including tombstones and superseded records.
    pub max_records: usize,
    /// Maximum retained idempotency receipts.
    pub max_idempotency_keys: usize,
    /// Maximum aggregate bytes across inline bodies.
    pub max_inline_bytes: u64,
    /// Maximum search limit accepted from a caller.
    pub max_search_results: usize,
    /// Maximum list page size accepted from a caller.
    pub max_page_size: usize,
    /// Maximum unacknowledged artifact ownership actions.
    pub max_artifact_actions: usize,
}

impl Default for MemoryStoreLimits {
    fn default() -> Self {
        Self {
            max_records: MAX_MEMORY_RECORDS,
            max_idempotency_keys: MAX_MEMORY_IDEMPOTENCY_KEYS,
            max_inline_bytes: MAX_MEMORY_INLINE_BYTES,
            max_search_results: MAX_MEMORY_SEARCH_RESULTS,
            max_page_size: MAX_MEMORY_PAGE_SIZE,
            max_artifact_actions: MAX_MEMORY_ARTIFACT_ACTIONS,
        }
    }
}

/// Stable non-secret store identity and effective limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryStoreDescriptor {
    /// Stable implementation/configuration identity.
    pub store_id: Arc<str>,
    /// Whether records survive a host restart.
    pub durable: bool,
    /// Whether mutations atomically enqueue artifact ownership actions.
    pub manages_artifact_ownership: bool,
    /// Effective store limits.
    pub limits: MemoryStoreLimits,
}

/// Durable desired state change for one memory-owned artifact.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum MemoryArtifactAction {
    /// Keep the artifact reachable for an active memory record.
    Pin {
        /// Stable action identity used for acknowledgement.
        action_id: Digest,
        /// Exact artifact scope.
        scope: ArtifactScope,
        /// Exact artifact reference.
        artifact: ArtifactRef,
        /// Stable memory owner.
        owner: ArtifactOwnerId,
    },
    /// Release ownership after forget, correction, or expiry.
    Unpin {
        /// Stable action identity used for acknowledgement.
        action_id: Digest,
        /// Exact artifact scope.
        scope: ArtifactScope,
        /// Exact artifact reference.
        artifact: ArtifactRef,
        /// Stable memory owner.
        owner: ArtifactOwnerId,
        /// Logical time at which the artifact became unreferenced.
        now: Timestamp,
    },
}

impl MemoryArtifactAction {
    /// Stable action identity.
    #[must_use]
    pub const fn action_id(&self) -> Digest {
        match self {
            Self::Pin { action_id, .. } | Self::Unpin { action_id, .. } => *action_id,
        }
    }
}

/// A search query against a [`MemoryStore`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryQuery {
    /// Look up a single record by its exact identifier.
    ExactId(MemoryId),
    /// Match records whose keywords contain any of the given terms
    /// (case-insensitive).
    Keywords(Arc<[Arc<str>]>),
    /// Match records whose preview or inline body contains any normalized
    /// query token as a case-insensitive token prefix.
    FullText(Arc<str>),
}

/// Which part of a [`MemoryHit`] matched the query that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchEvidence {
    /// The record matched by exact identifier.
    ExactId,
    /// The record matched because it carries this keyword.
    Keyword(Arc<str>),
    /// The record matched a full-text substring search.
    FullText,
}

/// One search result: the matched record, a relevance score, and evidence
/// for why it matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHit {
    /// The matched record.
    pub record: MemoryRecord,
    /// Relevance score; higher is more relevant. Not comparable across
    /// query kinds beyond ordering within one [`MemoryStore::search`] call.
    pub score: u32,
    /// Why this record matched.
    pub matched: MatchEvidence,
}

/// Outcome of a [`MemoryStore::put`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    /// The record was newly inserted.
    Inserted,
    /// The idempotency key had already been applied; no write occurred.
    AlreadyApplied,
}

/// Pagination parameters for [`MemoryStore::list`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPage {
    /// Number of matching records to skip.
    pub offset: usize,
    /// Maximum number of records to return.
    pub limit: usize,
}

/// A page of listed records plus the total count of matching records
/// (pre-pagination).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryListing {
    /// Records in this page.
    pub records: Vec<MemoryRecord>,
    /// Total number of matching records across all pages.
    pub total: usize,
}

/// Errors raised by a [`MemoryStore`] implementation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MemoryStoreError {
    /// The store is temporarily unavailable (e.g. a poisoned lock or
    /// backend outage).
    #[error("memory_store_unavailable: {message}")]
    Unavailable {
        /// Stable non-secret reason.
        message: Arc<str>,
    },
    /// No record exists for the requested identifier/scope.
    #[error("memory_not_found")]
    NotFound,
    /// The caller's scope does not permit access to the requested record.
    #[error("memory_scope_mismatch")]
    ScopeMismatch,
    /// The record failed validation.
    #[error("memory_record_invalid: {reason}")]
    InvalidRecord {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// A record already exists under the identifier being written, and this
    /// write is not an idempotent replay of the write that created it.
    ///
    /// Raised by [`MemoryStore::put`] instead of silently replacing the
    /// existing record in the exact same complete scope. The same identifier
    /// may exist independently in a different scope. Supersede an existing
    /// record with [`MemoryStore::correct`] instead.
    #[error("memory_id_conflict")]
    IdConflict,
    /// One idempotency key was reused for a different operation or payload.
    #[error("memory_idempotency_conflict")]
    IdempotencyConflict,
    /// A finite store resource was exhausted.
    #[error("memory_capacity_exceeded: {resource} limit {limit} exceeded")]
    CapacityExceeded {
        /// Stable resource name.
        resource: &'static str,
        /// Configured ceiling.
        limit: u64,
    },
    /// A query, page, scope, or idempotency key was malformed.
    #[error("memory_request_invalid: {reason}")]
    InvalidRequest {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Durable memory storage: write, point-read, search, tombstone, supersede,
/// and list memory records.
///
/// Implementations must be object-safe; all operations are asynchronous and
/// scope-checked. Off `wasm32`, implementations must also be safely
/// shareable across threads ([`PortObject`] relaxes this on `wasm32`, where
/// the runtime is single-threaded and JS host adapters are not `Send`).
pub trait MemoryStore: PortObject {
    /// Stable implementation identity and effective limits.
    fn descriptor(&self) -> MemoryStoreDescriptor {
        MemoryStoreDescriptor {
            store_id: Arc::from("memory-store.unspecified"),
            durable: false,
            manages_artifact_ownership: false,
            limits: MemoryStoreLimits::default(),
        }
    }

    /// Insert `record`, keyed by `idempotency_key`. Re-applying the same key
    /// is a no-op that reports [`PutOutcome::AlreadyApplied`] rather than
    /// erroring or double-writing.
    ///
    /// `put` never replaces an existing record: a write whose identifier is
    /// already taken in the exact same scope and whose idempotency key is new fails
    /// with [`MemoryStoreError::IdConflict`]. Replacing the content of a
    /// remembered record goes through [`MemoryStore::correct`], which
    /// records the supersession link.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::InvalidRecord`] when `record` fails
    /// validation, [`MemoryStoreError::IdConflict`] when `record.id` is
    /// already taken in its exact scope, or [`MemoryStoreError::Unavailable`] on a backend
    /// failure.
    fn put(
        &self,
        idempotency_key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>>;

    /// Look up one record by identifier, scoped to `scope`.
    ///
    /// Returns `Ok(None)` when no live record exists for the exact `(scope,
    /// id)` key. Tombstoned, superseded, and expired records are not live.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] on a backend failure.
    fn get(
        &self,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>>;

    /// Search for records in the exact `scope` matching `query`, returning at
    /// most `limit` hits ordered by descending relevance.
    ///
    /// Tombstoned and superseded records are excluded.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] on a backend failure.
    fn search(
        &self,
        scope: MemoryScope,
        query: MemoryQuery,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>>;

    /// Tombstone (soft-delete) the record `id`, keyed by `idempotency_key`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::NotFound`] when no such live record exists
    /// in the exact `scope`, or [`MemoryStoreError::Unavailable`] on a
    /// backend failure.
    fn forget(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<(), MemoryStoreError>>;

    /// Supersede record `old` with `replacement`, keyed by
    /// `idempotency_key`: `replacement` is linked as superseding `old`, and
    /// `old` is linked as superseded by `replacement`.
    ///
    /// `replacement.id` must differ from `old`: a record that supersedes
    /// itself would be permanently invisible to search and recall.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::NotFound`] when `old` is not live in the exact
    /// `scope`, [`MemoryStoreError::InvalidRecord`] when `replacement`
    /// fails validation or `replacement.id` equals `old` (reason
    /// `memory_self_supersession`), or [`MemoryStoreError::Unavailable`] on
    /// a backend failure.
    fn correct(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        replacement: MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>>;

    /// List live records in the exact `scope`, ordered by identifier and
    /// paged by `page`. Tombstoned, superseded, and expired records are excluded.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] on a backend failure.
    fn list(
        &self,
        scope: MemoryScope,
        page: MemoryPage,
    ) -> PortFuture<Result<MemoryListing, MemoryStoreError>>;

    /// Return at most `limit` pending artifact ownership actions in stable
    /// dispatch order.
    fn pending_artifact_actions(
        &self,
        _limit: usize,
    ) -> PortFuture<Result<Vec<MemoryArtifactAction>, MemoryStoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Acknowledge one artifact action after the artifact store applied it.
    fn acknowledge_artifact_action(
        &self,
        _action_id: Digest,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Drain a bounded batch of memory artifact ownership actions.
///
/// Actions are acknowledged only after the artifact operation succeeds, so
/// failures remain replayable across retries and durable-store restarts.
///
/// # Errors
///
/// Returns a store error when either store is unavailable or rejects an
/// action. Successfully applied earlier actions remain acknowledged.
pub async fn reconcile_memory_artifacts(
    memory_store: &dyn MemoryStore,
    artifact_store: &dyn ArtifactStore,
    limit: usize,
) -> Result<usize, MemoryStoreError> {
    let actions = memory_store.pending_artifact_actions(limit).await?;
    let mut applied = 0_usize;
    for action in actions {
        let action_id = action.action_id();
        let result = match action {
            MemoryArtifactAction::Pin {
                scope,
                artifact,
                owner,
                ..
            } => artifact_store.pin(scope, artifact, owner).await,
            MemoryArtifactAction::Unpin {
                scope,
                artifact,
                owner,
                now,
                ..
            } => artifact_store.unpin(scope, artifact, owner, now).await,
        };
        result.map_err(|_| MemoryStoreError::Unavailable {
            message: Arc::from("memory_artifact_reconciliation_failed"),
        })?;
        memory_store.acknowledge_artifact_action(action_id).await?;
        applied = applied.saturating_add(1);
    }
    Ok(applied)
}

pub(crate) fn artifact_transition_actions(
    mutation_key: &str,
    old: Option<&MemoryRecord>,
    new: Option<&MemoryRecord>,
    now: Timestamp,
) -> Result<Vec<MemoryArtifactAction>, MemoryStoreError> {
    let mut actions = Vec::new();
    if let Some(record) = new
        && let crate::record::MemoryBody::Blob { scope, artifact } = &record.body
    {
        let owner = memory_artifact_owner(record)?;
        let action_id =
            artifact_action_id(mutation_key, "pin", actions.len(), scope, artifact, &owner)?;
        actions.push(MemoryArtifactAction::Pin {
            action_id,
            scope: scope.clone(),
            artifact: artifact.clone(),
            owner,
        });
    }
    if let Some(record) = old
        && let crate::record::MemoryBody::Blob { scope, artifact } = &record.body
    {
        let owner = memory_artifact_owner(record)?;
        let action_id = artifact_action_id(
            mutation_key,
            "unpin",
            actions.len(),
            scope,
            artifact,
            &owner,
        )?;
        actions.push(MemoryArtifactAction::Unpin {
            action_id,
            scope: scope.clone(),
            artifact: artifact.clone(),
            owner,
            now,
        });
    }
    Ok(actions)
}

pub(crate) fn normalize_search_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub(crate) fn validate_new_record_lifecycle(record: &MemoryRecord) -> Result<(), MemoryStoreError> {
    if record.tombstoned || record.supersedes.is_some() || record.superseded_by.is_some() {
        return Err(MemoryStoreError::InvalidRecord {
            reason: "memory_record_lifecycle_not_initial",
        });
    }
    Ok(())
}

fn memory_artifact_owner(record: &MemoryRecord) -> Result<ArtifactOwnerId, MemoryStoreError> {
    let scope_bytes = serde_json_canonicalizer::to_vec(&record.scope).map_err(|_| {
        MemoryStoreError::InvalidRecord {
            reason: "memory_scope_invalid",
        }
    })?;
    let scope_digest = Digest::domain_separated("memory-scope", 1, &scope_bytes).map_err(|_| {
        MemoryStoreError::InvalidRecord {
            reason: "memory_scope_invalid",
        }
    })?;
    let id_digest = Digest::domain_separated("memory-id", 1, record.id.as_str().as_bytes())
        .map_err(|_| MemoryStoreError::InvalidRecord {
            reason: "invalid_memory_id",
        })?;
    ArtifactOwnerId::try_new(format!(
        "memory:{}:{}",
        scope_digest.to_hex(),
        id_digest.to_hex()
    ))
    .map_err(|_| MemoryStoreError::InvalidRecord {
        reason: "memory_artifact_owner_invalid",
    })
}

fn artifact_action_id(
    mutation_key: &str,
    kind: &'static str,
    ordinal: usize,
    scope: &ArtifactScope,
    artifact: &ArtifactRef,
    owner: &ArtifactOwnerId,
) -> Result<Digest, MemoryStoreError> {
    let encoded =
        serde_json_canonicalizer::to_vec(&(mutation_key, kind, ordinal, scope, artifact, owner))
            .map_err(|_| MemoryStoreError::InvalidRecord {
                reason: "memory_artifact_action_invalid",
            })?;
    Digest::domain_separated("memory-artifact-action", 1, &encoded).map_err(|_| {
        MemoryStoreError::InvalidRecord {
            reason: "memory_artifact_action_invalid",
        }
    })
}
