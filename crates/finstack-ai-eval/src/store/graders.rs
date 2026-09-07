//! Same append-only grading invariants for memory and `SQLite` projections.
use super::state::State;
use crate::{
    AttemptStatus, EVAL_ATTEMPT_SEQUENCE_CONFLICT, EVAL_ATTEMPT_UNRESOLVED, EvalError,
    ExecutionIdentity, GraderOutcome, GraderReservation, Reconciliation,
};
fn conflict() -> EvalError {
    EvalError::new(
        EVAL_ATTEMPT_SEQUENCE_CONFLICT,
        "grader mutation conflicts with durable sequence",
    )
}
impl State {
    pub(super) fn check_grader_reservation(
        &self,
        reservation: &GraderReservation,
    ) -> Result<bool, EvalError> {
        if let Some(prior) = self.snapshot.graders.get(&reservation.key()) {
            return if prior.reservation == *reservation {
                Ok(false)
            } else {
                Err(conflict())
            };
        }
        let spec = &self
            .snapshot
            .frozen
            .as_ref()
            .ok_or_else(crate::error::invalid)?
            .spec;
        let parent = self
            .result(&reservation.cell, reservation.attempt_sequence)
            .ok_or_else(conflict)?;
        let pass = parent
            .scores
            .iter()
            .filter(|score| score.scorer == reservation.scorer)
            .count()
            + 1;
        if !parent.status.is_final()
            || !spec.scorers.contains(&reservation.scorer)
            || reservation.scorer_version == 0
            || usize::try_from(reservation.pass).ok() != Some(pass)
            || parent.scores.len() >= 256
            || self.snapshot.graders.len() >= 100_000
        {
            return Err(conflict());
        }
        if self.snapshot.graders.values().any(|record| {
            record.reservation.cell == reservation.cell
                && record.reservation.attempt_sequence == reservation.attempt_sequence
                && record.reservation.scorer == reservation.scorer
                && record.unresolved()
        }) {
            return Err(EvalError::new(
                EVAL_ATTEMPT_UNRESOLVED,
                "prior grader must be reconciled",
            ));
        }
        Ok(true)
    }
    pub(super) fn check_grader_binding(
        &self,
        key: &str,
        identity: &ExecutionIdentity,
    ) -> Result<bool, EvalError> {
        let record = self.snapshot.graders.get(key).ok_or_else(conflict)?;
        if let Some(prior) = &record.execution {
            return if prior == identity {
                Ok(false)
            } else {
                Err(conflict())
            };
        }
        if record.outcome.is_some()
            || identity.tenant_scope.is_empty()
            || identity.tenant_scope.len() > 128
            || identity.tenant_scope.contains('\0')
        {
            return Err(conflict());
        }
        if self.session_used(identity) {
            return Err(conflict());
        }
        Ok(true)
    }
    pub(super) fn session_used(&self, identity: &ExecutionIdentity) -> bool {
        self.snapshot
            .reservations
            .values()
            .flatten()
            .filter_map(|record| record.execution.as_ref())
            .chain(
                self.snapshot
                    .graders
                    .values()
                    .filter_map(|record| record.execution.as_ref()),
            )
            .any(|prior| prior.session_id == identity.session_id)
    }
    pub(super) fn check_grader_outcome(
        &self,
        key: &str,
        outcome: &GraderOutcome,
    ) -> Result<bool, EvalError> {
        outcome
            .usage
            .validate_reconciliation(outcome.reconciliation)?;
        let record = self.snapshot.graders.get(key).ok_or_else(conflict)?;
        if outcome.completed_at_ms < record.reservation.started_at_ms
            || !matches!(
                (outcome.status, outcome.reconciliation),
                (
                    AttemptStatus::Completed | AttemptStatus::SubjectFailed,
                    Reconciliation::Terminal
                ) | (AttemptStatus::InfraFailed, Reconciliation::NoAdmission)
                    | (AttemptStatus::Indeterminate, Reconciliation::Unresolved)
            )
            || (outcome.status.is_final() && outcome.locator.is_none())
        {
            return Err(conflict());
        }
        if let Some(locator) = &outcome.locator {
            let identity = record.execution.as_ref().ok_or_else(conflict)?;
            if locator.tenant_scope != identity.tenant_scope
                || locator.session_id != identity.session_id
                || locator.lane_id != identity.lane_id
            {
                return Err(conflict());
            }
        }
        if let Some(prior) = &record.outcome {
            if prior == outcome {
                return Ok(false);
            }
            if prior.status != AttemptStatus::Indeterminate
                || outcome.status == AttemptStatus::Indeterminate
            {
                return Err(conflict());
            }
        }
        Ok(true)
    }
    pub(super) fn grader_blocks_score(
        &self,
        cell: &str,
        sequence: u32,
        scorer: &str,
        pass: usize,
    ) -> bool {
        self.snapshot.graders.values().any(|record| {
            record.reservation.cell.as_ref() == cell
                && record.reservation.attempt_sequence == sequence
                && record.reservation.scorer.as_ref() == scorer
                && usize::try_from(record.reservation.pass).ok() == Some(pass)
                && record.unresolved()
        })
    }
}
