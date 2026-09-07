//! Append-only attempt results reference authoritative journal executions.
use crate::{Cell, ScoreSet};
use finstack_ai_kernel::{ArtifactRef, LaneId, OperationLocator, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One-based per-cell admission sequence; repetition remains a separate axis.
pub type AttemptSequence = u32;

/// Evidence-based subject classification, independent of scorer failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    /// Journal proves a successful terminal subject result.
    Completed,
    /// The subject reached a failed or cancelled terminal state.
    SubjectFailed,
    /// Infrastructure failed before execution was admitted, proven by reconciliation.
    InfraFailed,
    /// Admitted work or its effects remain unresolved; replacement is forbidden.
    Indeterminate,
}
impl AttemptStatus {
    /// Whether this outcome permanently selects the cell's authoritative attempt.
    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::SubjectFailed)
    }
}

/// Evidence required before settling or replacing an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconciliation {
    /// No dispatch was possible, or a complete journal proves no run was accepted.
    NoAdmission,
    /// Committed terminal history was reconstructed, including child execution.
    Terminal,
    /// Missing, incomplete or uncertain history; never replace automatically.
    Unresolved,
}

/// Fresh actual session/lane persisted before calling `Lane::run`.
/// A crash before storing the returned locator is resolved within this one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIdentity {
    /// Explicit authorized tenant; no global journal discovery.
    pub tenant_scope: Arc<str>,
    /// Dedicated session for this attempt.
    pub session_id: SessionId,
    /// Actual lane on which execution is admitted.
    pub lane_id: LaneId,
}

/// Durable admission intent, before preparation or any subject dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptReservation {
    /// Frozen experiment coordinate.
    pub cell: Cell,
    /// Monotonic per-cell sequence.
    pub sequence: AttemptSequence,
    /// Host clock at reservation, milliseconds since Unix epoch.
    pub started_at_ms: u64,
    /// Bound exactly once after creating the actual session, before dispatch.
    pub execution: Option<ExecutionIdentity>,
}

/// Exact unit-aware aggregate. Mixed or missing costs never become a zero total.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredUsage {
    /// Sum when every relevant model effect reports input tokens.
    pub input_tokens: Option<u64>,
    /// Sum when every relevant model effect reports output tokens.
    pub output_tokens: Option<u64>,
    /// Sum when every relevant model effect reports total tokens.
    pub total_tokens: Option<u64>,
    /// Known subtotals by unit; incomplete coverage still leaves `cost` absent.
    pub cost_by_unit: BTreeMap<Arc<str>, u64>,
    /// Complete total only when all chargeable effects use one known unit.
    pub cost: Option<MeasuredCost>,
    /// Distinct settled effect identities folded into usage.
    pub effects: u64,
    /// Model requests with a settled receipt.
    pub model_effects: u64,
    /// Chargeable effects without reported cost, including failed dispatches.
    pub uncosted_effects: u64,
    /// Whether all admitted child and parent effect history is covered.
    pub complete: bool,
}

/// Aggregate cost in one explicitly known unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredCost {
    /// Monetary or other provider cost unit.
    pub unit: Arc<str>,
    /// Exact integer millionths. JSON exports additionally encode this as a string.
    pub micros: u64,
}

impl MeasuredUsage {
    pub(crate) fn validate_reconciliation(
        &self,
        reconciliation: Reconciliation,
    ) -> Result<(), crate::EvalError> {
        self.validate()?;
        if reconciliation == Reconciliation::NoAdmission
            && (self.effects != 0
                || self.model_effects != 0
                || self.uncosted_effects != 0
                || !self.cost_by_unit.is_empty()
                || self.cost.is_some()
                || [self.input_tokens, self.output_tokens, self.total_tokens]
                    .into_iter()
                    .flatten()
                    .any(|value| value != 0))
        {
            return Err(crate::error::invalid());
        }
        Ok(())
    }
    /// Validate bounded counters and the evidence supporting a complete cost total.
    /// # Errors
    /// Rejects inconsistent coverage, units or counters before persistence.
    pub fn validate(&self) -> Result<(), crate::EvalError> {
        if self.cost_by_unit.len() > 128
            || self
                .cost_by_unit
                .keys()
                .any(|unit| unit.is_empty() || unit.len() > 64 || unit.contains('\0'))
            || self.model_effects > self.effects
            // Pending chargeable effects lack receipts and are not included in `effects`.
            || self.uncosted_effects > 100_000
            || self.cost.as_ref().is_some_and(|cost| {
                !self.complete
                    || self.uncosted_effects != 0
                    || self.cost_by_unit.len() != 1
                    || self.cost_by_unit.get(&cost.unit) != Some(&cost.micros)
            })
        {
            return Err(crate::error::invalid());
        }
        Ok(())
    }
}

/// Settled result kernel. Transcripts and task bodies remain outside this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    /// Cell key from its immutable reservation.
    pub cell: Arc<str>,
    /// Reservation sequence, never reused for another execution.
    pub sequence: AttemptSequence,
    /// Subject outcome, not scorer outcome.
    pub status: AttemptStatus,
    /// Evidence for the outcome and replacement eligibility.
    pub reconciliation: Reconciliation,
    /// Safe stable subject/infrastructure failure code.
    pub failure_code: Option<Arc<str>>,
    /// Committed root locator when admission occurred.
    pub locator: Option<OperationLocator>,
    /// Root plus child usage from committed receipts, excluding graders.
    pub usage: MeasuredUsage,
    /// Journal acceptance-to-terminal duration, absent without complete endpoints.
    pub duration_ms: Option<u64>,
    /// Unique committed record kinds in lexical order.
    pub record_kinds: Vec<Arc<str>>,
    /// Append-only scoring passes projected from their own store events.
    pub scores: Vec<ScoreSet>,
    /// Scoped journal artifact references, never artifact contents.
    pub artifacts: Vec<ArtifactRef>,
    /// Reservation time, retained for infrastructure timing.
    pub started_at_ms: u64,
    /// Settlement or reconciliation time.
    pub completed_at_ms: u64,
}

/// A cell's earliest authoritative result, or its unresolved latest admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellOutcome {
    /// Stable cell key.
    pub cell: Arc<str>,
    /// Earliest final sequence, permanently selected when present.
    pub authoritative_sequence: Option<AttemptSequence>,
    /// Highest admitted sequence, including an unfinished reservation.
    pub latest_sequence: AttemptSequence,
}
