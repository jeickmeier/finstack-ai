//! Storage-independent, scoped retrieval contracts and deterministic fusion.
//!
//! [`SearchSource`] is an extension contract, not a new runtime port. Sources
//! retain their systems of record and authorize every direct call. A facade
//! must validate the query and call [`SearchSource::authorize`] for **all**
//! selected sources before starting any leg. Scope is host-supplied authority;
//! query text and retrieved evidence are always untrusted data.
#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![doc(test(attr(allow(clippy::expect_used))))]

mod catalog;
mod evidence;
mod fusion;
mod hit;
mod lexical;
mod query;
mod scope;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use catalog::{SourceReferencePage, validate_catalog_request};
pub use evidence::{MAX_EVIDENCE_BYTES, SearchEvidence};
pub use fusion::{Fusion, HybridLeg, HybridPlan, LegResult, fuse};
pub use hit::{
    FusedEvidence, FusedHit, SearchCitation, SearchHit, SearchProvenance, SearchResponse,
    SourceOutcome, SourceRef, SourceResult, SourceStatus, highest_sensitivity, preview,
};
pub use lexical::{MAX_LEXICAL_TERMS, fts_expression, lexical_terms};
pub use query::{
    GraphQuery, JournalFilter, LexicalKind, SearchLimits, SearchQuery, SearchStrategy,
};
pub use scope::{ScopeMapping, SearchScope};

use std::sync::Arc;

use finstack_ai_runtime::ports::{PortFuture, PortObject};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Invalid query, configuration, or source result.
pub const SEARCH_INVALID: &str = "search_invalid";

/// Requested scope is outside host-authorized bounds.
pub const SEARCH_SCOPE_DENIED: &str = "search_scope_denied";

/// A source or derived index is unavailable.
pub const SEARCH_UNAVAILABLE: &str = "search_unavailable";

/// The source does not support the requested strategy.
pub const SEARCH_UNSUPPORTED: &str = "search_unsupported";

/// A configured resource capacity was exceeded.
pub const SEARCH_CAPACITY_EXCEEDED: &str = "search_capacity_exceeded";

/// No selected source completed even partially.
pub const SEARCH_NO_SUCCESSFUL_SOURCES: &str = "search_no_successful_sources";

/// Stable, source-free search failures. Never contains backend diagnostics or
/// query content; sources may retain those separately in trusted host logging.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SearchError {
    /// Invalid bounded query, scope, plan, configuration, or source result.
    #[error("search_invalid: {reason}")]
    SearchInvalid {
        /// Stable non-secret validation reason.
        reason: Arc<str>,
    },
    /// Requested scope or session is not authorized by the bound source.
    #[error("search_scope_denied")]
    SearchScopeDenied,
    /// A source or derived index is unavailable.
    #[error("search_unavailable")]
    SearchUnavailable,
    /// Source cannot implement the requested strategy or filters.
    #[error("search_unsupported")]
    SearchUnsupported,
    /// An explicit retained resource limit would be exceeded.
    #[error("search_capacity_exceeded: {resource}")]
    SearchCapacityExceeded {
        /// Stable resource name, never a path or query.
        resource: Arc<str>,
    },
    /// No leg completed even partially. The attempted outcomes remain visible.
    #[error("search_no_successful_sources")]
    SearchNoSuccessfulSources {
        /// Outcome of every selected leg.
        outcomes: Vec<SourceOutcome>,
    },
}

impl SearchError {
    /// Construct a source-free validation failure from a static reason.
    #[must_use]
    pub fn invalid(reason: &'static str) -> Self {
        Self::SearchInvalid {
            reason: Arc::from(reason),
        }
    }

    /// Stable error code shared by native and binding APIs.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SearchInvalid { .. } => SEARCH_INVALID,
            Self::SearchScopeDenied => SEARCH_SCOPE_DENIED,
            Self::SearchUnavailable => SEARCH_UNAVAILABLE,
            Self::SearchUnsupported => SEARCH_UNSUPPORTED,
            Self::SearchCapacityExceeded { .. } => SEARCH_CAPACITY_EXCEEDED,
            Self::SearchNoSuccessfulSources { .. } => SEARCH_NO_SUCCESSFUL_SOURCES,
        }
    }
}

