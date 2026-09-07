use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{LaneId, SessionId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{SearchError, validate_id};

/// Offline lexical strategy, with source support declared explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LexicalKind {
    /// Token/keyword matching.
    Keyword,
    /// Full-text ranking, BM25 where the source implements it.
    Bm25,
    /// Literal case-sensitive substring search.
    Literal,
    /// Bounded Rust regex matching.
    Regex,
}

/// Bounded property-graph operation. Labels and identifiers are data, never SQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphQuery {
    /// Exact configured label/alias lookup using the query text.
    Entity,
    /// Breadth-first neighborhood of the matched entities, with hard ceilings.
    Neighborhood {
        /// Maximum edge distance; the default facade expansion uses two.
        depth: u8,
        /// Maximum visited entities (including seeds).
        max_nodes: usize,
        /// Maximum examined edges, including cycle/back edges.
        max_edges: usize,
    },
    /// Shortest bounded path from query text to a target entity label/alias.
    Path {
        /// Exact destination label/alias.
        target: Arc<str>,
        /// Maximum edge distance.
        depth: u8,
        /// Maximum visited entities.
        max_nodes: usize,
        /// Maximum examined edges.
        max_edges: usize,
    },
}

/// One source leg's strategy; hybrid behavior is expressed by a `HybridPlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "config", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SearchStrategy {
    /// Offline lexical retrieval.
    Lexical(LexicalKind),
    /// Exact-vector retrieval in an explicitly configured embedding space.
    Semantic {
        /// Stable embedder identity, including model revision/dimensionality.
        space: Arc<str>,
    },
    /// Configured property graph query.
    Graph(GraphQuery),
}

/// Optional journal-only filters. Other sources must report unsupported for a
/// non-empty filter; silently ignoring it could broaden the intended query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalFilter {
    /// Subset of the journal adapter's explicit authorized sessions.
    #[serde(default)]
    pub sessions: Vec<SessionId>,
    /// Optional lane restriction within those sessions.
    #[serde(default)]
    pub lanes: Vec<LaneId>,
    /// Inclusive committed-record timestamp lower bound.
    pub after: Option<Timestamp>,
    /// Inclusive committed-record timestamp upper bound.
    pub before: Option<Timestamp>,
}

impl JournalFilter {
    /// Whether no filters are requested.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
            && self.lanes.is_empty()
            && self.after.is_none()
            && self.before.is_none()
    }
}

/// Explicit resource ceilings. Defaults cover 100,000 document chunks and
/// 10,000 memory records, while retaining at most 256 results per leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchLimits {
    /// Maximum query UTF-8 bytes.
    pub max_query_bytes: usize,
    /// Maximum regex pattern bytes (also subject to compiled-regex size limits).
    pub max_pattern_bytes: usize,
    /// Maximum records scanned per source call.
    pub max_scan_records: usize,
    /// Maximum retained hits per leg or final response.
    pub max_results: usize,
    /// Maximum Unicode scalar values in each preview.
    pub max_preview_chars: usize,
    /// Maximum legs in one hybrid plan.
    pub max_legs: usize,
    /// Maximum aggregate stored document embedding bytes. Memory adapters also
    /// admit semantic scans/rebuilds against live records times vector bytes;
    /// their underlying store enforces its own aggregate retained-byte ceiling.
    pub max_embedding_bytes: u64,
    /// Maximum entities visited by a graph query.
    pub max_graph_nodes: usize,
    /// Maximum edges examined by a graph query.
    pub max_graph_edges: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_query_bytes: 4096,
            max_pattern_bytes: 1024,
            max_scan_records: 100_000,
            max_results: 256,
            max_preview_chars: 512,
            max_legs: 32,
            max_embedding_bytes: 256 * 1024 * 1024,
            max_graph_nodes: 100,
            max_graph_edges: 1000,
        }
    }
}

