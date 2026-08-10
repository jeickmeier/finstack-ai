//! Authoritative decide, append, apply, and post-commit dispatch coordination.

use std::sync::Arc;

use finstack_ai_kernel::{
    AppendRequest, CommittedBatch, Decision, Diagnostic, EffectId, EventId, Kernel, KernelError,
    KernelInput, KernelState, Metadata, ModelTextDelta, OperationLocator, PendingModelEffect,
    PostCommitAction, ProviderHeartbeat, ReasoningDelta, RecordBody, RunEvent, RunEventBody,
    Sensitivity, Timestamp, TransitionEnv,
};
use thiserror::Error;

use crate::{
    AuthorizationContext, JournalStore, LoadRequest, LoadedSession, ModelProgress, PortFuture,
    PortObject, StoreError,
};

/// Stable run-local fault state owned by a commit coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunFault {
    /// Stable non-secret fault code.
    pub code: &'static str,
}

/// Result of one accepted coordinator submission.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    /// Atomic committed batch, absent for an empty duplicate decision.
    pub committed: Option<CommittedBatch>,
    /// Events derived transactionally from the committed batch.
    pub events: Arc<[RunEvent]>,
    /// Non-semantic diagnostics returned by the pure decision.
    pub diagnostics: Arc<[Diagnostic]>,
    /// Number of post-commit actions successfully dispatched.
    pub dispatched_actions: usize,
    /// Post-commit fault, when the durable boundary succeeded but dispatch could not.
    pub fault: Option<RunFault>,
}

/// Commit-loop failures that do not have a fully applied outcome to return.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommitCoordinatorError {
    /// Run was already faulted by an uncertain prior boundary.
    #[error("run faulted: {code}")]
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
    /// Pure kernel decision rejected the normalized input.
    #[error("kernel decision rejected input: {code}")]
    Decision {
        /// Stable kernel error code.
        code: &'static str,
    },
    /// A configured effect driver rejected a request before any append.
    #[error("model request rejected before commit: {code}")]
    ModelRequest {
        /// Stable adapter error code.
        code: Arc<str>,
    },
    /// The environment did not provide exactly one append identity.
    #[error("non-empty decision requires exactly one append batch id")]
    AppendBatchIdCardinality,
    /// A definite pre-commit store failure occurred.
    #[error(transparent)]
    Store(StoreError),
    /// Store/replay/apply uncertainty faulted the run.
    #[error("commit boundary faulted: {code}")]
    BoundaryFault {
        /// Stable fault code.
        code: &'static str,
    },
}

/// One-run coordinator for the authoritative commit-before-effect path.
pub struct CommitCoordinator {
    kernel: Kernel,
    store: Arc<dyn JournalStore>,
    next_transient_sequence: u64,
    fault: Option<RunFault>,
    dispatcher: Option<Arc<dyn PostCommitDispatcher>>,
}

impl CommitCoordinator {
    /// Construct an empty coordinator over a direct journal-store handle.
    #[must_use]
    pub fn new(store: Arc<dyn JournalStore>) -> Self {
        Self {
            kernel: Kernel::default(),
            store,
            next_transient_sequence: 0,
            fault: None,
            dispatcher: None,
        }
    }

    /// Reconstruct a coordinator solely by loading and replaying one session.
    ///
    /// # Errors
    ///
    /// Returns a boundary fault when load metadata or any committed batch is invalid.
    pub async fn recover(
        store: Arc<dyn JournalStore>,
        session_id: finstack_ai_kernel::SessionId,
    ) -> Result<Self, CommitCoordinatorError> {
        let loaded = store
            .load(LoadRequest { session_id })
            .await
            .map_err(CommitCoordinatorError::Store)?;
        let (kernel, next_transient_sequence) = replay_loaded(&loaded)
            .map_err(|code| CommitCoordinatorError::BoundaryFault { code })?;
        Ok(Self {
            kernel,
            store,
            next_transient_sequence,
            fault: None,
            dispatcher: None,
        })
    }

