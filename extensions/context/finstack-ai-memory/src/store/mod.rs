//! `MemoryStore` port: the trait memory-backed extensions implement, plus
//! query/result types and an in-process reference implementation.
//!
//! [`InProcessMemoryStore`] is the non-durable reference/test implementation.
//! On non-WASM targets, the `sqlite` feature exposes the durable
//! [`SqliteMemoryStore`].

use std::sync::Arc;

use thiserror::Error;

use finstack_ai_embeddings::embedder::TextEmbedder;
use finstack_ai_embeddings::vector::{EmbeddingVector, truncate_to_bytes};
use finstack_ai_kernel::{ArtifactRef, Digest, Timestamp};
use finstack_ai_runtime::artifact::{ArtifactOwnerId, ArtifactScope, ArtifactStore};
use finstack_ai_runtime::ports::{PortFuture, PortObject};

use crate::record::{
    INLINE_BODY_MAX_BYTES, KEYWORD_MAX_BYTES, KEYWORDS_MAX_COUNT, MemoryBody, MemoryError,
    MemoryId, MemoryRecord, MemoryScope,
};

mod in_process;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
mod sqlite;

pub use finstack_ai_runtime::artifact::InProcessArtifactStore;
pub use in_process::InProcessMemoryStore;
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
/// Default maximum retained age of an idempotency receipt (30 days).
pub const MAX_MEMORY_RECEIPT_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// Default maximum embedding-vector dimensionality accepted by a store.
pub const MAX_MEMORY_EMBEDDING_DIMENSIONS: usize = 4096;
/// Default maximum distinct embedding spaces (embedder identities) retained
/// by a store.
pub const MAX_MEMORY_EMBEDDING_SPACES: usize = 4;
/// Maximum byte length of an embedder identity.
const MEMORY_EMBEDDER_ID_MAX_BYTES: usize = 256;

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
    /// Maximum retained age of an idempotency receipt, in milliseconds.
    ///
    /// Older receipts are pruned on write-path sweeps. Re-sending a pruned
    /// key is a new operation, not a replay.
    pub max_receipt_age_ms: u64,
    /// Maximum embedding-vector dimensionality accepted from writes and
    /// queries.
    pub max_embedding_dimensions: usize,
    /// Maximum distinct embedding spaces (embedder identities) retained by
    /// the store's index.
    pub max_embedding_spaces: usize,
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
            max_receipt_age_ms: MAX_MEMORY_RECEIPT_AGE_MS,
            max_embedding_dimensions: MAX_MEMORY_EMBEDDING_DIMENSIONS,
            max_embedding_spaces: MAX_MEMORY_EMBEDDING_SPACES,
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
    /// Rank records by cosine similarity between `vector` and the record
    /// vectors stored for the embedding space `embedder_id`.
    ///
    /// The space must have been populated by the same embedder identity
    /// ([`store_embedding`](MemoryStore::store_embedding)); querying an
    /// unknown space returns no hits. Stores without an embedding index
    /// reject this query with [`MemoryStoreError::InvalidRequest`] (reason
    /// `memory_embeddings_unsupported`).
    Embedding {
        /// Embedding space to search: the producing embedder's stable
        /// identity.
        embedder_id: Arc<str>,
        /// Query vector; its dimensionality must match the space.
        vector: EmbeddingVector,
    },
}

/// Which part of a [`MemoryHit`] matched the query that produced it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchEvidence {
    /// The record matched by exact identifier.
    ExactId,
    /// The record matched because it carries this keyword.
    Keyword(Arc<str>),
    /// The record matched a full-text substring search.
    FullText,
    /// The record's stored embedding was similar to the query vector.
    Semantic,
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

