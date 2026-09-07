//! Shared append validation. Storage backends cannot reinterpret attempt authority.
use super::{Mutation, StoreSnapshot};
use crate::error::invalid;
use crate::{
    AttemptStatus, EVAL_ATTEMPT_SEQUENCE_CONFLICT, EVAL_ATTEMPT_UNRESOLVED, EVAL_CELL_FINALIZED,
    EVAL_SPEC_DIVERGED, EVAL_SUBJECT_LOCK_MISMATCH, EVAL_SUBJECT_UNBOUND, EvalError,
    Reconciliation,
};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(super) const MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct State {
    pub(super) snapshot: StoreSnapshot,
    task_ids: BTreeSet<Arc<str>>,
}

fn conflict() -> EvalError {
    EvalError::new(
        EVAL_ATTEMPT_SEQUENCE_CONFLICT,
        "attempt mutation conflicts with durable sequence",
    )
}

impl State {
    pub(super) fn check(&self, mutation: &Mutation) -> Result<bool, EvalError> {
        if let Mutation::Freeze { experiment } = mutation {
            if experiment.spec.digest()? != experiment.digest {
                return Err(invalid());
            }
            if let Some(frozen) = &self.snapshot.frozen {
                if frozen.digest != experiment.digest {
                    return Err(EvalError::new(
                        EVAL_SPEC_DIVERGED,
                        "frozen experiment differs",
                    ));
                }
                return Ok(false);
            }
            return Ok(true);
        }
        let spec = &self.snapshot.frozen.as_ref().ok_or_else(invalid)?.spec;
        match mutation {
            Mutation::BindSubject { subject, digest } => {
                let declaration = spec
                    .subjects
                    .iter()
                    .find(|item| item.subject_id == *subject)
                    .ok_or_else(|| {
                        EvalError::new(EVAL_SUBJECT_UNBOUND, "subject is not declared")
                    })?;
                if declaration
                    .lock_digest
                    .is_some_and(|expected| expected != *digest)
                    || self
                        .snapshot
                        .subject_locks
                        .get(subject)
                        .is_some_and(|prior| prior != digest)
                {
                    return Err(EvalError::new(
                        EVAL_SUBJECT_LOCK_MISMATCH,
                        "subject lock differs",
                    ));
                }
                Ok(!self.snapshot.subject_locks.contains_key(subject))
            }
            Mutation::Reserve { reservation } => self.check_reservation(reservation, spec),
            Mutation::BindExecution {
                cell,
                sequence,
                identity,
            } => {
                let reservation = self.reservation(cell, *sequence)?;
                if let Some(prior) = &reservation.execution {
                    return if prior == identity {
                        Ok(false)
                    } else {
                        Err(conflict())
                    };
                }
                if identity.tenant_scope.is_empty()
                    || identity.tenant_scope.len() > 128
                    || identity.tenant_scope.contains('\0')
                    || self.result(cell, *sequence).is_some()
                {
                    return Err(invalid());
                }
                if self.session_used(identity) {
                    return Err(conflict());
                }
                Ok(true)
            }
            Mutation::Settle { record } => self.check_settlement(record),
            Mutation::GraderReserve { reservation } => self.check_grader_reservation(reservation),
            Mutation::GraderBind { key, identity } => self.check_grader_binding(key, identity),
            Mutation::GraderSettle { key, outcome } => self.check_grader_outcome(key, outcome),
            Mutation::Scores {
                cell,
                sequence,
                scores,
            } => self.check_scores(cell, *sequence, scores, spec),
            Mutation::Freeze { .. } => Err(invalid()),
        }
    }

    fn check_reservation(
        &self,
        reservation: &crate::AttemptReservation,
        spec: &crate::EvalSpec,
    ) -> Result<bool, EvalError> {
        let cell = &reservation.cell;
        if !self.task_ids.contains(&cell.task_id)
            || !self.snapshot.subject_locks.contains_key(&cell.subject_id)
            || cell.repetition >= spec.repetitions
            || cell.id.as_ref()
                != format!("{}::{}::{}", cell.task_id, cell.repetition, cell.subject_id)
            || reservation.execution.is_some()
        {
            return Err(invalid());
        }
        let reservations = self
            .snapshot
            .reservations
            .get(&cell.id)
            .map_or(&[][..], Vec::as_slice);
        if reservation.sequence as usize != reservations.len() + 1 {
            return Err(conflict());
        }
        if reservation.sequence > spec.limits.max_replacement_attempts + 1 {
            return Err(conflict());
        }
        if let Some(prior) = reservations.last() {
            let result =
                self.snapshot.attempts.get(&cell.id).and_then(|attempts| {
                    attempts.iter().find(|item| item.sequence == prior.sequence)
                });
            match result {
                Some(record) if record.status.is_final() => {
                    return Err(EvalError::new(
                        EVAL_CELL_FINALIZED,
                        "cell already has an authoritative result",
                    ));
                }
                Some(record)
                    if record.status == AttemptStatus::InfraFailed
                        && record.reconciliation == Reconciliation::NoAdmission => {}
                _ => {
                    return Err(EvalError::new(
                        EVAL_ATTEMPT_UNRESOLVED,
                        "prior attempt must be reconciled before replacement",
                    ));
                }
            }
        }
        Ok(true)
    }

