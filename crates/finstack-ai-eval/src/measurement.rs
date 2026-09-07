//! Read-only reconciliation from committed root and authorized child histories.
use crate::{
    AttemptRecord, AttemptReservation, AttemptStatus, EVAL_ARITHMETIC_OVERFLOW,
    EVAL_ATTEMPT_UNRESOLVED, EvalError, ExecutionIdentity, MeasuredCost, MeasuredUsage,
    Reconciliation,
};
use finstack_ai::runtime::{
    commit::CommitCoordinator,
    ports::journal::{JournalStore, LoadRequest, LoadedSession},
};
use finstack_ai::{AgentRunOutput, Session};
use finstack_ai_kernel::{
    ArtifactRef, EffectId, EffectKind, EffectRequested, KernelState, OperationLocator, RecordBody,
    RunId, SessionId, TerminalState, Timestamp, Usage,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

const MAX_RUNS: usize = 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_ARTIFACTS: usize = 4096;

fn unresolved() -> EvalError {
    EvalError::new(
        EVAL_ATTEMPT_UNRESOLVED,
        "journal execution cannot be reconciled",
    )
}
fn overflow() -> EvalError {
    EvalError::new(EVAL_ARITHMETIC_OVERFLOW, "evaluation usage overflow")
}
fn add(a: u64, b: u64) -> Result<u64, EvalError> {
    a.checked_add(b).ok_or_else(overflow)
}
fn add_optional(a: Option<u64>, b: Option<u64>) -> Result<Option<u64>, EvalError> {
    match (a, b) {
        (Some(a), Some(b)) => add(a, b).map(Some),
        _ => Ok(None),
    }
}

/// Journal-derived outcome and optional successful output, also used by rescoring.
#[derive(Debug, Clone)]
pub struct ReconciledAttempt {
    /// Store-ready result without scorer passes.
    pub record: AttemptRecord,
    /// Reconstructed successful output; failures need not have a final message.
    pub output: Option<AgentRunOutput>,
    /// Actual committed root lock for comparison with the experiment binding.
    pub accepted_lock_digest: Option<finstack_ai_kernel::Digest>,
}

/// Reconcile an attempt without dispatching any subject or replacing unresolved work.
/// Only the reserved session and its committed child locators are read. Pruned
/// receipts leave cost coverage incomplete even when a snapshot proves terminality.
/// # Errors
/// Returns a safe unresolved-history failure for missing, invalid, or over-bound
/// history; callers must retain the reservation and forbid replacement.
pub async fn reconcile_attempt(
    journal: Arc<dyn JournalStore>,
    reservation: &AttemptReservation,
    now_ms: u64,
) -> Result<ReconciledAttempt, EvalError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        reconcile_inner(journal, reservation, now_ms),
    )
    .await
    .map_err(|_| unresolved())?
}
async fn reconcile_inner(
    journal: Arc<dyn JournalStore>,
    reservation: &AttemptReservation,
    now_ms: u64,
) -> Result<ReconciledAttempt, EvalError> {
    let mut result = no_admission(reservation, now_ms, "eval_no_admission");
    let Some(identity) = &reservation.execution else {
        return Ok(result);
    };
    let Some((locator, loaded)) = locate_root(&journal, identity).await? else {
        return Ok(result);
    };
    let root = locator.run_id;
    result.record.locator = Some(locator.clone());
    let mut pending = VecDeque::from([(locator, None)]);
    let mut visited = BTreeSet::new();
    let mut sessions = BTreeMap::from([(identity.session_id, loaded)]);
    let mut records_seen = sessions.values().map(record_count).sum::<usize>();
    let mut fold = UsageFold::new();
    let mut all_terminal = true;
    let mut kinds = BTreeSet::new();
    let mut artifacts = Vec::new();
    while let Some((operation, parent)) = pending.pop_front() {
        if visited.len() >= MAX_RUNS || !visited.insert(operation.run_id) {
            return Err(unresolved());
        }
        if operation.tenant_scope != identity.tenant_scope {
            return Err(unresolved());
        }
        if let std::collections::btree_map::Entry::Vacant(entry) =
            sessions.entry(operation.session_id)
        {
            Session::open(
                Arc::clone(&journal),
                operation.session_id,
                Arc::clone(&identity.tenant_scope),
            )
            .await
            .map_err(|_| unresolved())?;
            let loaded = bounded_load(&journal, operation.session_id).await?;
            records_seen = records_seen
                .checked_add(record_count(&loaded))
                .ok_or_else(unresolved)?;
            if records_seen > MAX_RECORDS {
                return Err(unresolved());
            }
            entry.insert(loaded);
        }
        let loaded = sessions.get(&operation.session_id).ok_or_else(unresolved)?;
        let replay = CommitCoordinator::recover_run(
            Arc::clone(&journal),
            operation.session_id,
            Some(operation.run_id),
        )
        .await
        .map_err(|_| unresolved())?;
        let state = replay.state();
        // Fail closed if concurrent driving changed the snapshot being measured.
        if state.last_applied_sequence() != loaded.head_sequence
            || state.lane_id() != Some(operation.lane_id)
        {
            return Err(unresolved());
        }
        validate_lineage(state, &operation, root, parent)?;
        let terminal = state.terminal().is_some()
            && state.cancellation().is_none_or(|cancel| {
                cancel.uncertain_effects.is_empty() && cancel.outstanding_effects.is_empty()
            });
        all_terminal &= terminal;
        fold.usage.complete &= !loaded.omits_prefix();
        let trace = fold_records(
            loaded,
            &operation,
            state,
            &mut fold,
            &mut kinds,
            &mut artifacts,
        )?;
        if operation.run_id == root {
            project_root(&mut result, &operation, state, trace)?;
        }
        for child in state.child_preparations().values() {
            if child.child.remote.is_some() {
                all_terminal = false;
                fold.usage.complete = false;
                continue;
            }
            pending.push_back((
                child.child.operation.clone(),
                Some((operation.run_id, child.parent_effect_id)),
            ));
        }
        if pending.len().saturating_add(visited.len()) > MAX_RUNS {
            return Err(unresolved());
        }
    }
    Ok(finish_measurement(
        result,
        all_terminal,
        fold,
        kinds,
        artifacts,
    ))
}

