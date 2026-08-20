//! Rule-based candidate-memory extraction from run events.
//!
//! Extraction is pure and synchronous: it only reads event bodies and never
//! touches the store. The [`MemoryObserver`](crate::observer::MemoryObserver)
//! is what turns an extractor's output into persisted [`MemoryRecord`]s.
//!
//! Runtime `RunEvent` bodies do not carry arbitrary free text on
//! durable/terminal kinds (`RunCompleted`, `MessageFinalized`, ...) — those
//! only carry ids/digests. The one body that carries model-authored text is
//! [`finstack_ai_kernel::RunEventBody::ModelTextDelta`], so
//! [`RuleBasedExtractor`] scans that body kind rather than a "terminal" kind.

use std::sync::Arc;

use finstack_ai_kernel::{RunEvent, RunEventBody, Sensitivity};

use crate::record::MemoryId;

/// Default marker line prefix recognized by [`RuleBasedExtractor`].
pub const DEFAULT_MARKER: &str = "[[remember]]";

/// A candidate memory extracted from run events, not yet persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateMemory {
    /// Explicit identity for this candidate, when the extractor can assign
    /// one deterministically. When `None`, the caller derives an id (e.g.
    /// from a content digest).
    pub id: Option<MemoryId>,
    /// Search/filter keywords for the candidate.
    pub keywords: Vec<Arc<str>>,
    /// Candidate memory content.
    pub body: Arc<str>,
    /// Sensitivity classification for the candidate.
    pub sensitivity: Sensitivity,
    /// Extraction confidence, `0..=100`.
    pub confidence: u8,
    /// Run that produced this candidate, if known.
    pub source_run: Option<Arc<str>>,
    /// Opaque reference to the originating event, if known.
    pub source_ref: Option<Arc<str>>,
}

/// Pure, synchronous extraction of candidate memories from run events.
pub trait MemoryExtractor: Send + Sync {
    /// Scan `events` and return zero or more candidate memories.
    fn extract(&self, events: &[RunEvent]) -> Vec<CandidateMemory>;
}

/// Extractor that recognizes marker-prefixed lines in model text output.
///
/// Each line in a scanned event's text that begins with the configured
/// marker yields one [`CandidateMemory`] whose body is the line with the
/// marker (and any immediately following whitespace) stripped, whose
/// keywords are the first five whitespace-separated tokens of that body
/// lowercased, and whose confidence is a fixed `60`.
#[derive(Debug, Clone)]
pub struct RuleBasedExtractor {
    marker: Arc<str>,
}

impl RuleBasedExtractor {
    /// Construct an extractor recognizing lines that start with `marker`.
    #[must_use]
    pub fn new(marker: Arc<str>) -> Self {
        Self { marker }
    }
}

impl Default for RuleBasedExtractor {
    fn default() -> Self {
        Self::new(Arc::from(DEFAULT_MARKER))
    }
}

impl MemoryExtractor for RuleBasedExtractor {
    fn extract(&self, events: &[RunEvent]) -> Vec<CandidateMemory> {
        let mut candidates = Vec::new();
        for event in events {
            let RunEventBody::ModelTextDelta(delta) = event.body() else {
                continue;
            };
            let source_run: Arc<str> = Arc::from(event.run_id().to_string());
            let source_ref: Arc<str> = Arc::from(event.event_id().to_string());
            for line in delta.text().lines() {
                let Some(rest) = line.strip_prefix(self.marker.as_ref()) else {
                    continue;
                };
                let body = rest.trim_start();
                if body.is_empty() {
                    continue;
                }
                let keywords = body
                    .split_whitespace()
                    .take(5)
                    .map(|token| Arc::<str>::from(token.to_ascii_lowercase()))
                    .collect();
                candidates.push(CandidateMemory {
                    id: None,
                    keywords,
                    body: Arc::from(body),
                    sensitivity: event.sensitivity(),
                    confidence: 60,
                    source_run: Some(Arc::clone(&source_run)),
                    source_ref: Some(Arc::clone(&source_ref)),
                });
            }
        }
        candidates
    }
}