    fn check_scores(
        &self,
        cell: &str,
        sequence: u32,
        scores: &crate::ScoreSet,
        spec: &crate::EvalSpec,
    ) -> Result<bool, EvalError> {
        let record = self.result(cell, sequence).ok_or_else(conflict)?;
        if !record.status.is_final()
            || record.scores.len() >= 256
            || !spec.scorers.contains(&scores.scorer)
        {
            return Err(invalid());
        }
        let pass = record
            .scores
            .iter()
            .filter(|prior| prior.scorer == scores.scorer)
            .count()
            + 1;
        if self.grader_blocks_score(cell, sequence, &scores.scorer, pass) {
            return Err(EvalError::new(
                EVAL_ATTEMPT_UNRESOLVED,
                "grader must settle before its scoring pass",
            ));
        }
        scores.validate()?;
        Ok(true)
    }

    fn reservation(
        &self,
        cell: &str,
        sequence: u32,
    ) -> Result<&crate::AttemptReservation, EvalError> {
        self.snapshot
            .reservations
            .get(cell)
            .and_then(|items| items.iter().find(|item| item.sequence == sequence))
            .ok_or_else(conflict)
    }

    pub(super) fn result(&self, cell: &str, sequence: u32) -> Option<&crate::AttemptRecord> {
        self.snapshot
            .attempts
            .get(cell)
            .and_then(|items| items.iter().find(|item| item.sequence == sequence))
    }

    fn check_settlement(&self, record: &crate::AttemptRecord) -> Result<bool, EvalError> {
        record
            .usage
            .validate_reconciliation(record.reconciliation)?;
        let reservation = self.reservation(&record.cell, record.sequence)?;
        if record.started_at_ms != reservation.started_at_ms
            || record.completed_at_ms < record.started_at_ms
            || !record.scores.is_empty()
            || record.record_kinds.len() > 256
            || record.artifacts.len() > 4096
        {
            return Err(invalid());
        }
        let valid = matches!(
            (record.status, record.reconciliation),
            (
                AttemptStatus::Completed | AttemptStatus::SubjectFailed,
                Reconciliation::Terminal
            ) | (AttemptStatus::InfraFailed, Reconciliation::NoAdmission)
                | (AttemptStatus::Indeterminate, Reconciliation::Unresolved)
        );
        if !valid || (record.status.is_final() && record.locator.is_none()) {
            return Err(invalid());
        }
        if let Some(locator) = &record.locator {
            let identity = reservation.execution.as_ref().ok_or_else(invalid)?;
            if locator.session_id != identity.session_id
                || locator.lane_id != identity.lane_id
                || locator.tenant_scope != identity.tenant_scope
            {
                return Err(conflict());
            }
        }
        if let Some(prior) = self.result(&record.cell, record.sequence) {
            if prior == record {
                return Ok(false);
            }
            if prior.status != AttemptStatus::Indeterminate
                || record.status == AttemptStatus::Indeterminate
            {
                return Err(conflict());
            }
        }
        Ok(true)
    }

    pub(super) fn apply(&mut self, mutation: Mutation) -> Result<(), EvalError> {
        if !self.check(&mutation)? {
            return Ok(());
        }
        match mutation {
            Mutation::Freeze { experiment } => {
                self.task_ids = experiment
                    .spec
                    .tasks
                    .iter()
                    .map(|task| Arc::clone(&task.task_id))
                    .collect();
                self.snapshot.frozen = Some(experiment);
            }
            Mutation::BindSubject { subject, digest } => {
                self.snapshot.subject_locks.insert(subject, digest);
            }
            Mutation::Reserve { reservation } => {
                self.snapshot
                    .reservations
                    .entry(Arc::clone(&reservation.cell.id))
                    .or_default()
                    .push(reservation);
            }
            Mutation::BindExecution {
                cell,
                sequence,
                identity,
            } => {
                let reservation = self
                    .snapshot
                    .reservations
                    .get_mut(&cell)
                    .and_then(|items| items.iter_mut().find(|item| item.sequence == sequence))
                    .ok_or_else(conflict)?;
                reservation.execution = Some(identity);
            }
            Mutation::Settle { record } => {
                let records = self
                    .snapshot
                    .attempts
                    .entry(Arc::clone(&record.cell))
                    .or_default();
                if let Some(prior) = records
                    .iter_mut()
                    .find(|item| item.sequence == record.sequence)
                {
                    *prior = record;
                } else {
                    records.push(record);
                }
            }
            Mutation::GraderReserve { reservation } => {
                self.snapshot.graders.insert(
                    reservation.key(),
                    crate::GraderRecord {
                        reservation,
                        execution: None,
                        outcome: None,
                    },
                );
            }
            Mutation::GraderBind { key, identity } => {
                self.snapshot
                    .graders
                    .get_mut(&key)
                    .ok_or_else(conflict)?
                    .execution = Some(identity);
            }
            Mutation::GraderSettle { key, outcome } => {
                self.snapshot
                    .graders
                    .get_mut(&key)
                    .ok_or_else(conflict)?
                    .outcome = Some(outcome);
            }
            Mutation::Scores {
                cell,
                sequence,
                scores,
            } => {
                self.snapshot
                    .attempts
                    .get_mut(&cell)
                    .and_then(|items| items.iter_mut().find(|item| item.sequence == sequence))
                    .ok_or_else(conflict)?
                    .scores
                    .push(scores);
            }
        }
        Ok(())
    }
}
