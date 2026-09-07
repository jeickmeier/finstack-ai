//! Persisted agent grading with an explicit owner that is joined after callbacks.
use crate::execution::Cancellation;
use crate::{
    AttemptStatus, Cell, EVAL_ATTEMPT_UNRESOLVED, EVAL_SCORER_FAILED, EVAL_SUBJECT_LOCK_MISMATCH,
    EvalError, EvalSpec, ExecutionIdentity, GraderOutcome, GraderRecord, GraderReservation,
    ReconciledAttempt,
};
use finstack_ai::{Agent, AgentRunOutput, AgentRunRequest, Session};
use finstack_ai_kernel::Digest;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Notify, task::JoinHandle};

/// One runner-owned grader slot. Custom Rust scorers may invoke this helper once
/// per pass; subject/grader scheduling, accounting and durable identity stay in Rust.
/// Dropping its await requests cancellation. The runner always joins its owner
/// before releasing experiment ownership or finishing the scoring pass.
pub struct GraderExecution {
    inner: Arc<Inner>,
}
pub(crate) struct GraderInvocation {
    pub store: crate::async_store::AsyncEvalStore,
    pub agent: Agent,
    pub spec: Arc<EvalSpec>,
    pub cell: Cell,
    pub attempt_sequence: u32,
    pub scorer: Arc<str>,
    pub scorer_version: u32,
    pub pass: u32,
}
struct Inner {
    invocation: GraderInvocation,
    cancellation: Cancellation,
    called: AtomicBool,
    task: Mutex<Option<JoinHandle<()>>>,
    result: Mutex<Option<Result<AgentRunOutput, EvalError>>>,
    ready: Notify,
}
struct AwaitGuard(Cancellation);
impl Drop for AwaitGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
fn failed() -> EvalError {
    EvalError::new(
        EVAL_SCORER_FAILED,
        "grader execution did not produce a verified result",
    )
}
fn unresolved() -> EvalError {
    EvalError::new(
        EVAL_ATTEMPT_UNRESOLVED,
        "grader execution remains unresolved",
    )
}
impl GraderExecution {
    pub(crate) fn new(invocation: GraderInvocation) -> Self {
        Self {
            inner: Arc::new(Inner {
                invocation,
                cancellation: Cancellation::default(),
                called: AtomicBool::new(false),
                task: Mutex::new(None),
                result: Mutex::new(None),
                ready: Notify::new(),
            }),
        }
    }
    /// Run the bound grader on an actual fresh reserved session, or reuse its
    /// verified result when this scoring pass was interrupted after dispatch.
    /// # Errors
    /// Returns budget, configuration, store, subject or unresolved-execution errors.
    pub async fn run(&self, request: AgentRunRequest) -> Result<AgentRunOutput, EvalError> {
        if self.inner.called.swap(true, Ordering::AcqRel) {
            return Err(failed());
        }
        let owner = Arc::clone(&self.inner);
        let task = tokio::spawn(async move {
            let result = Box::pin(owner.execute(request)).await;
            if let Ok(mut slot) = owner.result.lock() {
                *slot = Some(result);
            }
            owner.ready.notify_waiters();
        });
        *self.inner.task.lock().map_err(|_| failed())? = Some(task);
        let _cancel_on_drop = AwaitGuard(self.inner.cancellation.clone());
        loop {
            let notified = self.inner.ready.notified();
            if let Some(result) = self.inner.result.lock().map_err(|_| failed())?.clone() {
                return result;
            }
            notified.await;
        }
    }
    pub(crate) async fn finish(&self, cancel: bool) -> Result<(), EvalError> {
        if cancel {
            self.inner.cancellation.cancel();
        }
        let task = self.inner.task.lock().map_err(|_| failed())?.take();
        if let Some(task) = task {
            task.await.map_err(|_| unresolved())?;
        }
        Ok(())
    }
}
impl Inner {
    async fn execute(&self, request: AgentRunRequest) -> Result<AgentRunOutput, EvalError> {
        let invocation = &self.invocation;
        let reservation = GraderReservation {
            cell: Arc::clone(&invocation.cell.id),
            attempt_sequence: invocation.attempt_sequence,
            scorer: Arc::clone(&invocation.scorer),
            scorer_version: invocation.scorer_version,
            pass: invocation.pass,
            lock_digest: crate::subject::agent_digest(&invocation.agent)?,
            request_digest: request_digest(&request)?,
            started_at_ms: crate::runner::now_ms(),
        };
        let key = reservation.key();
        let snapshot = invocation.store.snapshot().await?;
        if let Some(prior) = snapshot.graders.get(&key) {
            if prior.reservation.lock_digest != reservation.lock_digest
                || prior.reservation.request_digest != reservation.request_digest
                || prior.reservation.scorer_version != reservation.scorer_version
            {
                return Err(EvalError::new(
                    EVAL_SUBJECT_LOCK_MISMATCH,
                    "recorded grader configuration differs",
                ));
            }
            return recover_grader(
                &invocation.store,
                &invocation.agent,
                &invocation.cell,
                prior,
            )
            .await?
            .output
            .ok_or_else(failed);
        }
        crate::budget::admission_budget(&invocation.spec, &snapshot)?;
        invocation.store.reserve_grader(&reservation).await?;
        let mut record = GraderRecord {
            reservation,
            execution: None,
            outcome: None,
        };
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(invocation.spec.limits.attempt_timeout_ms);
        let session = tokio::time::timeout_at(
            deadline,
            Session::create(
                invocation.agent.journal_store(),
                request.security.tenant_scope(),
            ),
        )
        .await;
        let Ok(Ok(session)) = session else {
            return recover_grader(
                &invocation.store,
                &invocation.agent,
                &invocation.cell,
                &record,
            )
            .await?
            .output
            .ok_or_else(failed);
        };
        let lane = tokio::time::timeout_at(deadline, session.lane("main"))
            .await
            .map_err(|_| unresolved())?
            .map_err(|_| unresolved())?;
        let identity = ExecutionIdentity {
            tenant_scope: Arc::from(session.tenant_scope()),
            session_id: session.session_id(),
            lane_id: lane.lane_id(),
        };
        invocation
            .store
            .bind_grader_execution(&key, identity.clone())
            .await?;
        record.execution = Some(identity);
        crate::execution::drive(
            &lane,
            &invocation.agent,
            request,
            deadline,
            &self.cancellation,
        )
        .await;
        recover_grader(
            &invocation.store,
            &invocation.agent,
            &invocation.cell,
            &record,
        )
        .await?
        .output
        .ok_or_else(failed)
    }
}

