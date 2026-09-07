use std::sync::Arc;

use finstack_ai_kernel::{ArtifactRef, Digest, EntryId, LaneId, RecordId, Sensitivity, SessionId};
use serde::{Deserialize, Serialize};

use crate::{SearchError, SearchLimits, SearchScope, SearchStrategy, validate_id};

/// Typed stable reference. Citations use committed identifiers; journal token
/// events are never references. The source namespace remains part of identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SourceRef {
    /// A live memory record within its complete bound scope.
    Memory {
        /// Store-owned memory identifier.
        id: Arc<str>,
    },
    /// A deterministic chunk of an immutable artifact.
    ArtifactChunk {
        /// Exact scoped artifact reference, including content integrity.
        artifact: Box<ArtifactRef>,
        /// Zero-based chunk ordinal under the named chunking configuration.
        ordinal: u32,
        /// Versioned chunker configuration fingerprint.
        chunker: Digest,
    },
    /// A committed conversation entry and its optional retained journal span.
    JournalSpan {
        /// Authorized source session.
        session: SessionId,
        /// Committed source lane.
        lane: LaneId,
        /// Committed conversation entry identifier.
        entry: EntryId,
        /// First committed record when retained; absent after snapshot recovery.
        first_record: Option<RecordId>,
        /// Last committed record when retained; absent after snapshot recovery.
        last_record: Option<RecordId>,
    },
    /// A scoped entity in a configured property graph.
    Entity {
        /// Graph-owned deterministic identifier.
        id: Arc<str>,
    },
}

impl SourceRef {
    /// Canonical reference key. Combine with source namespace for deduplication.
    ///
    /// # Errors
    /// Rejects invalid identifiers, malformed spans, or oversized references.
    pub fn key(&self) -> Result<String, SearchError> {
        match self {
            Self::Memory { id } | Self::Entity { id } => validate_id(id)?,
            Self::JournalSpan {
                first_record,
                last_record,
                ..
            } if first_record.is_some() != last_record.is_some() => {
                return Err(SearchError::invalid("journal_span"));
            }
            _ => {}
        }
        let encoded = serde_json_canonicalizer::to_string(self)
            .map_err(|_| SearchError::invalid("reference_encoding"))?;
        if encoded.len() > 8192 {
            return Err(SearchError::invalid("reference_limit"));
        }
        Ok(encoded)
    }
}

/// Scoped source evidence retained by derived graph/document results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchCitation {
    /// Namespace that owns the cited system of record.
    pub source: Arc<str>,
    /// Exact referenced memory, artifact chunk, committed entry, or entity.
    pub reference: SourceRef,
    /// Complete authority scope, never merged across scopes.
    pub scope: SearchScope,
    /// Source-content fingerprint guarding correction/deletion reconciliation.
    pub content_digest: Digest,
}

/// Content provenance independent of relevance score and retrieval strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchProvenance {
    /// Exact scope in which the source evidence exists.
    pub scope: SearchScope,
    /// Fingerprint of authoritative source text, not of a preview.
    pub content_digest: Digest,
    /// Bounded citation locators such as heading, page, character offsets, or
    /// original memory provenance. These are data and confer no authority.
    pub locators: Vec<Arc<str>>,
    /// Original scoped evidence supporting a derived result (not recursive).
    pub citations: Vec<SearchCitation>,
}

/// One ranked source match. Scores order one leg only; fusion normalizes or
/// uses ranks and never compares raw scores between heterogeneous sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchHit {
    /// Source namespace, also part of the deduplication key.
    pub source: Arc<str>,
    /// Typed citation.
    pub reference: SourceRef,
    /// Source-owned nonnegative relevance order, higher first.
    pub score: u32,
    /// Bounded Unicode preview; retrieved content remains untrusted.
    pub preview: Arc<str>,
    /// Canonical entity label for graph matches and bounded query expansion.
    /// Present only for entity references; never inferred by parsing previews.
    #[serde(default)]
    pub entity_label: Option<Arc<str>>,
    /// Highest source-data classification represented by the result.
    pub sensitivity: Sensitivity,
    /// Exact source provenance, independent of fusion evidence.
    pub provenance: SearchProvenance,
}

impl SearchHit {
    /// Validate bounded evidence and scope preservation before retaining it.
    ///
    /// # Errors
    /// Rejects malformed/oversized results or citations from a different scope.
    pub fn validate(&self, limits: &SearchLimits) -> Result<(), SearchError> {
        validate_id(&self.source)?;
        self.reference.key()?;
        self.provenance.scope.validate()?;
        if self.preview.chars().count() > limits.max_preview_chars
            || self.preview.as_bytes().contains(&0)
            || self.provenance.locators.len() > 16
            || self.provenance.citations.len() > 16
        {
            return Err(SearchError::invalid("hit_bounds"));
        }
        if let Some(label) = &self.entity_label
            && (!matches!(self.reference, SourceRef::Entity { .. })
                || label.trim().is_empty()
                || label.len() > 256
                || label.contains('\0'))
        {
            return Err(SearchError::invalid("entity_label"));
        }
        for locator in &self.provenance.locators {
            if locator.len() > 1024 || locator.as_bytes().contains(&0) {
                return Err(SearchError::invalid("locator_bounds"));
            }
        }
        for citation in &self.provenance.citations {
            validate_id(&citation.source)?;
            citation.reference.key()?;
            if citation.scope != self.provenance.scope {
                return Err(SearchError::SearchScopeDenied);
            }
        }
        let bytes = serde_json::to_vec(self).map_err(|_| SearchError::invalid("hit_encoding"))?;
        if bytes.len() > 32_768 {
            return Err(SearchError::invalid("hit_bytes"));
        }
        Ok(())
    }
}