    /// Borrow replay-derived semantic state.
    #[must_use]
    pub const fn state(&self) -> &KernelState {
        self.kernel.state()
    }

    /// Current run-local runtime fault.
    #[must_use]
    pub const fn fault(&self) -> Option<RunFault> {
        self.fault
    }

    #[cfg(feature = "native-tokio")]
    pub(crate) fn classify(
        &self,
        env: &TransitionEnv,
        input: KernelInput,
    ) -> Result<Decision, KernelError> {
        self.kernel.decide(env, input)
    }

    /// Submit one normalized command through the authoritative commit boundary.
    ///
    /// # Errors
    ///
    /// Returns a decision/store error before a certain commit, or a boundary fault
    /// when append acknowledgement, replay, or apply cannot be known safely.
    #[expect(
        clippy::too_many_lines,
        reason = "the authoritative boundary keeps retry, apply, and dispatch ordering visibly contiguous"
    )]
    pub async fn submit(
        &mut self,
        env: TransitionEnv,
        input: KernelInput,
    ) -> Result<CommitOutcome, CommitCoordinatorError> {
        if let Some(fault) = self.fault {
            return Err(CommitCoordinatorError::Faulted { code: fault.code });
        }

        let mut decision = self
            .kernel
            .decide(&env, input.clone())
            .map_err(|error| decision_error(&error))?;
        if decision.records.is_empty() {
            return Ok(empty_outcome(decision));
        }
        if let Some(dispatcher) = &self.dispatcher {
            dispatcher.validate_before_commit(&input).map_err(|error| {
                CommitCoordinatorError::ModelRequest {
                    code: Arc::from(error.code),
                }
            })?;
        }
        let [append_batch_id] = env.ids.append_batch_ids() else {
            return Err(CommitCoordinatorError::AppendBatchIdCardinality);
        };

        let mut conflicts = 0_u8;
        loop {
            let session_id = decision
                .records
                .first()
                .map(finstack_ai_kernel::RecordDraft::session_id)
                .ok_or(CommitCoordinatorError::BoundaryFault {
                    code: "empty_nonduplicate_decision",
                })?;
            let frozen = AppendRequest::try_new(
                *append_batch_id,
                session_id,
                decision.expected_sequence,
                decision.records.clone(),
            )
            .map_err(|_| CommitCoordinatorError::BoundaryFault {
                code: "append_request_invalid",
            })?;

            let committed = match self.append_frozen(frozen.clone()).await {
                Ok(committed) => committed,
                Err(StoreError::Conflict { .. }) if conflicts == 0 => {
                    conflicts = 1;
                    let loaded = self
                        .store
                        .load(LoadRequest { session_id })
                        .await
                        .map_err(|_| self.boundary_fault("conflict_reload_failed"))?;
                    let (kernel, next_transient_sequence) =
                        replay_loaded(&loaded).map_err(|code| self.boundary_fault(code))?;
                    self.kernel = kernel;
                    self.next_transient_sequence = next_transient_sequence;
                    decision = self
                        .kernel
                        .decide(&env, input.clone())
                        .map_err(|error| decision_error(&error))?;
                    if decision.records.is_empty() {
                        return Ok(empty_outcome(decision));
                    }
                    continue;
                }
                Err(StoreError::Conflict { .. }) => {
                    return Err(self.boundary_fault("repeated_store_conflict"));
                }
                Err(StoreError::AmbiguousAcknowledgement) => {
                    return Err(self.boundary_fault("continued_ambiguous_acknowledgement"));
                }
                Err(error @ (StoreError::Corruption { .. } | StoreError::Integrity { .. })) => {
                    let _ = error;
                    return Err(self.boundary_fault("store_integrity_uncertain"));
                }
                Err(error) => return Err(CommitCoordinatorError::Store(error)),
            };

            if !committed_matches_request(&committed, &frozen) {
                return Err(self.boundary_fault("store_receipt_mismatch"));
            }
            let Ok(events) = self.kernel.apply(&committed, self.next_transient_sequence) else {
                return Err(self.boundary_fault("committed_batch_apply_failed"));
            };
            self.next_transient_sequence = self
                .next_transient_sequence
                .checked_add(
                    u64::try_from(events.len())
                        .map_err(|_| self.boundary_fault("transient_event_sequence_exhausted"))?,
                )
                .ok_or_else(|| self.boundary_fault("transient_event_sequence_exhausted"))?;

            let diagnostics: Arc<[Diagnostic]> = decision.diagnostics.into();
            let mut dispatched_actions = 0;
            for action in decision.actions {
                let dispatch_result = self
                    .dispatch_after_recheck(action, env.now, &committed)
                    .await;
                if let Err(code) = dispatch_result {
                    let fault = RunFault { code };
                    self.fault = Some(fault);
                    return Ok(CommitOutcome {
                        committed: Some(committed),
                        events,
                        diagnostics,
                        dispatched_actions,
                        fault: Some(fault),
                    });
                }
                dispatched_actions += 1;
            }
            return Ok(CommitOutcome {
                committed: Some(committed),
                events,
                diagnostics,
                dispatched_actions,
                fault: None,
            });
        }
    }

    async fn append_frozen(&self, frozen: AppendRequest) -> Result<CommittedBatch, StoreError> {
        match self.store.append(frozen.clone()).await {
            Err(StoreError::AmbiguousAcknowledgement) => self.store.append(frozen).await,
            result => result,
        }
    }

    async fn dispatch_after_recheck(
        &self,
        action: PostCommitAction,
        now: Timestamp,
        committed: &CommittedBatch,
    ) -> Result<(), &'static str> {
        if !action_is_authorized(self.kernel.state(), action, now, committed) {
            return Err("dispatch_precondition_failed");
        }
        let Some(dispatcher) = &self.dispatcher else {
            return Err("effect_driver_unavailable");
        };
        let dispatch = RuntimeDispatch {
            action,
            model: model_dispatch_seed(self.kernel.state(), action),
        };
        dispatcher
            .dispatch(dispatch)
            .await
            .map_err(|error| error.code)
    }

    fn boundary_fault(&mut self, code: &'static str) -> CommitCoordinatorError {
        self.fault = Some(RunFault { code });
        CommitCoordinatorError::BoundaryFault { code }
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn install_dispatcher(&mut self, dispatcher: Arc<dyn PostCommitDispatcher>) {
        self.dispatcher = Some(dispatcher);
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn materialize_model_progress(
        &mut self,
        progress: &[ModelProgress],
        provider: &str,
        now: Timestamp,
    ) -> Result<Arc<[RunEvent]>, &'static str> {
        let pending = self
            .kernel
            .state()
            .pending_model_effect
            .as_ref()
            .ok_or("model_progress_without_pending_effect")?;
        let state = self.kernel.state();
        let accepted = state
            .accepted
            .as_ref()
            .ok_or("model_progress_without_accepted_run")?;
        let session_id = state.session_id.ok_or("model_progress_without_session")?;
        let lane_id = state.lane_id.ok_or("model_progress_without_lane")?;
        let effect_id = pending.requested.effect_id();
        let mut events = Vec::with_capacity(progress.len());
        for (offset, item) in progress.iter().enumerate() {
            let transient_sequence = self
                .next_transient_sequence
                .checked_add(
                    u64::try_from(offset).map_err(|_| "transient_event_sequence_exhausted")?,
                )
                .ok_or("transient_event_sequence_exhausted")?;
            let body = match item {
                ModelProgress::Text(text) => RunEventBody::ModelTextDelta(
                    ModelTextDelta::try_new(text).map_err(|_| "model_progress_invalid")?,
                ),
                ModelProgress::Reasoning(text) => RunEventBody::ReasoningDelta(
                    ReasoningDelta::try_new(text).map_err(|_| "model_progress_invalid")?,
                ),
                ModelProgress::Heartbeat(metadata) => RunEventBody::ProviderHeartbeat(
                    ProviderHeartbeat::try_new(provider, Some(metadata.as_str()))
                        .map_err(|_| "model_progress_invalid")?,
                ),
            };
            events.push(
                RunEvent::try_transient(
                    finstack_ai_kernel::RUN_EVENT_SCHEMA_VERSION,
                    finstack_ai_kernel::RUN_EVENT_KIND_VERSION,
                    transient_event_id(effect_id, transient_sequence),
                    session_id,
                    lane_id,
                    accepted.run_id(),
                    Some(pending.turn_id),
                    Some(pending.model_request_id),
                    None,
                    Some(effect_id),
                    None,
                    transient_sequence,
                    now,
                    Sensitivity::Confidential,
                    body,
                )
                .map_err(|_| "model_progress_invalid")?,
            );
        }
        self.next_transient_sequence = self
            .next_transient_sequence
            .checked_add(
                u64::try_from(events.len()).map_err(|_| "transient_event_sequence_exhausted")?,
            )
            .ok_or("transient_event_sequence_exhausted")?;
        Ok(events.into())
    }

    #[cfg(test)]
    fn with_test_dispatcher(
        store: Arc<dyn JournalStore>,
        dispatcher: Arc<dyn PostCommitDispatcher>,
    ) -> Self {
        let mut coordinator = Self::new(store);
        coordinator.install_dispatcher(dispatcher);
        coordinator
    }
}