impl SearchLimits {
    /// Check nonzero finite limits against the supported operating envelope.
    ///
    /// # Errors
    /// Rejects unbounded or unsupported resource configurations.
    pub fn validate(&self) -> Result<(), SearchError> {
        if self.max_query_bytes == 0
            || self.max_query_bytes > 16_384
            || self.max_pattern_bytes == 0
            || self.max_pattern_bytes > 4096
            || self.max_pattern_bytes > self.max_query_bytes
            || self.max_scan_records == 0
            || self.max_scan_records > 100_000
            || self.max_results == 0
            || self.max_results > 256
            || self.max_preview_chars == 0
            || self.max_preview_chars > 2048
            || self.max_legs == 0
            || self.max_legs > 32
            || self.max_embedding_bytes == 0
            || self.max_embedding_bytes > 1024 * 1024 * 1024
            || self.max_graph_nodes == 0
            || self.max_graph_nodes > 10_000
            || self.max_graph_edges == 0
            || self.max_graph_edges > 100_000
        {
            return Err(SearchError::invalid("limits"));
        }
        Ok(())
    }
}

/// Untrusted query data for one source call. Authority arrives separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchQuery {
    /// Search text, literal, regex, or graph seed label.
    pub text: Arc<str>,
    /// One strategy, selected explicitly by the facade's plan.
    pub strategy: SearchStrategy,
    /// Optional journal restrictions; never grants access to a new session.
    #[serde(default)]
    pub journal: JournalFilter,
}

impl SearchQuery {
    /// Construct an offline keyword query with no journal filters.
    #[must_use]
    pub fn lexical(text: impl Into<Arc<str>>, kind: LexicalKind) -> Self {
        Self {
            text: text.into(),
            strategy: SearchStrategy::Lexical(kind),
            journal: JournalFilter::default(),
        }
    }

    /// Validate everything possible before source fan-out, including regex
    /// compilation and graph limits. Sources repeat this for direct calls.
    ///
    /// # Errors
    /// Rejects malformed or excessive inputs before any embedding or store I/O.
    pub fn validate(&self, limits: &SearchLimits, limit: usize) -> Result<(), SearchError> {
        limits.validate()?;
        if self.text.trim().is_empty()
            || self.text.len() > limits.max_query_bytes
            || self.text.as_bytes().contains(&0)
            || limit == 0
            || limit > limits.max_results
        {
            return Err(SearchError::invalid("query_or_result_limit"));
        }
        if self.journal.sessions.len() > 256
            || self.journal.lanes.len() > 256
            || self.journal.sessions.iter().collect::<BTreeSet<_>>().len()
                != self.journal.sessions.len()
            || self.journal.lanes.iter().collect::<BTreeSet<_>>().len() != self.journal.lanes.len()
            || self
                .journal
                .after
                .zip(self.journal.before)
                .is_some_and(|(a, b)| a > b)
        {
            return Err(SearchError::invalid("journal_filter"));
        }
        match &self.strategy {
            SearchStrategy::Lexical(LexicalKind::Regex) => {
                self.regex(limits)?;
            }
            SearchStrategy::Semantic { space } => validate_id(space)?,
            SearchStrategy::Graph(
                GraphQuery::Neighborhood {
                    depth,
                    max_nodes,
                    max_edges,
                }
                | GraphQuery::Path {
                    depth,
                    max_nodes,
                    max_edges,
                    ..
                },
            ) => {
                if *depth == 0
                    || *depth > 8
                    || *max_nodes == 0
                    || *max_nodes > limits.max_graph_nodes
                    || *max_edges == 0
                    || *max_edges > limits.max_graph_edges
                {
                    return Err(SearchError::invalid("graph_bounds"));
                }
                if let SearchStrategy::Graph(GraphQuery::Path { target, .. }) = &self.strategy {
                    validate_id(target)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Compile a regex with bounded pattern, DFA, and compiled program sizes.
    ///
    /// # Errors
    /// Rejects invalid/oversized patterns; no search starts on failure.
    pub fn regex(&self, limits: &SearchLimits) -> Result<regex::Regex, SearchError> {
        if self.text.len() > limits.max_pattern_bytes {
            return Err(SearchError::invalid("regex_pattern_limit"));
        }
        regex::RegexBuilder::new(&self.text)
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .build()
            .map_err(|_| SearchError::invalid("regex_pattern"))
    }
}
