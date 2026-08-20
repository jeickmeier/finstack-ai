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

use std::collections::HashMap;
use std::sync::Arc;

use finstack_ai_kernel::{ModelRequestId, RunEvent, RunEventBody, RunId, Sensitivity};

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
///
/// `ModelTextDelta` events are raw streaming chunks: a marker line can be
/// split across two or more deltas (e.g. `"[[remember]] the user pre"` then
/// `"fers dark mode\n"`), and scanning each event's text independently would
/// silently miss it. To avoid that, `extract` first accumulates delta text
/// **in arrival order, grouped by `(run_id, model_request_id)`**, and only
/// then splits the concatenated per-group text into lines and scans those.
/// Events with a different `model_request_id` (or none) never contribute to
/// the same group, even if they share a `run_id` — a new model turn starts
/// a new, unrelated text stream. Grouping lives here (inside the extractor)
/// rather than in `MemoryObserver`, so the `MemoryExtractor::extract`
/// signature stays a plain `&[RunEvent] -> Vec<CandidateMemory>` with no
/// batching contract leaking into the trait.
///
/// Each resulting candidate's `source_ref` is the **first** contributing
/// delta event's id for its group (not the id of whichever event happened
/// to complete a marker line) — that keeps the `MemoryObserver`-assigned
/// idempotency key (`capture:{run_id}:{event_id}:{candidate_index}`)
/// deterministic across literal batch redelivery, since replays repeat the
/// same event ids in the same order. Similarly, `sensitivity` for a group's
/// candidates is the sensitivity of that first event; every event in a
/// well-formed `ModelTextDelta` stream for one model request has the same
/// sensitivity in practice (see the kernel's model-event validation), so
/// this is not expected to lose information.
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

/// Accumulated per-`(run_id, model_request_id)` text-delta stream.
struct DeltaGroup {
    source_run: Arc<str>,
    source_ref: Arc<str>,
    sensitivity: Sensitivity,
    text: String,
}

impl MemoryExtractor for RuleBasedExtractor {
    fn extract(&self, events: &[RunEvent]) -> Vec<CandidateMemory> {
        let mut order: Vec<(RunId, Option<ModelRequestId>)> = Vec::new();
        let mut groups: HashMap<(RunId, Option<ModelRequestId>), DeltaGroup> = HashMap::new();

        for event in events {
            let RunEventBody::ModelTextDelta(delta) = event.body() else {
                continue;
            };
            let key = (event.run_id(), event.model_request_id());
            let group = groups.entry(key).or_insert_with(|| {
                order.push(key);
                DeltaGroup {
                    source_run: Arc::from(event.run_id().to_string()),
                    source_ref: Arc::from(event.event_id().to_string()),
                    sensitivity: event.sensitivity(),
                    text: String::new(),
                }
            });
            group.text.push_str(delta.text());
        }

        let mut candidates = Vec::new();
        for key in order {
            let Some(group) = groups.remove(&key) else {
                continue;
            };
            for line in group.text.lines() {
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
                    sensitivity: group.sensitivity,
                    confidence: 60,
                    source_run: Some(Arc::clone(&group.source_run)),
                    source_ref: Some(Arc::clone(&group.source_ref)),
                });
            }
        }
        candidates
    }
}