/// Stable identity, strategy declarations, exact scope policy, and limits of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSourceDescriptor {
    /// Namespace in deduplication and citations. Unique within a federation.
    pub source_id: Arc<str>,
    /// Human-readable source class: memory, documents, journal, graph, or a
    /// backend-defined extension name. Not used to infer authority.
    pub kind: Arc<str>,
    /// Supported lexical kinds. Semantic space and graph support are separate.
    pub lexical: Vec<LexicalKind>,
    /// Explicitly configured semantic spaces; empty means no embedding calls.
    pub semantic_spaces: Vec<Arc<str>>,
    /// Whether exact reference reads support derived-evidence revalidation.
    pub evidence_lookup: bool,
    /// Whether bounded graph queries are implemented.
    pub graph: bool,
    /// Whether source/index state survives process termination.
    pub durable: bool,
    /// Declarative mapping used by this adapter on every request.
    pub scope_mapping: ScopeMapping,
    /// Effective query, scan, preview, and traversal ceilings.
    pub limits: SearchLimits,
    /// Fingerprint of source identity, scope, backend configuration, and
    /// vocabulary/chunker versions, suitable for resolved component locks.
    pub configuration_digest: finstack_ai_kernel::Digest,
}

impl SearchSourceDescriptor {
    /// Whether this source implements a strategy (not a guarantee of availability).
    #[must_use]
    pub fn supports(&self, strategy: &SearchStrategy) -> bool {
        match strategy {
            SearchStrategy::Lexical(kind) => self.lexical.contains(kind),
            SearchStrategy::Semantic { space } => self.semantic_spaces.contains(space),
            SearchStrategy::Graph(_) => self.graph,
        }
    }
}

/// Replaceable source storage and retrieval boundary. Implementations are
/// trusted native/host extensions; index contents confer no authority.
pub trait SearchSource: PortObject {
    /// Immutable declaration used for composition and strategy selection.
    fn descriptor(&self) -> SearchSourceDescriptor;

    /// Validate scope and requested session/lane filters without I/O. Must
    /// reject unauthorized input even if a strategy is unsupported.
    ///
    /// # Errors
    /// Returns [`SearchError::SearchScopeDenied`] on any authority mismatch.
    fn authorize(&self, scope: &SearchScope, query: &SearchQuery) -> Result<(), SearchError>;

    /// Page authorized source references for explicit host graph indexing.
    /// These metadata candidates may be stale; exact evidence lookup is required
    /// before extraction. Sources without a catalog report unsupported. No global
    /// session discovery is implied; journal catalogs honor their bound session list.
    fn references(
        &self,
        _scope: SearchScope,
        _cursor: Option<String>,
        _limit: usize,
    ) -> PortFuture<Result<SourceReferencePage, SearchError>> {
        Box::pin(async { Err(SearchError::SearchUnsupported) })
    }

    /// Perform one bounded source leg. Direct callers receive the same query
    /// and authority checks as a federation. The result records truncation and
    /// incomplete historical coverage; an unavailable index is never empty success.
    fn search(
        &self,
        scope: SearchScope,
        query: SearchQuery,
        limit: usize,
    ) -> PortFuture<Result<SourceResult, SearchError>>;

    /// Read the current authoritative text for one exact reference. `None`
    /// means deleted, corrected away, or no longer retained. Unavailable storage
    /// returns an error, never `None`. Implementations repeat scope authorization.
    /// The default is explicitly unsupported; graph compositions require the
    /// descriptor's `evidence_lookup` capability before accepting a source.
    fn read_evidence(
        &self,
        _scope: SearchScope,
        _reference: SourceRef,
    ) -> PortFuture<Result<Option<SearchEvidence>, SearchError>> {
        Box::pin(async { Err(SearchError::SearchUnsupported) })
    }
}

/// Canonical, versioned fingerprint helper for source-owned configuration.
///
/// # Errors
/// Returns a validation error for invalid serialization or digest input.
pub fn configuration_digest<T: Serialize>(
    domain: &str,
    value: &T,
) -> Result<finstack_ai_kernel::Digest, SearchError> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| SearchError::invalid("configuration_encoding"))?;
    finstack_ai_kernel::Digest::domain_separated(domain, 1, &bytes)
        .map_err(|_| SearchError::invalid("configuration_digest"))
}

/// Validate a bounded non-empty non-secret source/space/scope identifier.
///
/// # Errors
/// Rejects whitespace-only, NUL-bearing, or overlong identifiers.
pub fn validate_id(value: &str) -> Result<(), SearchError> {
    if value.trim().is_empty() || value.len() > 256 || value.as_bytes().contains(&0) {
        return Err(SearchError::invalid("identifier"));
    }
    Ok(())
}