fn finish_measurement(
    mut result: ReconciledAttempt,
    all_terminal: bool,
    mut fold: UsageFold,
    kinds: BTreeSet<Arc<str>>,
    artifacts: Vec<ArtifactRef>,
) -> ReconciledAttempt {
    if all_terminal {
        result.record.reconciliation = Reconciliation::Terminal;
    } else {
        result.record.status = AttemptStatus::Indeterminate;
        result.record.reconciliation = Reconciliation::Unresolved;
        result.record.failure_code = Some(Arc::from(EVAL_ATTEMPT_UNRESOLVED));
        result.output = None;
        fold.usage.complete = false;
    }
    result.record.usage = fold.finish();
    result.record.record_kinds = kinds.into_iter().collect();
    result.record.artifacts = artifacts;
    result
}

async fn locate_root(
    journal: &Arc<dyn JournalStore>,
    identity: &ExecutionIdentity,
) -> Result<Option<(OperationLocator, LoadedSession)>, EvalError> {
    // Session opening validates tenant authority before reading any result bodies.
    Session::open(
        Arc::clone(journal),
        identity.session_id,
        Arc::clone(&identity.tenant_scope),
    )
    .await
    .map_err(|_| unresolved())?;
    let loaded = bounded_load(journal, identity.session_id).await?;
    let mut roots = BTreeSet::new();
    for record in loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
    {
        if let RecordBody::RunAccepted(accepted) = record.body()
            && record.lane_id() == identity.lane_id
        {
            roots.insert(accepted.run_id());
        }
    }
    if let Some(snapshot) = loaded.accelerated.as_ref()
        && snapshot.state.lane_id() == Some(identity.lane_id)
        && let Some(accepted) = snapshot.state.accepted()
    {
        roots.insert(accepted.run_id());
    }
    if roots.is_empty() {
        if loaded.omits_prefix() {
            return Err(unresolved());
        }
        // Structural replay verifies the complete session envelope chain.
        CommitCoordinator::recover_run(Arc::clone(journal), identity.session_id, None)
            .await
            .map_err(|_| unresolved())?;
        return Ok(None);
    }
    if roots.len() != 1 {
        return Err(unresolved());
    }
    let root = *roots.first().ok_or_else(unresolved)?;
    let locator = OperationLocator {
        tenant_scope: Arc::clone(&identity.tenant_scope),
        session_id: identity.session_id,
        lane_id: identity.lane_id,
        run_id: root,
    };
    Ok(Some((locator, loaded)))
}