/// Observable outcome of a selected source/strategy leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// Query completed with full available coverage, including a valid empty match.
    Completed,
    /// Source does not implement the requested strategy/filter.
    Unsupported,
    /// Source/index/embedding is missing, failed, or timed out.
    Unavailable,
    /// Some results/coverage are available, but a scan/result/history/index bound
    /// was reached. This is partial success and must remain visible to callers.
    Truncated,
}

impl SourceStatus {
    /// Whether this leg retrieved from at least a usable portion of its source.
    #[must_use]
    pub const fn successful(self) -> bool {
        matches!(self, Self::Completed | Self::Truncated)
    }
}

/// Source-local query outcome, with explicit historical and resource coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceResult {
    /// Ranked matches, bounded to the caller's requested result limit.
    pub hits: Vec<SearchHit>,
    /// Completion/partial/unavailable/unsupported state.
    pub status: SourceStatus,
    /// Number of source rows examined (not merely number of hits returned).
    pub examined: u64,
    /// Whether the available authoritative history was fully represented.
    pub historical_complete: bool,
    /// Stable bounded reason codes for missing coverage or index state.
    pub reasons: Vec<Arc<str>>,
}

impl SourceResult {
    /// Construct a fully covered result. Sources change status/reasons when
    /// truncation, missing index coverage, or snapshot limitations apply.
    #[must_use]
    pub fn completed(hits: Vec<SearchHit>, examined: u64) -> Self {
        Self {
            hits,
            status: SourceStatus::Completed,
            examined,
            historical_complete: true,
            reasons: Vec::new(),
        }
    }

    /// Mark known incomplete coverage without losing available hits.
    pub fn truncate(&mut self, reason: &'static str) {
        self.status = SourceStatus::Truncated;
        let reason: Arc<str> = Arc::from(reason);
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }
}

/// Per-leg coverage returned even when a leg contributes no hits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOutcome {
    /// Requested source namespace (even if absent from the registry).
    pub source: Arc<str>,
    /// Requested strategy.
    pub strategy: SearchStrategy,
    /// Completed, unsupported, unavailable, or truncated.
    pub status: SourceStatus,
    /// Source rows examined, if the source ran.
    pub examined: u64,
    /// Explicit historical coverage.
    pub historical_complete: bool,
    /// Stable coverage/failure reason codes; never backend exception messages.
    pub reasons: Vec<Arc<str>>,
}

/// One leg's evidence contributing to a fused hit. Keeping provenance here
/// preserves every leg even when the representative preview changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusedEvidence {
    /// Leg strategy.
    pub strategy: SearchStrategy,
    /// One-based rank within this leg, after duplicate-reference removal.
    pub rank: u32,
    /// Raw per-leg ordering value, not comparable to other legs.
    pub source_score: u32,
    /// Configured dimensionless leg weight in millionths.
    pub weight_micros: u32,
    /// Contribution to the fused fixed-point score (scale one billion).
    pub contribution: u64,
    /// Unmodified provenance from this contributing leg.
    pub provenance: SearchProvenance,
    /// This leg's data classification.
    pub sensitivity: Sensitivity,
}

/// Deduplicated result carrying all contributing evidence and highest sensitivity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusedHit {
    /// Source namespace.
    pub source: Arc<str>,
    /// Stable typed reference.
    pub reference: SourceRef,
    /// Fused relevance in billionths, bounded below the JSON exact-integer limit.
    pub score: u64,
    /// Deterministically selected bounded preview.
    pub preview: Arc<str>,
    /// Canonical graph entity label, when provided by the source.
    #[serde(default)]
    pub entity_label: Option<Arc<str>>,
    /// Highest classification across every contributing leg.
    pub sensitivity: Sensitivity,
    /// Full per-leg evidence and provenance.
    pub evidence: Vec<FusedEvidence>,
}

/// Search results and complete source outcomes. Empty hits are a successful
/// answer only when at least one source completed wholly or partially.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResponse {
    /// Deterministically fused, bounded final results.
    pub hits: Vec<FusedHit>,
    /// Outcomes of every selected leg, including failed and unsupported legs.
    pub outcomes: Vec<SourceOutcome>,
    /// Whether final top-k retention omitted other fused matches.
    pub results_truncated: bool,
}

/// Preserve the more restrictive of two classifications.
#[must_use]
pub const fn highest_sensitivity(left: Sensitivity, right: Sensitivity) -> Sensitivity {
    const fn rank(value: Sensitivity) -> u8 {
        match value {
            Sensitivity::Public => 0,
            Sensitivity::Internal => 1,
            Sensitivity::Confidential => 2,
            Sensitivity::Secret => 3,
            Sensitivity::Credential => 4,
        }
    }
    if rank(left) >= rank(right) {
        left
    } else {
        right
    }
}

/// Truncate on Unicode scalar boundaries, replacing NULs so previews can enter
/// normalized text blocks. Source digests always cover the original text.
#[must_use]
pub fn preview(text: &str, max_chars: usize) -> Arc<str> {
    text.chars()
        .take(max_chars)
        .map(|c| if c == '\0' { '\u{fffd}' } else { c })
        .collect::<String>()
        .into()
}