#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
fn transient_event_id(effect_id: EffectId, transient_sequence: u64) -> EventId {
    let mut bytes = effect_id.to_bytes();
    let digest = finstack_ai_kernel::Digest::raw_json(
        format!("{}:{transient_sequence}", effect_id.to_canonical_string()).as_bytes(),
    );
    bytes[9..].copy_from_slice(&digest.as_bytes()[..7]);
    EventId::from_bytes(bytes)
}

fn decision_error(error: &KernelError) -> CommitCoordinatorError {
    CommitCoordinatorError::Decision { code: error.code() }
}

fn empty_outcome(decision: Decision) -> CommitOutcome {
    CommitOutcome {
        committed: None,
        events: Arc::from([]),
        diagnostics: decision.diagnostics.into(),
        dispatched_actions: 0,
        fault: None,
    }
}

fn replay_loaded(loaded: &LoadedSession) -> Result<(Kernel, u64), &'static str> {
    let mut kernel = Kernel::default();
    let mut next_transient_sequence = 0_u64;
    let mut last_batch_sequence = 0_u64;
    for batch in loaded.committed_batches.iter() {
        let events = kernel
            .apply(batch, next_transient_sequence)
            .map_err(|_| "journal_replay_failed")?;
        next_transient_sequence = next_transient_sequence
            .checked_add(
                u64::try_from(events.len()).map_err(|_| "transient_event_sequence_exhausted")?,
            )
            .ok_or("transient_event_sequence_exhausted")?;
        last_batch_sequence = batch.last_sequence;
    }
    if last_batch_sequence != loaded.head_sequence
        || kernel.state().last_applied_sequence != loaded.head_sequence
    {
        return Err("loaded_head_mismatch");
    }
    Ok((kernel, next_transient_sequence))
}