struct RunTrace {
    kinds: Vec<Arc<str>>,
    ended_at: Option<Timestamp>,
}
fn fold_records(
    loaded: &LoadedSession,
    operation: &OperationLocator,
    state: &KernelState,
    fold: &mut UsageFold,
    kinds: &mut BTreeSet<Arc<str>>,
    artifacts: &mut Vec<ArtifactRef>,
) -> Result<RunTrace, EvalError> {
    let records = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .filter(|record| record.run_id() == Some(operation.run_id));
    let mut requests = BTreeMap::new();
    let mut receipts = BTreeMap::new();
    let mut ordered_kinds = Vec::new();
    let mut ended_at = None;
    for record in records {
        ordered_kinds.push(Arc::<str>::from(record.body().kind_name()));
        kinds.insert(Arc::<str>::from(record.body().kind_name()));
        match record.body() {
            RecordBody::EffectRequested(request) => {
                requests.insert(request.effect_id(), request);
            }
            RecordBody::EffectCompleted(receipt) => {
                insert_receipt(&mut receipts, receipt.effect_id(), receipt.usage())?;
                for artifact in receipt.artifacts() {
                    if !artifacts.contains(artifact) {
                        if artifacts.len() >= MAX_ARTIFACTS {
                            return Err(unresolved());
                        }
                        artifacts.push(artifact.clone());
                    }
                }
            }
            RecordBody::EffectFailed(receipt) => {
                insert_receipt(&mut receipts, receipt.effect_id(), receipt.usage())?;
            }
            RecordBody::EffectCancelled(receipt) => {
                insert_receipt(&mut receipts, receipt.effect_id(), None)?;
            }
            RecordBody::RunCompleted(_)
            | RecordBody::RunFailed(_)
            | RecordBody::RunCancelled(_) => {
                ended_at = Some(record.timestamp());
            }
            _ => {}
        }
    }
    for (effect, request) in requests {
        let receipt = receipts.remove(&effect);
        // Child-agent dispatch is orchestration. Its child receipts carry
        // spend; a reported dispatch surcharge, if any, is still counted.
        let child_dispatch = state.child_preparations().contains_key(&effect);
        fold.push(
            operation.run_id,
            request,
            receipt.flatten(),
            receipt.is_some(),
            child_dispatch,
        )?;
    }
    if !receipts.is_empty() {
        fold.usage.complete = false;
    }
    Ok(RunTrace {
        kinds: ordered_kinds,
        ended_at,
    })
}

fn project_root(
    result: &mut ReconciledAttempt,
    operation: &OperationLocator,
    state: &KernelState,
    trace: RunTrace,
) -> Result<(), EvalError> {
    result.accepted_lock_digest = state
        .accepted()
        .map(finstack_ai_kernel::RunAccepted::resolved_agent_lock_digest);
    result.record.duration_ms = state
        .accepted_at()
        .zip(trace.ended_at)
        .and_then(|(start, end)| end.as_unix_ms().checked_sub(start.as_unix_ms()))
        .and_then(|duration| u64::try_from(duration).ok());
    match state.terminal() {
        Some(TerminalState::Completed(done)) => {
            let message = state
                .messages()
                .iter()
                .find(|message| *message.id() == done.result_message_id)
                .ok_or_else(unresolved)?
                .clone();
            result.output = Some(AgentRunOutput {
                locator: operation.clone(),
                message,
                retry_attempts: state.retry().attempts,
                active_capabilities: Arc::clone(state.active_capabilities()),
                record_kinds: trace.kinds.into(),
            });
            result.record.status = AttemptStatus::Completed;
            result.record.failure_code = None;
        }
        Some(TerminalState::Failed(failed)) => {
            result.record.status = AttemptStatus::SubjectFailed;
            result.record.failure_code = Some(Arc::from(failed.error.code.as_ref()));
        }
        Some(TerminalState::Cancelled(_)) => {
            result.record.status = AttemptStatus::SubjectFailed;
            result.record.failure_code = Some(Arc::from("eval_subject_cancelled"));
        }
        None => {}
    }
    Ok(())
}

fn validate_lineage(
    state: &KernelState,
    operation: &OperationLocator,
    root: RunId,
    parent: Option<(RunId, EffectId)>,
) -> Result<(), EvalError> {
    let accepted = state.accepted().ok_or_else(unresolved)?;
    let relation = accepted.relation();
    if accepted.security().tenant_scope() != operation.tenant_scope.as_ref()
        || relation.root_run_id() != root
        || match parent {
            Some((run, effect)) => {
                relation.parent_run_id() != Some(run) || relation.parent_effect_id() != Some(effect)
            }
            None => relation.parent_run_id().is_some() || relation.parent_effect_id().is_some(),
        }
    {
        return Err(unresolved());
    }
    Ok(())
}