/// One live record awaiting embedding for a space: its identity, canonical
/// source text, and the digest guarding against mid-reconcile rewrites.
///
/// Produced by [`MemoryStore::pending_embedding_sources`]; `text` is the
/// full [`embedding_source_text`] (callers truncate to the embedder's input
/// limit before embedding), and `source_digest` is its
/// [`embedding_source_digest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingSource {
    /// Scope the record is bound to.
    pub scope: MemoryScope,
    /// Record identity within `scope`.
    pub id: MemoryId,
    /// Canonical embedding source text of the record.
    pub text: Arc<str>,
    /// Digest of `text`, passed back to [`MemoryStore::store_embedding`] as
    /// the staleness guard.
    pub source_digest: Digest,
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

    /// Return at most `limit` live records that have no embedding row for
    /// the space `embedder_id`, in stable order.
    ///
    /// "Pending" is derived, not queued: the anti-join of live records
    /// against the space's index rows. A record rewritten after being
    /// listed simply reappears here on the next call. Stores without an
    /// embedding index report no pending work.
    fn pending_embedding_sources(
        &self,
        _embedder_id: Arc<str>,
        _limit: usize,
    ) -> PortFuture<Result<Vec<EmbeddingSource>, MemoryStoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Store `vector` for `(scope, id)` in the space `embedder_id`, guarded
    /// by `source_digest`.
    ///
    /// The store recomputes the record's current source digest and
    /// **silently no-ops** when it differs from `source_digest` or the
    /// record is no longer live — the record was rewritten or removed
    /// mid-reconcile, and the anti-join re-surfaces it if appropriate.
    /// Vectors are unit-normalized at write. The first vector stored in a
    /// space fixes that space's dimensionality.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::InvalidRequest`] when the embedder id is
    /// malformed, the vector exceeds
    /// [`MemoryStoreLimits::max_embedding_dimensions`], or its
    /// dimensionality mismatches the space (reason
    /// `memory_embedding_dimensions_mismatch`);
    /// [`MemoryStoreError::CapacityExceeded`] (resource `embedding_spaces`)
    /// when a new space would exceed
    /// [`MemoryStoreLimits::max_embedding_spaces`]; and, for stores without
    /// an embedding index, [`MemoryStoreError::InvalidRequest`] with reason
    /// `memory_embeddings_unsupported`.
    fn store_embedding(
        &self,
        _embedder_id: Arc<str>,
        _scope: MemoryScope,
        _id: MemoryId,
        _source_digest: Digest,
        _vector: EmbeddingVector,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        Box::pin(async {
            Err(MemoryStoreError::InvalidRequest {
                reason: "memory_embeddings_unsupported",
            })
        })
    }

    /// Drop every embedding row of the space `embedder_id` (embedder
    /// rotation). The index is derived data, so forgetting a space merely
    /// makes its records pending again; stores without an embedding index
    /// have nothing to drop.
    fn forget_embedding_space(
        &self,
        _embedder_id: Arc<str>,
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

/// Drain a bounded batch of the embedding index's pending records through
/// `embedder`.
///
/// Pending source texts are truncated to the embedder's declared input
/// limit (at a character boundary), batch-embedded, and stored under the
/// digest guard, so records rewritten mid-drain are silently skipped and
/// re-surface on the next run. The embedding index is derived, best-effort
/// data — callers on write paths swallow this function's errors rather
/// than failing the write.
///
/// Returns the number of applied embeddings; a partial or failed run
/// leaves the remainder pending for the next call.
///
/// # Errors
///
/// Returns [`MemoryStoreError::Unavailable`] with message
/// `memory_embedder_failed` when the embedder rejects the batch, and
/// propagates store errors from listing or writing rows.
pub async fn reconcile_memory_embeddings(
    store: &dyn MemoryStore,
    embedder: &dyn TextEmbedder,
    limit: usize,
) -> Result<usize, MemoryStoreError> {
    let descriptor = embedder.descriptor();
    let pending = store
        .pending_embedding_sources(Arc::clone(&descriptor.embedder_id), limit)
        .await?;
    if pending.is_empty() {
        return Ok(0);
    }
    let texts: Vec<Arc<str>> = pending
        .iter()
        .map(|source| Arc::from(truncate_to_bytes(&source.text, descriptor.max_input_bytes)))
        .collect();
    let vectors = embedder
        .embed(texts)
        .await
        .map_err(|_| memory_embedder_failed())?;
    if vectors.len() != pending.len() {
        return Err(memory_embedder_failed());
    }
    let mut applied = 0_usize;
    for (source, vector) in pending.into_iter().zip(vectors) {
        store
            .store_embedding(
                Arc::clone(&descriptor.embedder_id),
                source.scope,
                source.id,
                source.source_digest,
                vector,
            )
            .await?;
        applied = applied.saturating_add(1);
    }
    Ok(applied)
}

fn memory_embedder_failed() -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from("memory_embedder_failed"),
    }
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

/// Canonical embedding source text of `record`: preview, inline body, and
/// keywords — exactly the fields full-text search indexes. Blob bodies
/// contribute preview and keywords only (their segment is empty).
///
/// Canonical form, version 1: three newline-separated segments — the
/// preview, the inline body text (empty for blob bodies), and the keywords
/// joined by single spaces. This form feeds
/// [`embedding_source_digest`], whose staleness guard compares digests
/// byte-for-byte: changing this helper requires bumping the digest domain
/// version and dropping existing embedding spaces.
#[must_use]
pub fn embedding_source_text(record: &MemoryRecord) -> String {
    let body = match &record.body {
        MemoryBody::Inline(text) => text.as_ref(),
        MemoryBody::Blob { .. } => "",
    };
    let keywords = record
        .keywords
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(" ");
    format!("{}\n{}\n{}", record.preview, body, keywords)
}

