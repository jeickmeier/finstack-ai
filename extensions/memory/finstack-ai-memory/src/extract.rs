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
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{EventId, ModelRequestId, RunEvent, RunEventBody, RunId, Sensitivity};

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
/// same event ids in the same order. A group's `sensitivity` is the
/// **highest** classification among the deltas that contributed text to it,
/// so a candidate assembled partly from a more sensitive delta is never
/// stored (and later recalled) under a lower one.
///
/// Delta streams also split across *delivery batches*, not just across
/// events within one batch: `observe` may hand over `"[[remember]] the user
/// pre"` in one batch and `"fers dark mode\n"` in the next. Scanning only
/// complete lines and carrying the trailing partial line forward keeps that
/// case whole. Residual text is therefore retained between `extract` calls,
/// per `(run_id, model_request_id)`, and is released when the run ends, when
/// it exceeds [`MAX_RESIDUAL_BYTES`], or when more than
/// [`MAX_TRACKED_GROUPS`] groups are live. Re-delivery of a batch already
/// seen resets that group's residual first, so a literal replay extracts
/// exactly what the original delivery did.
///
/// Not `Clone`: the retained residual is per-extractor state, and a clone
/// would silently fork a half-assembled line into two streams.
#[derive(Debug)]
pub struct RuleBasedExtractor {
    marker: Arc<str>,
    residual: Mutex<HashMap<(RunId, Option<ModelRequestId>), Residual>>,
}

/// Trailing partial line carried from one `extract` call to the next.
#[derive(Debug, Clone)]
struct Residual {
    /// Text after the last newline seen so far.
    text: String,
    /// Identity the group's candidates are attributed to.
    source_ref: Arc<str>,
    /// Highest sensitivity seen among contributing deltas.
    sensitivity: Sensitivity,
    /// First event of the batch that last extended this residual, used to
    /// recognize a redelivery of that same batch.
    last_batch_head: Option<EventId>,
}

/// Maximum retained partial-line length per group. A marker line longer than
/// this would exceed the inline body ceiling anyway.
const MAX_RESIDUAL_BYTES: usize = 4096;

/// Maximum number of groups whose residual is retained at once.
const MAX_TRACKED_GROUPS: usize = 64;

/// Rank used to compare [`Sensitivity`] values, which are not `Ord`.
const fn sensitivity_rank(sensitivity: Sensitivity) -> u8 {
    match sensitivity {
        Sensitivity::Public => 0,
        Sensitivity::Internal => 1,
        Sensitivity::Confidential => 2,
        Sensitivity::Secret => 3,
        Sensitivity::Credential => 4,
    }
}

fn max_sensitivity(left: Sensitivity, right: Sensitivity) -> Sensitivity {
    if sensitivity_rank(right) > sensitivity_rank(left) {
        right
    } else {
        left
    }
}

impl RuleBasedExtractor {
    /// Construct an extractor recognizing lines that start with `marker`.
    #[must_use]
    pub fn new(marker: Arc<str>) -> Self {
        Self {
            marker,
            residual: Mutex::new(HashMap::new()),
        }
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

impl RuleBasedExtractor {
    /// Push one candidate per marker line found in `text`.
    fn scan_into(
        &self,
        text: &str,
        source_run: &Arc<str>,
        source_ref: &Arc<str>,
        sensitivity: Sensitivity,
        candidates: &mut Vec<CandidateMemory>,
    ) {
        for line in text.lines() {
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
                sensitivity,
                confidence: 60,
                source_run: Some(Arc::clone(source_run)),
                source_ref: Some(Arc::clone(source_ref)),
            });
        }
    }

