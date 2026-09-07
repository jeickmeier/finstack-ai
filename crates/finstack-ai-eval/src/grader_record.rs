//! Append-only grader admissions, independent of subject outcomes and score passes.
use crate::{AttemptRecord, AttemptStatus, ExecutionIdentity, MeasuredUsage, Reconciliation};
use finstack_ai_kernel::{Digest, OperationLocator};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Durable intent for one grader execution in one scoring pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraderReservation {
    /// Parent subject cell.
    pub cell: Arc<str>,
    /// Parent authoritative subject attempt sequence.
    pub attempt_sequence: u32,
    /// Scorer identity from the frozen specification.
    pub scorer: Arc<str>,
    /// Grader behavior/configuration version.
    pub scorer_version: u32,
    /// One-based pass number for this scorer on this attempt.
    pub pass: u32,
    /// Exact credential-free grader agent lock, fixed before dispatch.
    pub lock_digest: Digest,
    /// Digest of the exact non-secret grader request and rubric inputs.
    pub request_digest: Digest,
    /// Reservation wall time in Unix milliseconds.
    pub started_at_ms: u64,
}
impl GraderReservation {
    /// Unambiguous immutable key under validated cell/scorer identities.
    #[must_use]
    pub fn key(&self) -> Arc<str> {
        Arc::from(format!(
            "{}::{}::{}::{}",
            self.cell, self.attempt_sequence, self.scorer, self.pass
        ))
    }
}

/// Grader execution evidence. Failed scoring still retains this spending record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraderOutcome {
    /// Completed/failed subject execution, or unresolved/no-admission classification.
    pub status: AttemptStatus,
    /// Evidence governing whether a later pass can safely proceed.
    pub reconciliation: Reconciliation,
    /// Stable safe failure code when execution did not complete successfully.
    pub failure_code: Option<Arc<str>>,
    /// Actual admitted grader operation, if known.
    pub locator: Option<OperationLocator>,
    /// Grader-only root and child usage, always separate from subject usage.
    pub usage: MeasuredUsage,
    /// Settlement/reconciliation wall time.
    pub completed_at_ms: u64,
}
impl From<AttemptRecord> for GraderOutcome {
    fn from(record: AttemptRecord) -> Self {
        Self {
            status: record.status,
            reconciliation: record.reconciliation,
            failure_code: record.failure_code,
            locator: record.locator,
            usage: record.usage,
            completed_at_ms: record.completed_at_ms,
        }
    }
}
/// Current projection of append-only grader reservation, identity and outcome events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraderRecord {
    /// Immutable admission intent.
    pub reservation: GraderReservation,
    /// Actual session/lane persisted before dispatch.
    pub execution: Option<ExecutionIdentity>,
    /// Absent until settled; unresolved outcomes require journal reconciliation.
    pub outcome: Option<GraderOutcome>,
}
impl GraderRecord {
    /// Whether this grader blocks new admission until reconciled.
    #[must_use]
    pub fn unresolved(&self) -> bool {
        self.outcome
            .as_ref()
            .is_none_or(|outcome| outcome.status == AttemptStatus::Indeterminate)
    }
    #[cfg(feature = "native-tokio")]
    pub(crate) fn measurement_reservation(
        &self,
        cell: &crate::Cell,
    ) -> Result<crate::AttemptReservation, crate::EvalError> {
        if cell.id != self.reservation.cell {
            return Err(crate::error::invalid());
        }
        Ok(crate::AttemptReservation {
            cell: cell.clone(),
            sequence: self.reservation.attempt_sequence,
            started_at_ms: self.reservation.started_at_ms,
            execution: self.execution.clone(),
        })
    }
}