fn record_count(loaded: &LoadedSession) -> usize {
    loaded
        .committed_batches
        .iter()
        .map(|batch| batch.records.len())
        .sum()
}
async fn bounded_load(
    journal: &Arc<dyn JournalStore>,
    session_id: SessionId,
) -> Result<LoadedSession, EvalError> {
    let loaded = journal
        .load(LoadRequest { session_id })
        .await
        .map_err(|_| unresolved())?;
    if record_count(&loaded) > MAX_RECORDS {
        return Err(unresolved());
    }
    Ok(loaded)
}
fn insert_receipt<'a>(
    receipts: &mut BTreeMap<EffectId, Option<&'a Usage>>,
    id: EffectId,
    usage: Option<&'a Usage>,
) -> Result<(), EvalError> {
    if let Some(prior) = receipts.insert(id, usage)
        && prior != usage
    {
        return Err(unresolved());
    }
    Ok(())
}

pub(crate) fn no_admission(
    reservation: &AttemptReservation,
    now_ms: u64,
    code: &str,
) -> ReconciledAttempt {
    ReconciledAttempt {
        output: None,
        accepted_lock_digest: None,
        record: AttemptRecord {
            cell: Arc::clone(&reservation.cell.id),
            sequence: reservation.sequence,
            status: AttemptStatus::InfraFailed,
            reconciliation: Reconciliation::NoAdmission,
            failure_code: Some(Arc::from(code)),
            locator: None,
            usage: UsageFold::new().finish(),
            duration_ms: None,
            record_kinds: Vec::new(),
            scores: Vec::new(),
            artifacts: Vec::new(),
            started_at_ms: reservation.started_at_ms,
            completed_at_ms: now_ms,
        },
    }
}

struct UsageFold {
    usage: MeasuredUsage,
    seen: BTreeSet<(RunId, EffectId)>,
}
impl UsageFold {
    fn new() -> Self {
        Self {
            usage: MeasuredUsage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                total_tokens: Some(0),
                complete: true,
                ..MeasuredUsage::default()
            },
            seen: BTreeSet::new(),
        }
    }
    fn push(
        &mut self,
        run: RunId,
        request: &EffectRequested,
        usage: Option<&Usage>,
        settled: bool,
        child_dispatch: bool,
    ) -> Result<(), EvalError> {
        if !self.seen.insert((run, request.effect_id())) {
            return Err(unresolved());
        }
        if !settled {
            self.usage.complete = false;
        }
        if settled {
            self.usage.effects = add(self.usage.effects, 1)?;
        }
        if request.kind() == EffectKind::Model {
            self.usage.model_effects = add(self.usage.model_effects, u64::from(settled))?;
            self.usage.input_tokens =
                add_optional(self.usage.input_tokens, usage.and_then(Usage::input_tokens))?;
            self.usage.output_tokens = add_optional(
                self.usage.output_tokens,
                usage.and_then(Usage::output_tokens),
            )?;
            self.usage.total_tokens =
                add_optional(self.usage.total_tokens, usage.and_then(Usage::total_tokens))?;
        }
        if let Some(cost) = usage.and_then(Usage::cost) {
            let total = self
                .usage
                .cost_by_unit
                .entry(Arc::from(cost.unit()))
                .or_default();
            *total = add(*total, cost.micros())?;
        } else if matches!(request.kind(), EffectKind::Model | EffectKind::Tool) && !child_dispatch
        {
            self.usage.uncosted_effects = add(self.usage.uncosted_effects, 1)?;
        }
        Ok(())
    }
    fn finish(mut self) -> MeasuredUsage {
        if !self.usage.complete {
            self.usage.input_tokens = None;
            self.usage.output_tokens = None;
            self.usage.total_tokens = None;
        }
        if self.usage.complete
            && self.usage.uncosted_effects == 0
            && self.usage.cost_by_unit.len() == 1
            && let Some((unit, micros)) = self.usage.cost_by_unit.first_key_value()
        {
            self.usage.cost = Some(MeasuredCost {
                unit: Arc::clone(unit),
                micros: *micros,
            });
        }
        self.usage
    }
}

pub(crate) fn verify_lock(
    expected: finstack_ai_kernel::Digest,
    result: ReconciledAttempt,
) -> Result<ReconciledAttempt, EvalError> {
    if result
        .accepted_lock_digest
        .is_some_and(|actual| actual != expected)
    {
        return Err(EvalError::new(
            crate::EVAL_SUBJECT_LOCK_MISMATCH,
            "recorded execution does not match its bound lock",
        ));
    }
    Ok(result)
}
