//! One scoring pipeline shared by execution, resume and explicit rescoring.
use crate::{
    AttemptRecord, Cell, EVAL_ATTEMPT_UNRESOLVED, EVAL_SCORER_FAILED, EvalError, EvalSpec,
    EvalStore, ScoreContext, ScoreSet, Scorer,
};
use crate::{
    execution::Cancellation,
    grading::{GraderExecution, GraderInvocation},
};
use finstack_ai::{AgentRunOutput, runtime::ports::journal::JournalStore};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub(crate) struct ScoringSession {
    spec: Arc<EvalSpec>,
    store: Arc<dyn EvalStore>,
    scorers: BTreeMap<Arc<str>, Arc<dyn Scorer>>,
}
impl ScoringSession {
    pub(crate) fn new(
        spec: Arc<EvalSpec>,
        store: Arc<dyn EvalStore>,
        scorers: Vec<Arc<dyn Scorer>>,
    ) -> Result<Self, EvalError> {
        let mut indexed = BTreeMap::new();
        for scorer in scorers {
            scorer.validate()?;
            if indexed.insert(scorer.id(), scorer).is_some() {
                return Err(crate::error::invalid());
            }
        }
        if indexed.len() != spec.scorers.len()
            || spec.scorers.iter().any(|id| !indexed.contains_key(id))
        {
            return Err(crate::error::invalid());
        }
        Ok(Self {
            spec,
            store,
            scorers: indexed,
        })
    }
    pub(crate) fn preflight(&self) -> Result<(), EvalError> {
        for scorer in self.scorers.values() {
            if let Some(agent) = scorer.grader_agent() {
                crate::subject::agent_digest(&agent)?;
                crate::budget::check_pricing_policy(&self.spec, &agent)?;
            }
        }
        Ok(())
    }
    pub(crate) async fn reconcile_graders(&self) -> Result<bool, EvalError> {
        let snapshot = self.store.snapshot()?;
        for record in snapshot
            .graders
            .values()
            .filter(|record| record.unresolved())
        {
            let scorer = self
                .scorers
                .get(&record.reservation.scorer)
                .ok_or_else(crate::error::invalid)?;
            if scorer.version() != record.reservation.scorer_version {
                return Err(EvalError::new(
                    crate::EVAL_SUBJECT_LOCK_MISMATCH,
                    "pending grader requires its original scoring version",
                ));
            }
            let agent = scorer.grader_agent().ok_or_else(crate::error::invalid)?;
            let cell = &snapshot
                .reservations
                .get(&record.reservation.cell)
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.sequence == record.reservation.attempt_sequence)
                })
                .ok_or_else(crate::error::invalid)?
                .cell;
            let result =
                crate::grading::recover_grader(self.store.as_ref(), &agent, cell, record).await;
            if let Err(error) = result
                && error.code() != EVAL_ATTEMPT_UNRESOLVED
            {
                return Err(error);
            }
        }
        Ok(self
            .store
            .snapshot()?
            .graders
            .values()
            .any(crate::GraderRecord::unresolved))
    }
    fn needs_scoring(&self, record: &AttemptRecord, force: bool) -> bool {
        record.status.is_final()
            && (force
                || self
                    .spec
                    .scorers
                    .iter()
                    .any(|id| !record.scores.iter().any(|pass| pass.scorer == *id)))
    }
    pub(crate) async fn score_recorded(
        &self,
        cell: &Cell,
        record: AttemptRecord,
        journal: Arc<dyn JournalStore>,
        force: bool,
        cancellation: &Cancellation,
    ) -> Result<AttemptRecord, EvalError> {
        if !self.needs_scoring(&record, force) {
            return Ok(record);
        }
        let reservation = self
            .store
            .snapshot()?
            .reservations
            .get(&cell.id)
            .and_then(|items| items.iter().find(|item| item.sequence == record.sequence))
            .cloned()
            .ok_or_else(crate::error::invalid)?;
        let output =
            crate::reconcile_attempt(Arc::clone(&journal), &reservation, crate::runner::now_ms())
                .await;
        let lock = self
            .store
            .snapshot()?
            .subject_locks
            .get(&cell.subject_id)
            .copied()
            .ok_or_else(crate::error::invalid)?;
        let output = output.and_then(|result| crate::measurement::verify_lock(lock, result));
        match output {
            Ok(result) if result.record.status == record.status => {
                self.score(
                    cell,
                    record,
                    result.output.as_ref(),
                    journal,
                    force,
                    cancellation,
                )
                .await
            }
            _ => self.record_unavailable(record, force),
        }
    }
    fn record_unavailable(
        &self,
        mut record: AttemptRecord,
        force: bool,
    ) -> Result<AttemptRecord, EvalError> {
        for id in &self.spec.scorers {
            if !force && record.scores.iter().any(|pass| pass.scorer == *id) {
                continue;
            }
            let implementation = self.scorers.get(id).ok_or_else(crate::error::invalid)?;
            let scores = ScoreSet {
                scorer: Arc::clone(id),
                scorer_version: implementation.version(),
                scores: Vec::new(),
                failure_code: Some(Arc::from(crate::EVAL_RESCORE_SESSION_MISSING)),
            };
            self.store
                .append_scores(&record.cell, record.sequence, &scores)?;
            record.scores.push(scores);
        }
        Ok(record)
    }
    pub(crate) async fn score(
        &self,
        cell: &Cell,
        mut record: AttemptRecord,
        output: Option<&AgentRunOutput>,
        journal: Arc<dyn JournalStore>,
        force: bool,
        cancellation: &Cancellation,
    ) -> Result<AttemptRecord, EvalError> {
        if !self.needs_scoring(&record, force) {
            return Ok(record);
        }
        let sample = self
            .spec
            .tasks
            .iter()
            .find(|sample| sample.task_id == cell.task_id)
            .ok_or_else(crate::error::invalid)?;
        for id in &self.spec.scorers {
            if !force && record.scores.iter().any(|pass| pass.scorer == *id) {
                continue;
            }
            if cancellation.is_cancelled() {
                break;
            }
            let implementation = self.scorers.get(id).ok_or_else(crate::error::invalid)?;
            let pass = u32::try_from(
                record
                    .scores
                    .iter()
                    .filter(|pass| pass.scorer == *id)
                    .count()
                    + 1,
            )
            .map_err(|_| crate::error::invalid())?;
            let grader = implementation.grader_agent().map(|agent| {
                GraderExecution::new(GraderInvocation {
                    store: Arc::clone(&self.store),
                    agent,
                    spec: Arc::clone(&self.spec),
                    cell: cell.clone(),
                    attempt_sequence: record.sequence,
                    scorer: Arc::clone(id),
                    scorer_version: implementation.version(),
                    pass,
                })
            });
            let context = ScoreContext {
                sample,
                output,
                record: &record,
                journal: &journal,
                grader: grader.as_ref(),
            };
            let result = tokio::select! {
                result = tokio::time::timeout(Duration::from_millis(self.spec.limits.attempt_timeout_ms), implementation.score(&context)) => result.unwrap_or_else(|_| Err(EvalError::new(EVAL_SCORER_FAILED, "scoring deadline exceeded"))),
                () = cancellation.wait() => Err(EvalError::new(EVAL_SCORER_FAILED, "scoring cancelled")),
            };
            if let Some(grader) = &grader {
                grader.finish(result.is_err()).await?;
            }
            // An unresolved agent grade must remain an unfinished pass. Closing
            // it as a mere scorer failure would permit unaccounted repeat spend.
            if result
                .as_ref()
                .is_err_and(|error| error.code() == EVAL_ATTEMPT_UNRESOLVED)
            {
                return Err(EvalError::new(
                    EVAL_ATTEMPT_UNRESOLVED,
                    "grader needs reconciliation",
                ));
            }
            let scores = match result {
                Ok(scores) => ScoreSet {
                    scorer: Arc::clone(id),
                    scorer_version: implementation.version(),
                    scores,
                    failure_code: None,
                },
                Err(error) => ScoreSet {
                    scorer: Arc::clone(id),
                    scorer_version: implementation.version(),
                    scores: Vec::new(),
                    failure_code: Some(Arc::from(error.code())),
                },
            };
            let scores = match scores.validate() {
                Ok(()) => scores,
                Err(_) => ScoreSet {
                    scorer: Arc::clone(id),
                    scorer_version: implementation.version(),
                    scores: Vec::new(),
                    failure_code: Some(Arc::from(EVAL_SCORER_FAILED)),
                },
            };
            self.store
                .append_scores(&record.cell, record.sequence, &scores)?;
            record.scores.push(scores);
        }
        Ok(record)
    }
}