/// Digest of a canonical [`embedding_source_text`] under the
/// `memory-embed-source` domain, version 1.
///
/// [`MemoryStore::store_embedding`] compares this digest against the
/// record's current source text to detect records rewritten mid-reconcile.
///
/// # Errors
///
/// Returns [`MemoryStoreError::InvalidRequest`] (reason
/// `memory_embedding_source_invalid`) when digest computation fails.
pub fn embedding_source_digest(text: &str) -> Result<Digest, MemoryStoreError> {
    Digest::domain_separated("memory-embed-source", 1, text.as_bytes()).map_err(|_| {
        MemoryStoreError::InvalidRequest {
            reason: "memory_embedding_source_invalid",
        }
    })
}

/// Map a dot product over unit-normalized vectors to a search score.
///
/// Clamps `dot` to `[-1, 1]`, then maps it linearly onto `0..=1_000_000`
/// (`-1 → 0`, `0 → 500_000`, `1 → 1_000_000`). The map is monotonic, and a
/// degenerate NaN input scores `0` so it ranks last deterministically. Both
/// store implementations score through this one function, keeping their
/// semantic rankings identical.
#[must_use]
pub fn similarity_score(dot: f32) -> u32 {
    if dot.is_nan() {
        return 0;
    }
    let clamped = f64::from(dot).clamp(-1.0, 1.0);
    // The clamped value lands in 0.0..=1_000_000.0 after scaling, so the
    // narrowing conversion cannot truncate or lose sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let score = ((clamped + 1.0) * 500_000.0).round() as u32;
    score
}

pub(crate) fn validate_embedder_id(embedder_id: &str) -> Result<(), MemoryStoreError> {
    if embedder_id.is_empty()
        || embedder_id.len() > MEMORY_EMBEDDER_ID_MAX_BYTES
        || embedder_id.as_bytes().contains(&0)
    {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedder_id_invalid",
        });
    }
    Ok(())
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

pub(crate) fn validate_record(record: &MemoryRecord) -> Result<(), MemoryStoreError> {
    record
        .validate()
        .map_err(|error| MemoryStoreError::InvalidRecord {
            reason: match error {
                MemoryError::InvalidRecord { reason } | MemoryError::Configuration { reason } => {
                    reason
                }
            },
        })
}

pub(crate) fn validate_scope(scope: &MemoryScope) -> Result<(), MemoryStoreError> {
    scope
        .validate()
        .map_err(|_| MemoryStoreError::InvalidRequest {
            reason: "memory_scope_invalid",
        })
}

pub(crate) fn validate_idempotency_key(key: &str) -> Result<(), MemoryStoreError> {
    if key.is_empty() || key.len() > MEMORY_IDEMPOTENCY_KEY_MAX_BYTES || key.as_bytes().contains(&0)
    {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_idempotency_key_invalid",
        });
    }
    Ok(())
}

pub(crate) fn validate_query(
    query: &MemoryQuery,
    limit: usize,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    if limit > limits.max_search_results {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_search_limit_exceeded",
        });
    }
    match query {
        MemoryQuery::ExactId(_) => Ok(()),
        MemoryQuery::Keywords(keywords) => {
            if keywords.is_empty()
                || keywords.len() > KEYWORDS_MAX_COUNT
                || keywords.iter().any(|keyword| {
                    keyword.is_empty()
                        || keyword.len() > KEYWORD_MAX_BYTES
                        || keyword.as_bytes().contains(&0)
                })
            {
                return Err(MemoryStoreError::InvalidRequest {
                    reason: "memory_query_keywords_invalid",
                });
            }
            Ok(())
        }
        MemoryQuery::FullText(text) => {
            if text.len() > INLINE_BODY_MAX_BYTES || text.as_bytes().contains(&0) {
                return Err(MemoryStoreError::InvalidRequest {
                    reason: "memory_query_text_invalid",
                });
            }
            Ok(())
        }
        MemoryQuery::Embedding {
            embedder_id,
            vector,
        } => {
            validate_embedder_id(embedder_id)?;
            if vector.dimensions() > limits.max_embedding_dimensions {
                return Err(MemoryStoreError::InvalidRequest {
                    reason: "memory_embedding_dimensions_exceeded",
                });
            }
            Ok(())
        }
    }
}

/// Canonical digest of one mutation (`operation` plus its payload), stored
/// with the idempotency receipt so a replayed key can be told apart from a
/// reused one.
pub(crate) fn operation_fingerprint<T: serde::Serialize>(
    operation: &'static str,
    payload: &T,
) -> Result<Digest, MemoryStoreError> {
    let encoded = serde_json_canonicalizer::to_vec(&(operation, payload)).map_err(|_| {
        MemoryStoreError::InvalidRequest {
            reason: "memory_idempotency_payload_invalid",
        }
    })?;
    Digest::domain_separated("memory-idempotency", 1, &encoded).map_err(|_| {
        MemoryStoreError::InvalidRequest {
            reason: "memory_idempotency_payload_invalid",
        }
    })
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