pub(crate) async fn recover_grader(
    store: &crate::async_store::AsyncEvalStore,
    agent: &Agent,
    cell: &Cell,
    record: &GraderRecord,
) -> Result<ReconciledAttempt, EvalError> {
    if crate::subject::agent_digest(agent)? != record.reservation.lock_digest {
        return Err(EvalError::new(
            EVAL_SUBJECT_LOCK_MISMATCH,
            "grader lock differs on recovery",
        ));
    }
    let reservation = record.measurement_reservation(cell)?;
    let result =
        crate::reconcile_attempt(agent.journal_store(), &reservation, crate::runner::now_ms())
            .await
            .and_then(|result| {
                crate::measurement::verify_lock(record.reservation.lock_digest, result)
            });
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            if record.outcome.is_none() {
                let mut unknown = crate::measurement::no_admission(
                    &reservation,
                    crate::runner::now_ms(),
                    EVAL_ATTEMPT_UNRESOLVED,
                )
                .record;
                unknown.status = AttemptStatus::Indeterminate;
                unknown.reconciliation = crate::Reconciliation::Unresolved;
                unknown.usage = crate::MeasuredUsage::default();
                store
                    .settle_grader(&record.reservation.key(), &GraderOutcome::from(unknown))
                    .await?;
            }
            return Err(error);
        }
    };
    if record.outcome.is_none()
        || (record.unresolved() && result.record.status != AttemptStatus::Indeterminate)
    {
        store
            .settle_grader(
                &record.reservation.key(),
                &GraderOutcome::from(result.record.clone()),
            )
            .await?;
    }
    if result.record.status == AttemptStatus::Indeterminate {
        return Err(unresolved());
    }
    Ok(result)
}

fn request_digest(request: &AgentRunRequest) -> Result<Digest, EvalError> {
    if request.input.len() > 262_656
        || request.settings.values.as_bytes().len() > 65_536
        || request.attachments.len() > 8
    {
        return Err(failed());
    }
    let value = serde_json::json!({
        "model": request.model.as_str(), "input": request.input, "security": request.security,
        "settings": request.settings.values, "timeout_ms": request.timeout.as_millis().to_string(),
        "max_cycles": request.max_cycles, "max_output_retries": request.max_output_retries,
        "capability": request.capability, "attachments": request.attachments.iter().map(|attachment| &attachment.artifact).collect::<Vec<_>>()
    });
    let bytes = serde_json_canonicalizer::to_vec(&value).map_err(|_| failed())?;
    Digest::domain_separated("eval-grader-request", 1, &bytes).map_err(|_| failed())
}