fn committed_matches_request(committed: &CommittedBatch, request: &AppendRequest) -> bool {
    if committed.batch_id != request.batch_id()
        || committed.first_sequence != request.expected_sequence()
        || committed.records.len() != request.records().len()
    {
        return false;
    }
    let expected_last = request
        .expected_sequence()
        .checked_add(u64::try_from(request.records().len().saturating_sub(1)).unwrap_or(u64::MAX));
    if expected_last != Some(committed.last_sequence) {
        return false;
    }
    committed
        .records
        .iter()
        .zip(request.records())
        .all(|(record, draft)| {
            record.record_id() == draft.record_id()
                && record.session_id() == draft.session_id()
                && record.lane_id() == draft.lane_id()
                && record.run_id() == draft.run_id()
                && record.timestamp() == draft.timestamp()
                && record.derived_event_ids() == draft.derived_event_ids()
                && record.body() == draft.body()
        })
}

fn action_is_authorized(
    state: &KernelState,
    action: PostCommitAction,
    now: Timestamp,
    committed: &CommittedBatch,
) -> bool {
    if state.terminal.is_some() {
        return false;
    }
    let effect_id = match action {
        PostCommitAction::ExecuteEffect { effect_id }
        | PostCommitAction::CancelEffect { effect_id } => effect_id,
    };
    match action {
        PostCommitAction::ExecuteEffect { .. } => {
            if state.cancellation.is_some() {
                return false;
            }
            committed_effect_request(committed, effect_id)
                .or_else(|| pending_effect_request(state, effect_id))
                .is_some_and(|request| request.deadline().is_none_or(|deadline| deadline > now))
        }
        PostCommitAction::CancelEffect { .. } => state
            .cancellation
            .as_ref()
            .is_some_and(|value| value.outstanding_effects.contains(&effect_id)),
    }
}

