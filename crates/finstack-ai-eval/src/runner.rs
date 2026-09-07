//! Bounded execution with reservation-before-dispatch and explicit cancellation.
use crate::{
    AttemptRecord, AttemptReservation, AttemptStatus, EVAL_ATTEMPT_UNRESOLVED,
    EVAL_SUBJECT_UNBOUND, EvalError, EvalSpec, EvalStore, ExecutionIdentity, Reconciliation,
    StoreSnapshot, SubjectBinding,
};
use finstack_ai::Session;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinSet;

/// Result snapshot plus an explicit admission stop; partial execution is never an empty success.
#[derive(Debug, Clone)]
pub struct EvalRunReport {
    /// Durable current experiment state, including unresolved reservations.
    pub snapshot: StoreSnapshot,
    /// Stable reason further admission stopped; absent after all eligible cells finished.
    pub stop_reason: Option<Arc<str>>,
}

/// Owns experiment execution. Dropping `run()`'s await detaches its owner task;
/// call [`Self::cancel`] explicitly and await `run()` to observe settlement.
/// A cancelled runner stays cancelled; use a new instance to resume later.
#[derive(Clone)]
pub struct EvalRunner {
    inner: Arc<Inner>,
}
struct Inner {
    spec: Arc<EvalSpec>,
    scoring: crate::scoring_session::ScoringSession,
    store: Arc<dyn EvalStore>,
    subjects: BTreeMap<Arc<str>, SubjectBinding>,
    cancellation: crate::execution::Cancellation,
}
impl EvalRunner {
    /// Validate bounded data and subject identities. No subject executes here.
    /// # Errors
    /// Returns invalid specs, duplicate or missing subject bindings.
    pub fn new(
        spec: EvalSpec,
        store: Arc<dyn EvalStore>,
        subjects: Vec<SubjectBinding>,
        scorers: Vec<Arc<dyn crate::Scorer>>,
    ) -> Result<Self, EvalError> {
        spec.validate()?;
        let mut indexed = BTreeMap::new();
        for subject in subjects {
            if indexed.insert(subject.subject.id(), subject).is_some() {
                return Err(crate::error::invalid());
            }
        }
        if indexed.len() != spec.subjects.len()
            || spec
                .subjects
                .iter()
                .any(|subject| !indexed.contains_key(&subject.subject_id))
        {
            return Err(EvalError::new(
                EVAL_SUBJECT_UNBOUND,
                "declared subjects must be bound exactly once",
            ));
        }
        let spec = Arc::new(spec);
        let scoring = crate::scoring_session::ScoringSession::new(
            Arc::clone(&spec),
            Arc::clone(&store),
            scorers,
        )?;
        Ok(Self {
            inner: Arc::new(Inner {
                spec,
                scoring,
                store,
                subjects: indexed,
                cancellation: crate::execution::Cancellation::default(),
            }),
        })
    }
    /// Stop admission and explicitly request cancellation of active attempts.
    pub fn cancel(&self) {
        self.inner.cancellation.cancel();
    }
    /// Freeze, reconcile previous admissions, then execute eligible cells.
    /// The spawned owner retains the exclusive store lease through settlement.
    /// # Errors
    /// Returns lock/spec/store conflicts. Execution stops are carried by the report.
    pub async fn run(&self) -> Result<EvalRunReport, EvalError> {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move { inner.execute().await })
            .await
            .map_err(|_| crate::error::unavailable())?
    }
    /// Resume the same frozen experiment, reconciling before any replacement.
    /// # Errors
    /// Returns the same preflight/storage failures as [`Self::run`].
    pub async fn resume(&self) -> Result<EvalRunReport, EvalError> {
        self.run().await
    }
    /// Append a new scoring pass for each final recorded subject, reading its journal.
    /// Never prepares or executes subjects. Judge scorers may execute graders and incur
    /// separately recorded spending. The owner retains its lease until graders settle.
    /// # Errors
    /// Returns frozen/lock mismatch, unavailable history or storage errors. Missing
    /// subject journals become explicit scoring failures rather than repeated subjects.
    pub async fn rescore(&self) -> Result<EvalRunReport, EvalError> {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move { inner.rescore().await })
            .await
            .map_err(|_| crate::error::unavailable())?
    }
}
impl Inner {
    async fn rescore(&self) -> Result<EvalRunReport, EvalError> {
        let _lease = self.store.acquire_runner()?;
        self.store.freeze(&self.spec)?;
        for (id, binding) in &self.subjects {
            self.store.bind_subject(id, binding.lock_digest()?)?;
        }
        self.scoring.preflight()?;
        let mut stop_reason = self
            .scoring
            .reconcile_graders()
            .await?
            .then(|| Arc::from(EVAL_ATTEMPT_UNRESOLVED));
        let snapshot = self.store.snapshot()?;
        for cell in self.spec.cells()? {
            if stop_reason.is_some() {
                break;
            }
            if self.cancellation.is_cancelled() {
                stop_reason = Some(Arc::from("eval_cancelled"));
                break;
            }
            let Some(record) = snapshot
                .attempts
                .get(&cell.id)
                .and_then(|records| records.iter().find(|record| record.status.is_final()))
            else {
                continue;
            };
            let binding = self
                .subjects
                .get(&cell.subject_id)
                .ok_or_else(crate::error::invalid)?;
            match self
                .scoring
                .score_recorded(
                    &cell,
                    record.clone(),
                    binding.agent.journal_store(),
                    true,
                    &self.cancellation,
                )
                .await
            {
                Ok(_) => {}
                Err(error) if error.code() == EVAL_ATTEMPT_UNRESOLVED => {
                    stop_reason = Some(Arc::from(EVAL_ATTEMPT_UNRESOLVED));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(EvalRunReport {
            snapshot: self.store.snapshot()?,
            stop_reason,
        })
    }
    async fn execute(self: Arc<Self>) -> Result<EvalRunReport, EvalError> {
        let _lease = self.store.acquire_runner()?;
        let mut stop = self.preflight_and_reconcile().await?;
        let mut cells: VecDeque<_> = self.spec.cells()?.into();
        let mut active = JoinSet::new();
        let mut store_error = None;
        loop {
            while stop.is_none()
                && active.len() < self.spec.limits.max_concurrency as usize
                && !cells.is_empty()
            {
                if self.cancellation.is_cancelled() {
                    stop = Some(Arc::from("eval_cancelled"));
                    break;
                }
                let snapshot = match self.store.snapshot() {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        store_error = Some(error);
                        stop = Some(Arc::from("eval_store_unavailable"));
                        break;
                    }
                };
                let Some(cell) = cells.pop_front() else {
                    break;
                };
                let reservations = snapshot.reservations.get(&cell.id);
                let prior = snapshot
                    .attempts
                    .get(&cell.id)
                    .and_then(|records| records.last());
                if prior.is_some_and(|record| record.status.is_final()) {
                    continue;
                }
                if prior.is_some_and(|record| record.status == AttemptStatus::Indeterminate) {
                    stop = Some(Arc::from(EVAL_ATTEMPT_UNRESOLVED));
                    break;
                }
                let sequence = reservations
                    .and_then(|items| items.last())
                    .map_or(1, |item| item.sequence.saturating_add(1));
                if sequence > self.spec.limits.max_replacement_attempts + 1 {
                    continue;
                }
                if let Err(error) = crate::budget::admission_budget(&self.spec, &snapshot) {
                    stop = Some(Arc::from(error.code()));
                    break;
                }
                let reservation = match self.store.reserve(&cell, sequence, now_ms()) {
                    Ok(reservation) => reservation,
                    Err(error) => {
                        store_error = Some(error);
                        stop = Some(Arc::from("eval_store_unavailable"));
                        break;
                    }
                };
                let owner = Arc::clone(&self);
                active.spawn(async move {
                    let cell = reservation.cell.clone();
                    (cell, Box::pin(owner.attempt(reservation)).await)
                });
            }
            if active.is_empty() {
                break;
            }
            match active.join_next().await {
                Some(Ok((cell, Ok(record)))) => {
                    if record.status == AttemptStatus::Indeterminate {
                        stop = Some(Arc::from(EVAL_ATTEMPT_UNRESOLVED));
                    }
                    if record.status == AttemptStatus::InfraFailed
                        && record.sequence <= self.spec.limits.max_replacement_attempts
                    {
                        cells.push_back(cell);
                    }
                }
                Some(Ok((_, Err(error)))) => {
                    store_error = Some(error);
                    stop = Some(Arc::from("eval_store_unavailable"));
                }
                Some(Err(_)) => {
                    stop = Some(Arc::from(EVAL_ATTEMPT_UNRESOLVED));
                }
                None => break,
            }
        }
        if let Some(error) = store_error {
            return Err(error);
        }
        Ok(EvalRunReport {
            snapshot: self.store.snapshot()?,
            stop_reason: stop,
        })
    }
    async fn preflight_and_reconcile(&self) -> Result<Option<Arc<str>>, EvalError> {
        self.store.freeze(&self.spec)?;
        for (id, binding) in &self.subjects {
            self.store.bind_subject(id, binding.lock_digest()?)?;
            crate::budget::check_pricing_policy(&self.spec, &binding.agent)?;
        }
        self.scoring.preflight()?;
        let initial = self.store.snapshot()?;
        for reservations in initial.reservations.values() {
            for reservation in reservations {
                let prior = initial
                    .attempts
                    .get(&reservation.cell.id)
                    .and_then(|records| {
                        records
                            .iter()
                            .find(|record| record.sequence == reservation.sequence)
                    });
                if prior.is_some_and(|record| record.status != AttemptStatus::Indeterminate) {
                    continue;
                }
                let binding = self
                    .subjects
                    .get(&reservation.cell.subject_id)
                    .ok_or_else(crate::error::invalid)?;
                let measured =
                    crate::reconcile_attempt(binding.agent.journal_store(), reservation, now_ms())
                        .await;
                let measured = measured.and_then(|result| {
                    crate::measurement::verify_lock(binding.lock_digest()?, result)
                });
                let record = measured.map_or_else(|_| unknown(reservation), |result| result.record);
                if !(prior.is_some() && record.status == AttemptStatus::Indeterminate) {
                    self.store.settle(&record)?;
                }
            }
        }
        if self.scoring.reconcile_graders().await? {
            return Ok(Some(Arc::from(EVAL_ATTEMPT_UNRESOLVED)));
        }
        let snapshot = self.store.snapshot()?;
        for (cell_id, records) in &snapshot.attempts {
            for record in records.iter().filter(|record| record.status.is_final()) {
                let cell = &snapshot
                    .reservations
                    .get(cell_id)
                    .and_then(|items| items.iter().find(|item| item.sequence == record.sequence))
                    .ok_or_else(crate::error::invalid)?
                    .cell;
                let binding = self
                    .subjects
                    .get(&cell.subject_id)
                    .ok_or_else(crate::error::invalid)?;
                self.scoring
                    .score_recorded(
                        cell,
                        record.clone(),
                        binding.agent.journal_store(),
                        false,
                        &self.cancellation,
                    )
                    .await?;
            }
        }
        Ok(self
            .store
            .snapshot()?
            .attempts
            .values()
            .flatten()
            .any(|record| record.status == AttemptStatus::Indeterminate)
            .then(|| Arc::from(EVAL_ATTEMPT_UNRESOLVED)))
    }
    async fn attempt(&self, reservation: AttemptReservation) -> Result<AttemptRecord, EvalError> {
        let binding = self
            .subjects
            .get(&reservation.cell.subject_id)
            .ok_or_else(crate::error::invalid)?;
        let timeout = Duration::from_millis(self.spec.limits.attempt_timeout_ms);
        let deadline = tokio::time::Instant::now() + timeout;
        let prepared = tokio::select! {
            value = tokio::time::timeout_at(deadline, binding.subject.prepare(&reservation.cell)) => value.ok().and_then(Result::ok),
            () = self.cancellation.wait() => None,
        };
        let Some(prepared) = prepared else {
            let record =
                crate::measurement::no_admission(&reservation, now_ms(), "eval_prepare_failed")
                    .record;
            self.store.settle(&record)?;
            return Ok(record);
        };
        if binding.check(&prepared).is_err() {
            let record = crate::measurement::no_admission(
                &reservation,
                now_ms(),
                crate::EVAL_SUBJECT_LOCK_MISMATCH,
            )
            .record;
            self.store.settle(&record)?;
            return Ok(record);
        }
        let session = tokio::time::timeout_at(
            deadline,
            Session::create(
                prepared.agent.journal_store(),
                prepared.request.security.tenant_scope(),
            ),
        )
        .await;
        let Ok(Ok(session)) = session else {
            let record =
                crate::measurement::no_admission(&reservation, now_ms(), "eval_session_failed")
                    .record;
            self.store.settle(&record)?;
            return Ok(record);
        };
        let lane = tokio::time::timeout_at(deadline, session.lane("main"))
            .await
            .map_err(|_| crate::error::unavailable())?
            .map_err(|_| crate::error::unavailable())?;
        let identity = ExecutionIdentity {
            tenant_scope: Arc::from(session.tenant_scope()),
            session_id: session.session_id(),
            lane_id: lane.lane_id(),
        };
        self.store
            .bind_execution(&reservation.cell.id, reservation.sequence, identity.clone())?;
        let reservation = AttemptReservation {
            execution: Some(identity),
            ..reservation
        };
        crate::execution::drive(
            &lane,
            &prepared.agent,
            prepared.request,
            deadline,
            &self.cancellation,
        )
        .await;
        let measured =
            crate::reconcile_attempt(prepared.agent.journal_store(), &reservation, now_ms())
                .await
                .and_then(|result| crate::measurement::verify_lock(binding.lock_digest()?, result));
        let (record, output) = measured.map_or_else(
            |_| (unknown(&reservation), None),
            |result| (result.record, result.output),
        );
        self.store.settle(&record)?;
        self.scoring
            .score(
                &reservation.cell,
                record,
                output.as_ref(),
                prepared.agent.journal_store(),
                false,
                &self.cancellation,
            )
            .await
    }
}
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
fn unknown(reservation: &AttemptReservation) -> AttemptRecord {
    let mut record =
        crate::measurement::no_admission(reservation, now_ms(), EVAL_ATTEMPT_UNRESOLVED).record;
    record.status = AttemptStatus::Indeterminate;
    record.reconciliation = Reconciliation::Unresolved;
    record.usage = crate::MeasuredUsage::default();
    record
}
