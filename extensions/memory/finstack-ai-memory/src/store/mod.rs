//! `MemoryStore` port: the trait memory-backed extensions implement, plus
//! query/result types and an in-process reference implementation.
//!
//! Nothing here is opinionated about persistence; [`InProcessMemoryStore`]
//! is a reference/test implementation, and the `sqlite` feature (a later
//! task) adds a durable one.

use std::sync::Arc;

use thiserror::Error;

use finstack_ai_runtime::{PortFuture, PortObject};

use crate::record::{MemoryId, MemoryRecord, MemoryScope};

mod in_process;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
mod sqlite;

pub use in_process::{InProcessArtifactStore, InProcessMemoryStore};
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub use sqlite::SqliteMemoryStore;

/// A search query against a [`MemoryStore`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryQuery {
    /// Look up a single record by its exact identifier.
    ExactId(MemoryId),
    /// Match records whose keywords contain any of the given terms
    /// (case-insensitive).
    Keywords(Arc<[Arc<str>]>),
    /// Match records whose preview or inline body contains the given text
    /// as a case-insensitive substring.
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
}

/// Durable memory storage: write, point-read, search, tombstone, supersede,
/// and list memory records.
///
/// Implementations must be object-safe; all operations are asynchronous and
/// scope-checked. Off `wasm32`, implementations must also be safely
/// shareable across threads ([`PortObject`] relaxes this on `wasm32`, where
/// the runtime is single-threaded and JS host adapters are not `Send`).
pub trait MemoryStore: PortObject {
    /// Insert `record`, keyed by `idempotency_key`. Re-applying the same key
    /// is a no-op that reports [`PutOutcome::AlreadyApplied`] rather than
    /// erroring or double-writing.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::InvalidRecord`] when `record` fails
    /// validation, or [`MemoryStoreError::Unavailable`] on a backend
    /// failure.
    fn put(
        &self,
        idempotency_key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>>;

    /// Look up one record by identifier, scoped to `scope`.
    ///
    /// Returns `Ok(None)` when no record exists for `id`, or when one
    /// exists but `scope` does not permit access to it.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] on a backend failure.
    fn get(
        &self,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>>;

    /// Search for records visible to `scope` matching `query`, returning at
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
    /// Returns [`MemoryStoreError::NotFound`] when no such record is
    /// visible to `scope`, or [`MemoryStoreError::Unavailable`] on a
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
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::NotFound`] when `old` is not visible to
    /// `scope`, [`MemoryStoreError::InvalidRecord`] when `replacement`
    /// fails validation, or [`MemoryStoreError::Unavailable`] on a backend
    /// failure.
    fn correct(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        replacement: MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>>;

    /// List records visible to `scope`, ordered by identifier, paged by
    /// `page`. Includes tombstoned and superseded records (for
    /// inspection/management).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] on a backend failure.
    fn list(
        &self,
        scope: MemoryScope,
        page: MemoryPage,
    ) -> PortFuture<Result<MemoryListing, MemoryStoreError>>;
}