fn committed_effect_request(
    committed: &CommittedBatch,
    effect_id: EffectId,
) -> Option<&finstack_ai_kernel::EffectRequested> {
    committed.records.iter().find_map(|record| {
        let RecordBody::EffectRequested(request) = record.body() else {
            return None;
        };
        (request.effect_id() == effect_id).then_some(request)
    })
}

fn pending_effect_request(
    state: &KernelState,
    effect_id: EffectId,
) -> Option<&finstack_ai_kernel::EffectRequested> {
    if let Some(pending) = &state.pending_model_effect
        && pending.requested.effect_id() == effect_id
        && pending.deferred.is_none()
    {
        return Some(&pending.requested);
    }
    state.active_tool_batch.as_ref().and_then(|batch| {
        batch.calls.iter().find_map(|call| {
            let finstack_ai_kernel::ActiveToolCallStatus::Requested {
                requested,
                deferred,
            } = &call.status
            else {
                return None;
            };
            (requested.effect_id() == effect_id && deferred.is_none()).then_some(requested)
        })
    })
}

pub(crate) struct DispatchError {
    pub(crate) code: &'static str,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct ModelDispatchSeed {
    pub(crate) pending: PendingModelEffect,
    pub(crate) locator: OperationLocator,
    pub(crate) authorization: AuthorizationContext,
    pub(crate) budget_scope_id: Option<finstack_ai_kernel::BudgetScopeId>,
    pub(crate) attempt: u32,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct RuntimeDispatch {
    pub(crate) action: PostCommitAction,
    pub(crate) model: Option<ModelDispatchSeed>,
}

pub(crate) trait PostCommitDispatcher: PortObject {
    fn validate_before_commit(&self, _input: &KernelInput) -> Result<(), DispatchError> {
        Ok(())
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>>;
}

fn model_dispatch_seed(state: &KernelState, action: PostCommitAction) -> Option<ModelDispatchSeed> {
    let PostCommitAction::ExecuteEffect { effect_id } = action else {
        return None;
    };
    let pending = state.pending_model_effect.as_ref()?;
    if pending.requested.effect_id() != effect_id || pending.deferred.is_some() {
        return None;
    }
    let accepted = state.accepted.as_ref()?;
    let security = accepted.security();
    let locator = OperationLocator::try_new(
        security.tenant_scope(),
        state.session_id?,
        state.lane_id?,
        accepted.run_id(),
    )
    .ok()?;
    let authorization = AuthorizationContext {
        principal: security.principal().clone(),
        authentication_method: Arc::from(security.authentication_method()),
        assurance_level: Arc::from(security.assurance_level()),
        roles: Arc::from([]),
        permitted_scopes: Arc::from([Arc::from(security.tenant_scope())]),
        safe_claims: Metadata::empty(),
        policy_version: Arc::from(security.authorization_policy_version()),
        decision_id: Arc::from(security.authorization_decision_id()),
    };
    Some(ModelDispatchSeed {
        pending: pending.clone(),
        locator,
        authorization,
        budget_scope_id: accepted.relation().budget_scope_id(),
        attempt: state.retry.attempts.checked_add(1)?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, ContentBlock,
        DeadlinePropagation, Digest, EffectOutputContract, EffectOutputKind, Id, IdTag, LaneTag,
        Message, MessageRole, Metadata, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson,
        RecordEnvelope, ReducerStageOutcome, RetrySafety, RunAccepted, RunLimits,
        RunPropagationPolicy, RunRelation, RunSecurityContext, SessionTag, Stage, StageCursor,
        TextBlock, Timestamp,
    };

    use super::*;
    use crate::{LoadedSession, SnapshotReceipt, SnapshotRequest, StoreHealth};

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn timestamp(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    #[allow(clippy::too_many_arguments)]
    fn env(
        now: i64,
        records: &[u64],
        events: &[u64],
        effects: &[u64],
        turns: &[u64],
        model_requests: &[u64],
        messages: &[u64],
        append_batch: u64,
    ) -> TransitionEnv {
        TransitionEnv {
            now: timestamp(now),
            ids: AllocatedIds::try_new(
                records.iter().copied().map(id).collect(),
                events.iter().copied().map(id).collect(),
                effects.iter().copied().map(id).collect(),
                Vec::new(),
                messages.iter().copied().map(id).collect(),
                turns.iter().copied().map(id).collect(),
                model_requests.iter().copied().map(id).collect(),
                Vec::new(),
                Vec::new(),
                vec![id(append_batch)],
                Vec::new(),
            )
            .expect("allocated ids"),
        }
    }

    fn acceptance() -> RunAccepted {
        let run_id = id(3);
        RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"agent":"fixture"}"#),
            None,
        )
        .expect("acceptance")
    }

    fn accept_input() -> KernelInput {
        KernelInput::AcceptRun(AcceptRun {
            session_id: id::<SessionTag>(1),
            lane_id: id::<LaneTag>(2),
            accepted: acceptance(),
        })
    }

    fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
        KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
            cursor: StageCursor { cycle: 0, stage },
            outcome,
        })
    }

    fn context_message() -> Message {
        Message::try_new(
            id(4),
            MessageRole::User,
            vec![ContentBlock::Text(
                TextBlock::try_new("Say hello.").expect("text"),
            )],
            timestamp(900),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn output_contract() -> EffectOutputContract {
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
        }
    }

    async fn drive_to_model_request(coordinator: &mut CommitCoordinator) -> CommitOutcome {
        coordinator
            .submit(
                env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
                accept_input(),
            )
            .await
            .expect("accept");
        coordinator
            .submit(
                env(1_100, &[2], &[], &[], &[], &[], &[], 102),
                stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
            )
            .await
            .expect("before run");
        coordinator
            .submit(
                env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
                stage(
                    Stage::PrepareContext,
                    ReducerStageOutcome::ContextPrepared {
                        messages: Arc::from([context_message()]),
                    },
                ),
            )
            .await
            .expect("context");
        coordinator
            .submit(
                env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
                stage(
                    Stage::BeforeModel,
                    ReducerStageOutcome::ModelRequestPrepared {
                        request: RawJson::parse(
                            r#"{"messages":[{"role":"user","text":"Say hello."}]}"#,
                        )
                        .expect("request"),
                        component: None,
                        output_contract: output_contract(),
                        retry_safety: RetrySafety::SafeToRetry,
                        deadline: Some(timestamp(8_000)),
                    },
                ),
            )
            .await
            .expect("model request")
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FakeMode {
        Normal,
        ConflictOnce,
        ConflictAlways,
        AmbiguousOnce,
        AmbiguousAlways,
    }

    struct FakeStore {
        inner: Mutex<FakeInner>,
    }

    struct FakeInner {
        mode: FakeMode,
        append_calls: usize,
        batches: Vec<CommittedBatch>,
        requests: BTreeMap<finstack_ai_kernel::AppendBatchId, AppendRequest>,
    }

    impl FakeStore {
        fn new(mode: FakeMode) -> Self {
            Self {
                inner: Mutex::new(FakeInner {
                    mode,
                    append_calls: 0,
                    batches: Vec::new(),
                    requests: BTreeMap::new(),
                }),
            }
        }

        fn append_calls(&self) -> usize {
            self.inner.lock().expect("lock").append_calls
        }
    }

    impl JournalStore for FakeStore {
        fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
            let mut inner = self.inner.lock().expect("lock");
            inner.append_calls += 1;
            if let Some(existing) = inner.requests.get(&request.batch_id()) {
                if existing != &request {
                    return Box::pin(async {
                        Err(StoreError::Corruption {
                            reason_code: "batch_reuse",
                        })
                    });
                }
                let committed = inner
                    .batches
                    .iter()
                    .find(|batch| batch.batch_id == request.batch_id())
                    .cloned()
                    .expect("indexed batch");
                if inner.mode == FakeMode::AmbiguousAlways {
                    return Box::pin(async { Err(StoreError::AmbiguousAcknowledgement) });
                }
                return Box::pin(async move { Ok(committed) });
            }
            if inner.mode == FakeMode::ConflictAlways
                || (inner.mode == FakeMode::ConflictOnce && inner.append_calls == 1)
            {
                return Box::pin(async {
                    Err(StoreError::Conflict {
                        expected_sequence: 1,
                        actual_next_sequence: 1,
                    })
                });
            }
            let committed = commit_request(&request);
            inner.requests.insert(request.batch_id(), request);
            inner.batches.push(committed.clone());
            if matches!(
                inner.mode,
                FakeMode::AmbiguousOnce | FakeMode::AmbiguousAlways
            ) {
                return Box::pin(async { Err(StoreError::AmbiguousAcknowledgement) });
            }
            Box::pin(async move { Ok(committed) })
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            let inner = self.inner.lock().expect("lock");
            let batches = inner
                .batches
                .iter()
                .filter(|batch| {
                    batch
                        .records
                        .first()
                        .is_some_and(|record| record.session_id() == request.session_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
            Box::pin(async move {
                Ok(LoadedSession {
                    session_id: request.session_id,
                    head_sequence,
                    committed_batches: batches.into(),
                    snapshot: None,
                })
            })
        }

        fn write_snapshot(
            &self,
            _request: SnapshotRequest,
        ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "not_used",
                })
            })
        }

        fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
            Box::pin(async {
                Ok(StoreHealth {
                    ready: true,
                    durable: false,
                    detail: Arc::from("test"),
                })
            })
        }
    }

    fn commit_request(request: &AppendRequest) -> CommittedBatch {
        let records = request
            .records()
            .iter()
            .enumerate()
            .map(|(offset, draft)| {
                let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
                RecordEnvelope::try_new(
                    draft.format_version(),
                    draft.kind_version(),
                    draft.record_id(),
                    draft.session_id(),
                    draft.lane_id(),
                    draft.run_id(),
                    sequence,
                    draft.timestamp(),
                    None,
                    Digest::raw_json(format!("payload-{sequence}").as_bytes()),
                    None,
                    Digest::raw_json(format!("checksum-{sequence}").as_bytes()),
                    draft.derived_event_ids().to_vec(),
                    draft.body().clone(),
                )
                .expect("envelope")
            })
            .collect::<Vec<_>>();
        CommittedBatch::try_new(
            request.batch_id(),
            request.expected_sequence(),
            request.expected_sequence() + u64::try_from(records.len()).expect("count") - 1,
            records,
        )
        .expect("batch")
    }

    #[derive(Default)]
    struct RecordingDispatcher {
        actions: Mutex<Vec<PostCommitAction>>,
    }

    impl PostCommitDispatcher for RecordingDispatcher {
        fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
            self.actions.lock().expect("lock").push(dispatch.action);
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn append_and_apply_precede_test_dispatch() {
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let mut coordinator =
            CommitCoordinator::with_test_dispatcher(store.clone(), dispatcher.clone());
        let outcome = block_on(drive_to_model_request(&mut coordinator));
        assert!(outcome.committed.is_some());
        assert_eq!(outcome.dispatched_actions, 1);
        assert!(outcome.fault.is_none());
        assert_eq!(store.append_calls(), 4);
        assert_eq!(dispatcher.actions.lock().expect("lock").len(), 1);
        assert_eq!(
            coordinator.state().phase,
            Some(finstack_ai_kernel::RunPhase::AwaitingModel)
        );
    }

    #[test]
    fn unsupported_driver_faults_only_after_committed_request() {
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let mut coordinator = CommitCoordinator::new(store.clone());
        let outcome = block_on(drive_to_model_request(&mut coordinator));
        assert_eq!(
            outcome.fault,
            Some(RunFault {
                code: "effect_driver_unavailable"
            })
        );
        assert_eq!(store.append_calls(), 4);
        assert!(matches!(
            block_on(coordinator.submit(
                env(1_400, &[], &[], &[], &[], &[], &[], 105),
                stage(Stage::AfterModel, ReducerStageOutcome::Continue),
            )),
            Err(CommitCoordinatorError::Faulted {
                code: "effect_driver_unavailable"
            })
        ));
    }

    #[test]
    fn duplicate_decision_skips_append_and_recovery_matches_live_state() {
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let mut coordinator = CommitCoordinator::new(store.clone());
        let accepted_env = env(1_000, &[1], &[1], &[], &[], &[], &[], 101);
        block_on(coordinator.submit(accepted_env, accept_input())).expect("accept");
        let stage_env = env(1_100, &[2], &[], &[], &[], &[], &[], 102);
        let stage_input = stage(Stage::BeforeRun, ReducerStageOutcome::Continue);
        block_on(coordinator.submit(stage_env.clone(), stage_input.clone())).expect("before run");
        let calls = store.append_calls();
        let duplicate = block_on(coordinator.submit(stage_env, stage_input)).expect("duplicate");
        assert!(duplicate.committed.is_none());
        assert_eq!(store.append_calls(), calls);
        let live_hash = coordinator.state().state_hash().expect("hash");
        let recovered =
            block_on(CommitCoordinator::recover(store, id::<SessionTag>(1))).expect("recover");
        assert_eq!(recovered.state().state_hash().expect("hash"), live_hash);
    }

    #[test]
    fn one_conflict_reloads_and_repeated_conflict_faults() {
        let once = Arc::new(FakeStore::new(FakeMode::ConflictOnce));
        let mut coordinator = CommitCoordinator::new(once.clone());
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        ))
        .expect("retry after conflict");
        assert_eq!(once.append_calls(), 2);

        let repeated = Arc::new(FakeStore::new(FakeMode::ConflictAlways));
        let mut coordinator = CommitCoordinator::new(repeated);
        assert!(matches!(
            block_on(coordinator.submit(
                env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
                accept_input(),
            )),
            Err(CommitCoordinatorError::BoundaryFault {
                code: "repeated_store_conflict"
            })
        ));
    }

    #[test]
    fn ambiguous_ack_retries_identical_request_once() {
        let once = Arc::new(FakeStore::new(FakeMode::AmbiguousOnce));
        let mut coordinator = CommitCoordinator::new(once.clone());
        block_on(coordinator.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        ))
        .expect("ambiguous recovery");
        assert_eq!(once.append_calls(), 2);

        let repeated = Arc::new(FakeStore::new(FakeMode::AmbiguousAlways));
        let mut coordinator = CommitCoordinator::new(repeated);
        assert!(matches!(
            block_on(coordinator.submit(
                env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
                accept_input(),
            )),
            Err(CommitCoordinatorError::BoundaryFault {
                code: "continued_ambiguous_acknowledgement"
            })
        ));
    }
}