    /// Scan and release residual text for runs that ended, so a final marker
    /// line without a trailing newline is captured rather than discarded.
    fn flush_ended_runs(
        &self,
        ended_runs: &[RunId],
        residual: &mut HashMap<(RunId, Option<ModelRequestId>), Residual>,
        candidates: &mut Vec<CandidateMemory>,
    ) {
        for run in ended_runs {
            let stale: Vec<(RunId, Option<ModelRequestId>)> = residual
                .keys()
                .filter(|(residual_run, _)| residual_run == run)
                .copied()
                .collect();
            for key in stale {
                let Some(carried) = residual.remove(&key) else {
                    continue;
                };
                let source_run: Arc<str> = Arc::from(key.0.to_string());
                self.scan_into(
                    &carried.text,
                    &source_run,
                    &carried.source_ref,
                    carried.sensitivity,
                    candidates,
                );
            }
        }
    }
}

impl MemoryExtractor for RuleBasedExtractor {
    fn extract(&self, events: &[RunEvent]) -> Vec<CandidateMemory> {
        let mut order: Vec<(RunId, Option<ModelRequestId>)> = Vec::new();
        let mut groups: HashMap<(RunId, Option<ModelRequestId>), DeltaGroup> = HashMap::new();
        let batch_head = events.first().map(RunEvent::event_id);
        // Runs whose streams end in this batch: their residual is scanned and
        // released here, so a final marker line with no trailing newline is
        // still captured instead of being held forever.
        let mut ended_runs: Vec<RunId> = Vec::new();

        for event in events {
            let RunEventBody::ModelTextDelta(delta) = event.body() else {
                if matches!(
                    event.body(),
                    RunEventBody::RunCompleted { .. } | RunEventBody::RunFailed { .. }
                ) {
                    ended_runs.push(event.run_id());
                }
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
            // Highest classification among the contributing deltas, so text
            // from a more sensitive delta is never labeled with the first
            // delta's lower one.
            group.sensitivity = max_sensitivity(group.sensitivity, event.sensitivity());
            group.text.push_str(delta.text());
        }

        let mut residual = match self.residual.lock() {
            Ok(residual) => residual,
            // A poisoned residual map only costs cross-batch continuity;
            // extraction of whole lines in this batch still works.
            Err(poisoned) => poisoned.into_inner(),
        };

        // Prepend any carried partial line, and adopt its identity so the
        // candidate is attributed to where its text began.
        for key in &order {
            let Some(group) = groups.get_mut(key) else {
                continue;
            };
            let Some(carried) = residual.remove(key) else {
                continue;
            };
            // A redelivery of the batch that produced this residual must not
            // prepend that batch's own tail to itself.
            if carried.last_batch_head.is_some() && carried.last_batch_head == batch_head {
                continue;
            }
            group.sensitivity = max_sensitivity(group.sensitivity, carried.sensitivity);
            group.source_ref = carried.source_ref;
            group.text.insert_str(0, &carried.text);
        }

        let mut candidates = Vec::new();
        for key in order {
            let Some(group) = groups.remove(&key) else {
                continue;
            };
            let run_ended = ended_runs.contains(&key.0);
            // Everything after the final newline is an incomplete line: hold
            // it back unless the run has ended, since the rest of it may
            // arrive in a later batch.
            let (complete, trailing) = match group.text.rfind('\n') {
                Some(index) => group.text.split_at(index + 1),
                None => ("", group.text.as_str()),
            };
            let scanned = if run_ended {
                group.text.as_str()
            } else {
                complete
            };

            self.scan_into(
                scanned,
                &group.source_run,
                &group.source_ref,
                group.sensitivity,
                &mut candidates,
            );

            if run_ended || trailing.is_empty() || trailing.len() > MAX_RESIDUAL_BYTES {
                continue;
            }
            if residual.len() >= MAX_TRACKED_GROUPS && !residual.contains_key(&key) {
                continue;
            }
            residual.insert(
                key,
                Residual {
                    text: trailing.to_owned(),
                    source_ref: Arc::clone(&group.source_ref),
                    sensitivity: group.sensitivity,
                    last_batch_head: batch_head,
                },
            );
        }

        self.flush_ended_runs(&ended_runs, &mut residual, &mut candidates);

        candidates
    }
}
