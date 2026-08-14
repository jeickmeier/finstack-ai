//! Authoritative decide, append, apply, and post-commit dispatch coordination.

use std::sync::Arc;

use finstack_ai_kernel::{
    ActiveToolCallStatus, AppendBatchId, AppendRequest, CommittedBatch, Decision, Diagnostic,
    EffectId, EffectRequested, EventId, Kernel, KernelError, KernelInput, KernelState, Metadata,
    ModelTextDelta, OperationLocator, PendingModelEffect, PostCommitAction, ProviderHeartbeat,
    ReasoningDelta, RecordBody, RecordDraft, RunEvent, RunEventBody, Sensitivity, Timestamp,
    ToolBatchId, ToolCallId, ToolProgress, TransitionEnv, ValidatedToolCall,
};
use thiserror::Error;

use crate::{
    AcceleratedRestore, AuthorizationContext, JournalStore, LoadRequest, LoadedSession,
    ModelProgress, PortFuture, PortObject, SnapshotSchedule, StateSnapshotRequest, StoreError,
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
    /// A durable composition identity already exists with different content.
    #[error("durable composition sidecar conflicts with existing identity")]
    SidecarConflict,
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
    /// The installed runtime event hub failed after apply and before dispatch.
    #[error("event delivery faulted: {code}")]
    EventDelivery {
        /// Stable event-delivery error code.
        code: &'static str,
    },
}

/// One-run coordinator for the authoritative commit-before-effect path.
pub struct CommitCoordinator {
    kernel: Kernel,
    store: Arc<dyn JournalStore>,
    next_transient_sequence: u64,
    pending_timer_scheduled_at: Option<Timestamp>,
    snapshot_schedule: SnapshotSchedule,
    last_snapshot_sequence: Option<u64>,
    fault: Option<RunFault>,
    dispatcher: Option<Arc<dyn PostCommitDispatcher>>,
    #[cfg(feature = "native-tokio")]
    manual_drive: Option<crate::manual_drive::ManualDriveGate>,
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    event_publisher: Option<Arc<dyn crate::event_hub::RuntimeEventPublisher>>,
}

impl CommitCoordinator {
    /// Construct an empty coordinator over a direct journal-store handle.
    #[must_use]
    pub fn new(store: Arc<dyn JournalStore>) -> Self {
        Self {
            kernel: Kernel::default(),
            store,
            next_transient_sequence: 0,
            pending_timer_scheduled_at: None,
            snapshot_schedule: SnapshotSchedule::default(),
            last_snapshot_sequence: None,
            fault: None,
            dispatcher: None,
            #[cfg(feature = "native-tokio")]
            manual_drive: None,
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            event_publisher: None,
        }
    }

    /// Replace the default snapshot write policy.
    #[must_use]
    pub fn with_snapshot_schedule(mut self, schedule: SnapshotSchedule) -> Self {
        self.snapshot_schedule = schedule;
        self
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
        let (kernel, next_transient_sequence, pending_timer_scheduled_at, used_snapshot) =
            replay_loaded(&loaded)
                .map_err(|code| CommitCoordinatorError::BoundaryFault { code })?;
        Ok(Self {
            kernel,
            store,
            next_transient_sequence,
            pending_timer_scheduled_at,
            snapshot_schedule: SnapshotSchedule::default(),
            last_snapshot_sequence: used_snapshot
                .then(|| {
                    loaded
                        .accelerated
                        .as_ref()
                        .map(|snapshot| snapshot.sequence)
                })
                .flatten(),
            fault: None,
            dispatcher: None,
            #[cfg(feature = "native-tokio")]
            manual_drive: None,
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            event_publisher: None,
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
    pub(crate) fn pending_timer_seed(&self) -> Option<TimerDispatchSeed> {
        Some(TimerDispatchSeed {
            scheduled: self.kernel.state().retry.pending.clone()?,
            scheduled_at: self.pending_timer_scheduled_at?,
        })
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn pending_model_seed(&self) -> Option<ModelDispatchSeed> {
        let state = self.kernel.state();
        let pending = state.pending_model_effect.clone()?;
        let (locator, authorization, budget_scope_id) = dispatch_security_context(state)?;
        Some(ModelDispatchSeed {
            pending,
            locator,
            authorization,
            budget_scope_id,
            attempt: state.retry.attempts.checked_add(1)?,
            requested_at: state.accepted_at?,
        })
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn pending_tool_seeds(&self) -> Vec<ToolDispatchSeed> {
        let state = self.kernel.state();
        let Some(batch) = state.active_tool_batch.as_ref() else {
            return Vec::new();
        };
        let Some((locator, authorization, budget_scope_id)) = dispatch_security_context(state)
        else {
            return Vec::new();
        };
        let Some(requested_at) = state.accepted_at else {
            return Vec::new();
        };
        batch
            .calls
            .iter()
            .filter_map(|call| {
                let ActiveToolCallStatus::Requested {
                    requested,
                    deferred: None,
                } = &call.status
                else {
                    return None;
                };
                let finstack_ai_kernel::ToolCallPlan::Execute(validated) = &call.assigned.plan
                else {
                    return None;
                };
                Some(ToolDispatchSeed {
                    requested: requested.clone(),
                    tool_batch_id: batch.opened.tool_batch_id,
                    tool_call_id: *validated.call.tool_call_id(),
                    call: validated.clone(),
                    locator: locator.clone(),
                    authorization: authorization.clone(),
                    budget_scope_id,
                    attempt: 1,
                    requested_at,
                })
            })
            .collect()
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
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
                    let (
                        kernel,
                        next_transient_sequence,
                        pending_timer_scheduled_at,
                        used_snapshot,
                    ) = replay_loaded(&loaded).map_err(|code| self.boundary_fault(code))?;
                    self.kernel = kernel;
                    self.next_transient_sequence = next_transient_sequence;
                    self.pending_timer_scheduled_at = pending_timer_scheduled_at;
                    self.last_snapshot_sequence = used_snapshot
                        .then(|| {
                            loaded
                                .accelerated
                                .as_ref()
                                .map(|snapshot| snapshot.sequence)
                        })
                        .flatten();
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
            update_pending_timer_timestamp(&mut self.pending_timer_scheduled_at, &committed);
            self.next_transient_sequence = self
                .next_transient_sequence
                .checked_add(
                    u64::try_from(events.len())
                        .map_err(|_| self.boundary_fault("transient_event_sequence_exhausted"))?,
                )
                .ok_or_else(|| self.boundary_fault("transient_event_sequence_exhausted"))?;

            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            self.publish_events(Arc::clone(&events)).await?;

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
            self.maybe_write_snapshot(&committed).await;
            return Ok(CommitOutcome {
                committed: Some(committed),
                events,
                diagnostics,
                dispatched_actions,
                fault: None,
            });
        }
    }

    async fn maybe_write_snapshot(&mut self, committed: &CommittedBatch) {
        if self.snapshot_schedule.every_n_records == 0 {
            return;
        }
        let head = self.kernel.state().last_applied_sequence;
        let last = self.last_snapshot_sequence.unwrap_or(0);
        if head.saturating_sub(last) < self.snapshot_schedule.every_n_records {
            return;
        }
        let Some(record) = committed.records.last() else {
            return;
        };
        let request = StateSnapshotRequest {
            session_id: record.session_id(),
            state: self.kernel.state().clone(),
            head_checksum: record.checksum(),
            pending_timer_scheduled_at: self.pending_timer_scheduled_at,
        };
        let write = self.store.write_state_snapshot(request);
        #[cfg(feature = "native-tokio")]
        let accepted = matches!(
            tokio::time::timeout(self.snapshot_schedule.write_timeout, write).await,
            Ok(Ok(_))
        );
        #[cfg(not(feature = "native-tokio"))]
        let accepted = write.await.is_ok();
        if accepted {
            self.last_snapshot_sequence = Some(head);
        }
    }

    async fn append_frozen(&self, frozen: AppendRequest) -> Result<CommittedBatch, StoreError> {
        match self.store.append(frozen.clone()).await {
            Err(StoreError::AmbiguousAcknowledgement) => self.store.append(frozen).await,
            result => result,
        }
    }

    /// Commit zero-event composition records through the same append/apply boundary.
    ///
    /// This is intentionally narrower than general effect dispatch: it accepts
    /// only PR-022 child/budget sidecar records and never calls an external service.
    /// A sequence race reloads only the known session; equal durable identities
    /// converge and different content fails closed.
    ///
    /// # Errors
    ///
    /// Returns a store/boundary failure, or [`CommitCoordinatorError::SidecarConflict`]
    /// for conflicting durable idempotency reuse.
    pub(crate) async fn commit_composition_records(
        &mut self,
        batch_id: AppendBatchId,
        records: Vec<RecordDraft>,
    ) -> Result<Option<CommittedBatch>, CommitCoordinatorError> {
        if records.is_empty()
            || records.iter().any(|record| {
                !record.derived_event_ids().is_empty()
                    || !matches!(
                        record.body(),
                        RecordBody::ChildRunPrepared(_)
                            | RecordBody::BudgetReservationRequested(_)
                            | RecordBody::BudgetReservationSettled(_)
                            | RecordBody::BudgetChargeRecorded(_)
                            | RecordBody::BudgetReservationReleased(_)
                    )
            })
        {
            return Err(CommitCoordinatorError::BoundaryFault {
                code: "composition_records_invalid",
            });
        }
        let session_id = records[0].session_id();
        let mut conflicts = 0_u8;
        loop {
            match classify_composition_records(self.kernel.state(), &records) {
                CompositionRecordStatus::Equal => return Ok(None),
                CompositionRecordStatus::Conflict => {
                    return Err(CommitCoordinatorError::SidecarConflict);
                }
                CompositionRecordStatus::Absent => {}
            }
            let expected_sequence = self
                .kernel
                .state()
                .last_applied_sequence
                .checked_add(1)
                .ok_or_else(|| self.boundary_fault("composition_sequence_overflow"))?;
            let request =
                AppendRequest::try_new(batch_id, session_id, expected_sequence, records.clone())
                    .map_err(|_| CommitCoordinatorError::BoundaryFault {
                        code: "composition_append_request_invalid",
                    })?;
            let committed = match self.append_frozen(request).await {
                Ok(committed) => committed,
                Err(StoreError::Conflict { .. }) if conflicts == 0 => {
                    conflicts = 1;
                    let loaded = self
                        .store
                        .load(LoadRequest { session_id })
                        .await
                        .map_err(|_| self.boundary_fault("composition_conflict_reload_failed"))?;
                    let (
                        kernel,
                        next_transient_sequence,
                        pending_timer_scheduled_at,
                        used_snapshot,
                    ) = replay_loaded(&loaded).map_err(|code| self.boundary_fault(code))?;
                    self.kernel = kernel;
                    self.next_transient_sequence = next_transient_sequence;
                    self.pending_timer_scheduled_at = pending_timer_scheduled_at;
                    self.last_snapshot_sequence = used_snapshot
                        .then(|| {
                            loaded
                                .accelerated
                                .as_ref()
                                .map(|snapshot| snapshot.sequence)
                        })
                        .flatten();
                    continue;
                }
                Err(StoreError::Conflict { .. }) => {
                    return Err(CommitCoordinatorError::Store(StoreError::Conflict {
                        expected_sequence,
                        actual_next_sequence: self
                            .kernel
                            .state()
                            .last_applied_sequence
                            .saturating_add(1),
                    }));
                }
                Err(error) => return Err(CommitCoordinatorError::Store(error)),
            };
            let events = self
                .kernel
                .apply(&committed, self.next_transient_sequence)
                .map_err(|_| self.boundary_fault("composition_apply_failed"))?;
            if !events.is_empty() {
                return Err(self.boundary_fault("composition_emitted_events"));
            }
            return Ok(Some(committed));
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
            model: model_dispatch_seed(self.kernel.state(), action, committed),
            tool: tool_dispatch_seed(self.kernel.state(), action, committed),
            timer: timer_dispatch_seed(
                self.kernel.state(),
                action,
                self.pending_timer_scheduled_at,
            ),
        };
        #[cfg(feature = "native-tokio")]
        if let Some(manual_drive) = &self.manual_drive {
            manual_drive.pause(action).await?;
        }
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

    /// Pause every committed execute/cancel action immediately before external dispatch.
    ///
    /// This deterministic native test mode preserves normal validation, append,
    /// apply, event publication, and dispatch precondition checks. The returned
    /// exclusive controller must explicitly release each action.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ManualDriveError::ZeroCapacity`] when `capacity` is zero.
    #[cfg(feature = "native-tokio")]
    pub fn enable_manual_drive(
        &mut self,
        capacity: usize,
    ) -> Result<crate::ManualDriveController, crate::ManualDriveError> {
        let (gate, controller) = crate::manual_drive::manual_drive(capacity)?;
        self.manual_drive = Some(gate);
        Ok(controller)
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn install_event_publisher(
        &mut self,
        publisher: Arc<dyn crate::event_hub::RuntimeEventPublisher>,
    ) {
        self.event_publisher = Some(publisher);
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) async fn publish_events(
        &mut self,
        events: Arc<[RunEvent]>,
    ) -> Result<(), CommitCoordinatorError> {
        let Some(publisher) = self.event_publisher.clone() else {
            return Ok(());
        };
        publisher.publish(events).await.map_err(|error| {
            self.fault = Some(RunFault { code: error.code });
            CommitCoordinatorError::EventDelivery { code: error.code }
        })
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn materialize_model_progress(
        &mut self,
        progress: &ModelProgress,
        event_id: EventId,
        provider: &str,
        now: Timestamp,
    ) -> Result<RunEvent, &'static str> {
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
        let transient_sequence = self.next_transient_sequence;
        let body = match progress {
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
        let event = RunEvent::try_transient(
            finstack_ai_kernel::RUN_EVENT_SCHEMA_VERSION,
            finstack_ai_kernel::RUN_EVENT_KIND_VERSION,
            event_id,
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
        .map_err(|_| "model_progress_invalid")?;
        self.next_transient_sequence = self
            .next_transient_sequence
            .checked_add(1)
            .ok_or("transient_event_sequence_exhausted")?;
        Ok(event)
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn materialize_tool_progress(
        &mut self,
        progress: &ToolProgress,
        event_id: EventId,
        effect_id: EffectId,
        now: Timestamp,
    ) -> Result<RunEvent, &'static str> {
        let state = self.kernel.state();
        let accepted = state
            .accepted
            .as_ref()
            .ok_or("tool_progress_without_accepted_run")?;
        let session_id = state.session_id.ok_or("tool_progress_without_session")?;
        let lane_id = state.lane_id.ok_or("tool_progress_without_lane")?;
        let batch = state
            .active_tool_batch
            .as_ref()
            .ok_or("tool_progress_without_active_batch")?;
        let call = batch
            .calls
            .iter()
            .find(|call| call.assigned.effect_id == effect_id)
            .ok_or("tool_progress_without_active_call")?;
        let run_id = accepted.run_id();
        let turn_id = batch.opened.turn_id;
        let tool_batch_id = batch.opened.tool_batch_id;
        let tool_call_id = *call.assigned.plan.call().tool_call_id();
        let transient_sequence = self.next_transient_sequence;
        let event = RunEvent::try_transient(
            finstack_ai_kernel::RUN_EVENT_SCHEMA_VERSION,
            finstack_ai_kernel::RUN_EVENT_KIND_VERSION,
            event_id,
            session_id,
            lane_id,
            run_id,
            Some(turn_id),
            None,
            Some(tool_batch_id),
            Some(effect_id),
            Some(tool_call_id),
            transient_sequence,
            now,
            Sensitivity::Confidential,
            RunEventBody::ToolProgress(progress.clone()),
        )
        .map_err(|_| "tool_progress_invalid")?;
        self.next_transient_sequence = self
            .next_transient_sequence
            .checked_add(1)
            .ok_or("transient_event_sequence_exhausted")?;
        Ok(event)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompositionRecordStatus {
    Absent,
    Equal,
    Conflict,
}

fn classify_composition_records(
    state: &KernelState,
    records: &[RecordDraft],
) -> CompositionRecordStatus {
    let mut present = 0_usize;
    for record in records {
        let comparison = match record.body() {
            RecordBody::ChildRunPrepared(value) => state
                .child_preparations
                .get(&value.parent_effect_id)
                .map(|existing| existing == value),
            RecordBody::BudgetReservationRequested(value) => state
                .budget_reservations
                .get(&value.request.reservation_id)
                .map(|existing| existing.request == value.request),
            RecordBody::BudgetReservationSettled(value) => state
                .budget_reservations
                .get(&value.receipt.reservation_id)
                .and_then(|existing| existing.settlement.as_ref())
                .map(|existing| existing == &value.receipt),
            RecordBody::BudgetChargeRecorded(value) => state
                .budget_charges
                .get(&value.receipt.effect_id)
                .map(|existing| existing == &value.receipt),
            RecordBody::BudgetReservationReleased(value) => state
                .budget_reservations
                .get(&value.receipt.reservation_id)
                .and_then(|existing| existing.release.as_ref())
                .map(|existing| existing == &value.receipt),
            _ => return CompositionRecordStatus::Conflict,
        };
        match comparison {
            Some(true) => present += 1,
            Some(false) => return CompositionRecordStatus::Conflict,
            None => {}
        }
    }
    if present == records.len() {
        CompositionRecordStatus::Equal
    } else {
        CompositionRecordStatus::Absent
    }
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

fn replay_loaded(
    loaded: &LoadedSession,
) -> Result<(Kernel, u64, Option<Timestamp>, bool), &'static str> {
    if let Some(accelerated) = loaded.accelerated.as_ref()
        && let Ok((kernel, next_transient_sequence, pending_timer_scheduled_at)) =
            replay_from_snapshot(loaded, accelerated)
    {
        return Ok((
            kernel,
            next_transient_sequence,
            pending_timer_scheduled_at,
            true,
        ));
    }
    let (kernel, next_transient_sequence, pending_timer_scheduled_at) =
        replay_from_default(loaded)?;
    Ok((
        kernel,
        next_transient_sequence,
        pending_timer_scheduled_at,
        false,
    ))
}

fn replay_from_snapshot(
    loaded: &LoadedSession,
    accelerated: &AcceleratedRestore,
) -> Result<(Kernel, u64, Option<Timestamp>), &'static str> {
    if accelerated.sequence > loaded.head_sequence
        || accelerated.sequence != accelerated.state.last_applied_sequence
    {
        return Err("snapshot_sequence_invalid");
    }
    let journal_checksum =
        checksum_at(loaded, accelerated.sequence).ok_or("snapshot_missing_record")?;
    if journal_checksum != accelerated.head_checksum {
        return Err("snapshot_checksum_mismatch");
    }
    let mut kernel =
        Kernel::try_restore(accelerated.state.clone()).map_err(|_| "snapshot_state_invalid")?;
    let mut next_transient_sequence = 0_u64;
    let mut pending_timer_scheduled_at = accelerated.pending_timer_scheduled_at;
    for batch in loaded.committed_batches.iter() {
        if batch.last_sequence <= accelerated.sequence {
            continue;
        }
        if batch.first_sequence <= accelerated.sequence {
            return Err("snapshot_splits_batch");
        }
        let events = kernel
            .apply(batch, next_transient_sequence)
            .map_err(|_| "journal_replay_failed")?;
        next_transient_sequence = next_transient_sequence
            .checked_add(
                u64::try_from(events.len()).map_err(|_| "transient_event_sequence_exhausted")?,
            )
            .ok_or("transient_event_sequence_exhausted")?;
        update_pending_timer_timestamp(&mut pending_timer_scheduled_at, batch);
    }
    if kernel.state().last_applied_sequence != loaded.head_sequence {
        return Err("loaded_head_mismatch");
    }
    Ok((kernel, next_transient_sequence, pending_timer_scheduled_at))
}

fn replay_from_default(
    loaded: &LoadedSession,
) -> Result<(Kernel, u64, Option<Timestamp>), &'static str> {
    let mut kernel = Kernel::default();
    let mut next_transient_sequence = 0_u64;
    let mut last_batch_sequence = 0_u64;
    let mut pending_timer_scheduled_at = None;
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
        update_pending_timer_timestamp(&mut pending_timer_scheduled_at, batch);
    }
    if last_batch_sequence != loaded.head_sequence
        || kernel.state().last_applied_sequence != loaded.head_sequence
    {
        return Err("loaded_head_mismatch");
    }
    Ok((kernel, next_transient_sequence, pending_timer_scheduled_at))
}

fn checksum_at(loaded: &LoadedSession, sequence: u64) -> Option<finstack_ai_kernel::Digest> {
    loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .find(|record| record.sequence() == sequence)
        .map(finstack_ai_kernel::RecordEnvelope::checksum)
}

fn update_pending_timer_timestamp(pending: &mut Option<Timestamp>, committed: &CommittedBatch) {
    for record in committed.records.iter() {
        match record.body() {
            RecordBody::RetryScheduled(_) => *pending = Some(record.timestamp()),
            RecordBody::TimerFired(_) => *pending = None,
            _ => {}
        }
    }
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

#[derive(Debug)]
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
    pub(crate) requested_at: Timestamp,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct ToolDispatchSeed {
    pub(crate) requested: EffectRequested,
    pub(crate) tool_batch_id: ToolBatchId,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) call: ValidatedToolCall,
    pub(crate) locator: OperationLocator,
    pub(crate) authorization: AuthorizationContext,
    pub(crate) budget_scope_id: Option<finstack_ai_kernel::BudgetScopeId>,
    pub(crate) attempt: u32,
    pub(crate) requested_at: Timestamp,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct TimerDispatchSeed {
    pub(crate) scheduled: finstack_ai_kernel::RetryScheduled,
    pub(crate) scheduled_at: Timestamp,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
pub(crate) struct RuntimeDispatch {
    pub(crate) action: PostCommitAction,
    pub(crate) model: Option<ModelDispatchSeed>,
    pub(crate) tool: Option<ToolDispatchSeed>,
    pub(crate) timer: Option<TimerDispatchSeed>,
}

pub(crate) trait PostCommitDispatcher: PortObject {
    fn validate_before_commit(&self, _input: &KernelInput) -> Result<(), DispatchError> {
        Ok(())
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>>;
}

fn model_dispatch_seed(
    state: &KernelState,
    action: PostCommitAction,
    committed: &CommittedBatch,
) -> Option<ModelDispatchSeed> {
    let PostCommitAction::ExecuteEffect { effect_id } = action else {
        return None;
    };
    let pending = state.pending_model_effect.as_ref()?;
    if pending.requested.effect_id() != effect_id || pending.deferred.is_some() {
        return None;
    }
    let (locator, authorization, budget_scope_id) = dispatch_security_context(state)?;
    Some(ModelDispatchSeed {
        pending: pending.clone(),
        locator,
        authorization,
        budget_scope_id,
        attempt: state.retry.attempts.checked_add(1)?,
        requested_at: effect_requested_at(committed, effect_id)?,
    })
}

fn tool_dispatch_seed(
    state: &KernelState,
    action: PostCommitAction,
    committed: &CommittedBatch,
) -> Option<ToolDispatchSeed> {
    let PostCommitAction::ExecuteEffect { effect_id } = action else {
        return None;
    };
    let batch = state.active_tool_batch.as_ref()?;
    let active = batch.calls.iter().find(|call| {
        call.assigned.effect_id == effect_id
            && matches!(
                call.status,
                ActiveToolCallStatus::Requested { deferred: None, .. }
            )
    })?;
    let ActiveToolCallStatus::Requested { requested, .. } = &active.status else {
        return None;
    };
    let finstack_ai_kernel::ToolCallPlan::Execute(call) = &active.assigned.plan else {
        return None;
    };
    let (locator, authorization, budget_scope_id) = dispatch_security_context(state)?;
    Some(ToolDispatchSeed {
        requested: requested.clone(),
        tool_batch_id: batch.opened.tool_batch_id,
        tool_call_id: *call.call.tool_call_id(),
        call: call.clone(),
        locator,
        authorization,
        budget_scope_id,
        attempt: 1,
        requested_at: effect_requested_at(committed, effect_id)?,
    })
}

fn timer_dispatch_seed(
    state: &KernelState,
    action: PostCommitAction,
    scheduled_at: Option<Timestamp>,
) -> Option<TimerDispatchSeed> {
    let PostCommitAction::ExecuteEffect { effect_id } = action else {
        return None;
    };
    let scheduled = state.retry.pending.as_ref()?;
    let scheduled_at = scheduled_at?;
    (scheduled.timer_effect_id == effect_id).then(|| TimerDispatchSeed {
        scheduled: scheduled.clone(),
        scheduled_at,
    })
}

fn effect_requested_at(committed: &CommittedBatch, effect_id: EffectId) -> Option<Timestamp> {
    committed.records.iter().find_map(|record| {
        matches!(record.body(), RecordBody::EffectRequested(request) if request.effect_id() == effect_id)
            .then(|| record.timestamp())
    })
}

fn dispatch_security_context(
    state: &KernelState,
) -> Option<(
    OperationLocator,
    AuthorizationContext,
    Option<finstack_ai_kernel::BudgetScopeId>,
)> {
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
    Some((
        locator,
        authorization,
        accepted.relation().budget_scope_id(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, BudgetChargeReceipt, BudgetChargeRequest, BudgetPropagation,
        BudgetReleaseReceipt, BudgetReleaseRequest, CancellationPropagation, ContentBlock,
        DeadlinePropagation, Digest, EffectCompleted, EffectOutputContract, EffectOutputKind, Id,
        IdTag, LaneTag, Message, MessageRole, Metadata, ModelSettled, ModelSettlement,
        PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, RecordEnvelope,
        ReducerStageOutcome, RetrySafety, RunAccepted, RunLimits, RunPropagationPolicy,
        RunRelation, RunSecurityContext, SessionTag, Stage, StageCursor, TextBlock, Timestamp,
        Usage,
    };

    use super::*;
    use crate::{
        AgentInvokeError, AgentInvoker, AgentRef, AuthorizationContext, BudgetCoordinator,
        BudgetError, BudgetLedger, BudgetOperationIds, BudgetRequest, BudgetReservationReceipt,
        BudgetReservationState, BudgetReserveRequest, ChildCoordinationIds, ChildPlacement,
        ChildRunContext, ChildRunCoordinator, ChildRunHandle, ChildRunLocator, ChildRunRequest,
        CompositionError, LoadedSession, OperationLocator, PortFuture, SnapshotReceipt,
        SnapshotRequest, SnapshotSchedule, StateSnapshotRequest, StoreHealth,
        child_relation_digest,
    };

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
        drive_accepted_to_model_request(coordinator).await
    }

    async fn drive_accepted_to_model_request(coordinator: &mut CommitCoordinator) -> CommitOutcome {
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
        composition_log: Option<Arc<Mutex<Vec<&'static str>>>>,
    }

    struct FakeInner {
        mode: FakeMode,
        append_calls: usize,
        batches: Vec<CommittedBatch>,
        requests: BTreeMap<finstack_ai_kernel::AppendBatchId, AppendRequest>,
    }

    struct StallingSnapshotStore {
        inner: FakeStore,
    }

    impl JournalStore for StallingSnapshotStore {
        fn append(
            &self,
            request: finstack_ai_kernel::AppendRequest,
        ) -> PortFuture<Result<finstack_ai_kernel::CommittedBatch, StoreError>> {
            self.inner.append(request)
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            self.inner.load(request)
        }

        fn write_snapshot(
            &self,
            request: SnapshotRequest,
        ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
            self.inner.write_snapshot(request)
        }

        fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
            self.inner.health()
        }

        fn write_state_snapshot(
            &self,
            _request: StateSnapshotRequest,
        ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
            Box::pin(async {
                #[cfg(feature = "native-tokio")]
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                Err(StoreError::Unavailable {
                    reason_code: "snapshot_write_stalled",
                })
            })
        }
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
                composition_log: None,
            }
        }

        fn with_composition_log(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
            let mut store = Self::new(FakeMode::Normal);
            store.composition_log = Some(log);
            store
        }

        fn append_calls(&self) -> usize {
            self.inner.lock().expect("lock").append_calls
        }
    }

    impl JournalStore for FakeStore {
        fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
            if let Some(log) = &self.composition_log {
                for record in request.records() {
                    let label = match record.body() {
                        RecordBody::ChildRunPrepared(_) => Some("prepare_committed"),
                        RecordBody::BudgetReservationRequested(_) => Some("reservation_requested"),
                        RecordBody::BudgetReservationSettled(_) => Some("reservation_settled"),
                        _ => None,
                    };
                    if let Some(label) = label {
                        log.lock().expect("log").push(label);
                    }
                }
            }
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
                    head_checksum: batches.last().and_then(|batch| {
                        batch
                            .records
                            .last()
                            .map(finstack_ai_kernel::RecordEnvelope::checksum)
                    }),
                    metadata: finstack_ai_kernel::Metadata::empty(),
                    committed_batches: batches.into(),
                    snapshot: None,
                    accelerated: None,
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

    struct AmbiguousReserveLedger {
        log: Arc<Mutex<Vec<&'static str>>>,
        receipt: BudgetReservationReceipt,
        charge_receipt: Option<finstack_ai_kernel::BudgetChargeReceipt>,
        release_receipt: Option<finstack_ai_kernel::BudgetReleaseReceipt>,
        reserved: Mutex<bool>,
        fail_after_reserve_once: Mutex<bool>,
        reserve_calls: Mutex<usize>,
        charge_calls: Mutex<usize>,
        release_calls: Mutex<usize>,
    }

    impl BudgetLedger for AmbiguousReserveLedger {
        fn reserve(
            &self,
            request: BudgetReserveRequest,
        ) -> PortFuture<Result<BudgetReservationReceipt, BudgetError>> {
            self.log.lock().expect("log").push("reserve");
            *self.reserve_calls.lock().expect("calls") += 1;
            *self.reserved.lock().expect("reserved") = true;
            let fail = std::mem::take(&mut *self.fail_after_reserve_once.lock().expect("fail"));
            let receipt = self.receipt.clone();
            assert_eq!(request.request_digest, receipt.request_digest);
            Box::pin(async move {
                if fail {
                    Err(BudgetError::Unavailable {
                        code: crate::BUDGET_UNAVAILABLE,
                        message: Arc::from("ambiguous reserve acknowledgement"),
                    })
                } else {
                    Ok(receipt)
                }
            })
        }

        fn reconcile(
            &self,
            scope_id: finstack_ai_kernel::BudgetScopeId,
            reservation_id: finstack_ai_kernel::BudgetReservationId,
        ) -> PortFuture<Result<BudgetReservationState, BudgetError>> {
            self.log.lock().expect("log").push("reconcile");
            let reserved = *self.reserved.lock().expect("reserved");
            let receipt = self.receipt.clone();
            assert_eq!(scope_id, receipt.scope_id);
            assert_eq!(reservation_id, receipt.reservation_id);
            Box::pin(async move {
                Ok(if reserved {
                    BudgetReservationState::Reserved(receipt)
                } else {
                    BudgetReservationState::NotFound
                })
            })
        }

        fn charge(
            &self,
            request: finstack_ai_kernel::BudgetChargeRequest,
        ) -> PortFuture<Result<finstack_ai_kernel::BudgetChargeReceipt, BudgetError>> {
            *self.charge_calls.lock().expect("calls") += 1;
            let receipt = self.charge_receipt.clone();
            Box::pin(async move {
                receipt.map_or_else(
                    || {
                        Err(BudgetError::InvalidRequest {
                            code: crate::BUDGET_INVALID_RECEIPT,
                            message: Arc::from("charge not configured"),
                        })
                    },
                    |receipt| {
                        assert_eq!(receipt.effect_id, request.effect_id);
                        Ok(receipt)
                    },
                )
            })
        }

        fn release(
            &self,
            request: finstack_ai_kernel::BudgetReleaseRequest,
        ) -> PortFuture<Result<finstack_ai_kernel::BudgetReleaseReceipt, BudgetError>> {
            *self.release_calls.lock().expect("calls") += 1;
            let receipt = self.release_receipt.clone();
            Box::pin(async move {
                receipt.map_or_else(
                    || {
                        Err(BudgetError::InvalidRequest {
                            code: crate::BUDGET_INVALID_RECEIPT,
                            message: Arc::from("release not configured"),
                        })
                    },
                    |receipt| {
                        assert_eq!(receipt.terminal_run_id, request.terminal_run_id);
                        Ok(receipt)
                    },
                )
            })
        }
    }

    struct IdempotentChildInvoker {
        log: Arc<Mutex<Vec<&'static str>>>,
        accepted: Mutex<BTreeMap<EffectId, Digest>>,
        physical_starts: Mutex<usize>,
    }

    impl AgentInvoker for IdempotentChildInvoker {
        fn start_or_attach(
            &self,
            context: ChildRunContext,
            request: ChildRunRequest,
        ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
            self.log.lock().expect("log").push("invoke");
            let relation_digest = child_relation_digest(&context, &request).expect("relation");
            let mut accepted = self.accepted.lock().expect("accepted");
            match accepted.get(&context.parent_effect_id) {
                Some(existing) if *existing != request.request_digest => {
                    let existing = *existing;
                    let submitted = request.request_digest;
                    return Box::pin(async move {
                        Err(AgentInvokeError::Conflict {
                            code: crate::AGENT_INVOKE_CONFLICT,
                            existing,
                            submitted,
                        })
                    });
                }
                Some(_) => {}
                None => {
                    accepted.insert(context.parent_effect_id, request.request_digest);
                    *self.physical_starts.lock().expect("starts") += 1;
                }
            }
            let locator = request.locator;
            Box::pin(async move {
                Ok(ChildRunHandle {
                    locator,
                    relation_digest,
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

    #[cfg(feature = "native-tokio")]
    #[tokio::test]
    async fn manual_drive_exposes_a_recoverable_committed_prefix_before_dispatch() {
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let mut coordinator =
            CommitCoordinator::with_test_dispatcher(store.clone(), dispatcher.clone());
        let mut controller = coordinator.enable_manual_drive(1).expect("manual drive");

        let blocked = tokio::spawn(async move {
            let outcome = drive_to_model_request(&mut coordinator).await;
            (coordinator, outcome)
        });
        let permit = controller.next_effect().await.expect("paused dispatch");

        assert_eq!(permit.effect().action, crate::ManualDriveAction::Execute);
        assert_eq!(store.append_calls(), 4);
        assert!(dispatcher.actions.lock().expect("lock").is_empty());
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover committed prefix");
        assert_eq!(
            recovered.state().phase,
            Some(finstack_ai_kernel::RunPhase::AwaitingModel)
        );
        assert!(recovered.state().pending_model_effect.is_some());

        blocked.abort();
        assert!(matches!(blocked.await, Err(error) if error.is_cancelled()));
        drop(permit);
        assert!(dispatcher.actions.lock().expect("lock").is_empty());
    }

    #[cfg(feature = "native-tokio")]
    #[tokio::test]
    async fn manual_drive_releases_exactly_the_observed_committed_action() {
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let mut coordinator = CommitCoordinator::with_test_dispatcher(store, dispatcher.clone());
        let mut controller = coordinator.enable_manual_drive(1).expect("manual drive");
        let blocked = tokio::spawn(async move { drive_to_model_request(&mut coordinator).await });

        let permit = controller.next_effect().await.expect("paused dispatch");
        let effect = permit.effect();
        permit.continue_dispatch();
        let outcome = blocked.await.expect("join");

        assert!(outcome.fault.is_none());
        assert_eq!(outcome.dispatched_actions, 1);
        assert_eq!(
            dispatcher.actions.lock().expect("lock").as_slice(),
            &[PostCommitAction::ExecuteEffect {
                effect_id: effect.effect_id,
            }]
        );
    }

    #[cfg(feature = "native-tokio")]
    #[tokio::test]
    async fn snapshot_write_timeout_does_not_fail_or_stall_submit() {
        let store = Arc::new(StallingSnapshotStore {
            inner: FakeStore::new(FakeMode::Normal),
        });
        let mut coordinator =
            CommitCoordinator::new(store).with_snapshot_schedule(SnapshotSchedule {
                every_n_records: 1,
                write_timeout: std::time::Duration::from_millis(50),
            });
        let started = std::time::Instant::now();
        coordinator
            .submit(
                env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
                accept_input(),
            )
            .await
            .expect("accept");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(150),
            "snapshot write must not block submit beyond write_timeout"
        );
        assert!(coordinator.fault().is_none());
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
    #[expect(
        clippy::too_many_lines,
        reason = "the crash-recovery scenario is clearer as one chronological proof"
    )]
    fn child_retry_reconciles_ambiguous_reservation_before_invoke() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeStore::with_composition_log(log.clone()));
        let mut commit = CommitCoordinator::new(store);
        block_on(commit.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        ))
        .expect("accept parent");

        let parent = OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(2),
            run_id: id(3),
        };
        let child_locator = ChildRunLocator {
            operation: OperationLocator {
                tenant_scope: Arc::from("tenant-a"),
                session_id: id(1),
                lane_id: id(40),
                run_id: id(41),
            },
            remote: None,
        };
        let budget = BudgetRequest {
            input_tokens: Some(1_000),
            output_tokens: Some(250),
            cost: None,
            extension_counters: BTreeMap::new(),
        };
        let scope_id = id(42);
        let reservation_id = id(43);
        let reserve_digest = BudgetReserveRequest::compute_digest(
            scope_id,
            reservation_id,
            child_locator.operation.run_id,
            &budget,
        )
        .expect("reserve digest");
        let reserve = BudgetReserveRequest {
            scope_id,
            reservation_id,
            run_id: child_locator.operation.run_id,
            amount: budget.clone(),
            request_digest: reserve_digest,
        };
        let receipt = BudgetReservationReceipt {
            scope_id,
            reservation_id,
            reserved: budget.clone(),
            remaining: BudgetRequest::default(),
            request_digest: reserve_digest,
            receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
        };
        let ledger = Arc::new(AmbiguousReserveLedger {
            log: log.clone(),
            receipt,
            charge_receipt: None,
            release_receipt: None,
            reserved: Mutex::new(false),
            fail_after_reserve_once: Mutex::new(true),
            reserve_calls: Mutex::new(0),
            charge_calls: Mutex::new(0),
            release_calls: Mutex::new(0),
        });
        let invoker = Arc::new(IdempotentChildInvoker {
            log: log.clone(),
            accepted: Mutex::new(BTreeMap::new()),
            physical_starts: Mutex::new(0),
        });
        let coordinator =
            ChildRunCoordinator::new(invoker.clone()).with_budget_ledger(ledger.clone());
        let context = ChildRunContext {
            parent: parent.clone(),
            parent_effect_id: id(44),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: Arc::from("oidc"),
                assurance_level: Arc::from("high"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
        };
        let request = ChildRunRequest {
            agent: AgentRef {
                id: crate::AgentId::parse("finstack.agent.child").expect("agent id"),
                bundle: None,
                spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
            },
            input: Arc::from([ContentBlock::Text(
                TextBlock::try_new("do the work").expect("text"),
            )]),
            placement: ChildPlacement::CompatibleLaneInParentSession,
            locator: child_locator,
            requested_deadline: Some(timestamp(5_000)),
            requested_budget: budget,
            delegation_id: None,
            metadata: Metadata::empty(),
            request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
        };
        let ids = ChildCoordinationIds {
            preparation_batch_id: id(201),
            preparation_record_id: id(202),
            reservation_request_record_id: Some(id(203)),
            reservation_settlement: Some(BudgetOperationIds {
                batch_id: id(204),
                record_id: id(205),
            }),
        };

        assert!(matches!(
            block_on(coordinator.start_or_attach(
                &mut commit,
                context.clone(),
                request.clone(),
                Some(reserve.clone()),
                ids,
                timestamp(1_100),
            )),
            Err(CompositionError::Budget(BudgetError::Unavailable { .. }))
        ));
        assert!(!log.lock().expect("log").contains(&"invoke"));

        let first = block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            Some(reserve.clone()),
            ids,
            timestamp(1_100),
        ))
        .expect("reconcile and invoke");
        let attached = block_on(coordinator.start_or_attach(
            &mut commit,
            context.clone(),
            request.clone(),
            Some(reserve.clone()),
            ids,
            timestamp(1_100),
        ))
        .expect("attach equal retry");
        assert_eq!(first, attached);
        assert_eq!(*ledger.reserve_calls.lock().expect("calls"), 1);
        assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
        assert_eq!(
            log.lock().expect("log").as_slice(),
            &[
                "prepare_committed",
                "reservation_requested",
                "reconcile",
                "reserve",
                "reconcile",
                "reservation_settled",
                "invoke",
                "invoke",
            ]
        );

        let mut conflicting = request;
        conflicting.request_digest = Digest::raw_json(br#"{"request":"child-b"}"#);
        assert!(matches!(
            block_on(coordinator.start_or_attach(
                &mut commit,
                context,
                conflicting,
                Some(reserve),
                ids,
                timestamp(1_100),
            )),
            Err(CompositionError::Commit(
                CommitCoordinatorError::SidecarConflict
            ))
        ));
        assert_eq!(*ledger.reserve_calls.lock().expect("calls"), 1);
        assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "all placement policies share one table-driven handshake proof"
    )]
    fn every_child_placement_converges_and_rejects_conflicting_digest() {
        let parent = OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(2),
            run_id: id(3),
        };
        let remote = crate::RemoteRouteRef {
            service: crate::ComponentRef::new(
                crate::ComponentId::parse("finstack.remote.worker").expect("service"),
                Some(crate::Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
            ),
            route: crate::ExternalHandleRef::try_new(
                crate::ComponentId::parse("finstack.remote.worker").expect("provider"),
                "route-a",
                RawJson::parse(r#"{"cluster":"a"}"#).expect("route metadata"),
            )
            .expect("route"),
        };
        let cases = [
            (
                ChildPlacement::CompatibleLaneInParentSession,
                ChildRunLocator {
                    operation: OperationLocator {
                        tenant_scope: Arc::from("tenant-a"),
                        session_id: id(1),
                        lane_id: id(50),
                        run_id: id(51),
                    },
                    remote: None,
                },
            ),
            (
                ChildPlacement::IsolatedChildSession,
                ChildRunLocator {
                    operation: OperationLocator {
                        tenant_scope: Arc::from("tenant-a"),
                        session_id: id(60),
                        lane_id: id(61),
                        run_id: id(62),
                    },
                    remote: None,
                },
            ),
            (
                ChildPlacement::RemoteChildSession,
                ChildRunLocator {
                    operation: OperationLocator {
                        tenant_scope: Arc::from("tenant-a"),
                        session_id: id(70),
                        lane_id: id(71),
                        run_id: id(72),
                    },
                    remote: Some(remote),
                },
            ),
        ];

        for (index, (placement, locator)) in cases.into_iter().enumerate() {
            let store = Arc::new(FakeStore::new(FakeMode::Normal));
            let mut commit = CommitCoordinator::new(store);
            block_on(commit.submit(
                env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
                accept_input(),
            ))
            .expect("accept parent");
            let invoker = Arc::new(IdempotentChildInvoker {
                log: Arc::new(Mutex::new(Vec::new())),
                accepted: Mutex::new(BTreeMap::new()),
                physical_starts: Mutex::new(0),
            });
            let coordinator = ChildRunCoordinator::new(invoker.clone());
            let context = ChildRunContext {
                parent: parent.clone(),
                parent_effect_id: id(80 + u64::try_from(index).expect("index")),
                authorization: AuthorizationContext {
                    principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                        .expect("principal"),
                    authentication_method: Arc::from("oidc"),
                    assurance_level: Arc::from("high"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("policy-v1"),
                    decision_id: Arc::from("decision-v1"),
                },
            };
            let request = ChildRunRequest {
                agent: AgentRef {
                    id: crate::AgentId::parse("finstack.agent.placement").expect("agent id"),
                    bundle: None,
                    spec_digest: Digest::raw_json(br#"{"agent":"placement"}"#),
                },
                input: Arc::from([]),
                placement,
                locator,
                requested_deadline: None,
                requested_budget: BudgetRequest::default(),
                delegation_id: None,
                metadata: Metadata::empty(),
                request_digest: Digest::raw_json(format!(r#"{{"placement":{index}}}"#).as_bytes()),
            };
            let ordinal = 500 + u64::try_from(index).expect("index") * 10;
            let ids = ChildCoordinationIds {
                preparation_batch_id: id(ordinal),
                preparation_record_id: id(ordinal + 1),
                reservation_request_record_id: None,
                reservation_settlement: None,
            };
            let first = block_on(coordinator.start_or_attach(
                &mut commit,
                context.clone(),
                request.clone(),
                None,
                ids,
                timestamp(1_100),
            ))
            .expect("start child");
            let attached = block_on(coordinator.start_or_attach(
                &mut commit,
                context.clone(),
                request.clone(),
                None,
                ids,
                timestamp(1_100),
            ))
            .expect("attach child");
            assert_eq!(first, attached);
            assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
            assert_eq!(
                commit
                    .state()
                    .child_preparations
                    .get(&context.parent_effect_id)
                    .map(|prepared| &prepared.child),
                Some(&request.locator)
            );

            let mut conflicting = request;
            conflicting.request_digest = Digest::raw_json(b"conflicting child request");
            assert!(matches!(
                block_on(coordinator.start_or_attach(
                    &mut commit,
                    context,
                    conflicting,
                    None,
                    ids,
                    timestamp(1_100),
                )),
                Err(CompositionError::Commit(
                    CommitCoordinatorError::SidecarConflict
                ))
            ));
            assert_eq!(*invoker.physical_starts.lock().expect("starts"), 1);
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the post-commit charge and release proof intentionally covers one lifecycle"
    )]
    fn budget_charge_and_release_are_post_commit_and_idempotent() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeStore::new(FakeMode::Normal));
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let mut commit = CommitCoordinator::with_test_dispatcher(store, dispatcher);
        block_on(commit.submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        ))
        .expect("accept parent");

        let parent = OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(2),
            run_id: id(3),
        };
        let child_locator = ChildRunLocator {
            operation: OperationLocator {
                tenant_scope: Arc::from("tenant-a"),
                session_id: id(1),
                lane_id: id(340),
                run_id: id(341),
            },
            remote: None,
        };
        let budget = BudgetRequest {
            input_tokens: Some(1_000),
            output_tokens: Some(250),
            cost: None,
            extension_counters: BTreeMap::new(),
        };
        let scope_id = id(342);
        let reservation_id = id(343);
        let reserve_digest = BudgetReserveRequest::compute_digest(
            scope_id,
            reservation_id,
            child_locator.operation.run_id,
            &budget,
        )
        .expect("reserve digest");
        let reserve = BudgetReserveRequest {
            scope_id,
            reservation_id,
            run_id: child_locator.operation.run_id,
            amount: budget.clone(),
            request_digest: reserve_digest,
        };
        let reserve_receipt = BudgetReservationReceipt {
            scope_id,
            reservation_id,
            reserved: budget.clone(),
            remaining: BudgetRequest::default(),
            request_digest: reserve_digest,
            receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
        };
        let usage =
            Usage::try_new(Some(20), Some(10), Some(30), None, BTreeMap::new()).expect("usage");
        let usage_digest = Digest::effect_output(&usage.canonical_bytes().expect("usage bytes"));
        let charge_request = BudgetChargeRequest {
            scope_id,
            reservation_id,
            effect_id: id(103),
            usage: usage.clone(),
            usage_digest,
        };
        let charge_receipt = BudgetChargeReceipt {
            scope_id,
            reservation_id,
            effect_id: id(103),
            charged_usage: usage.clone(),
            cumulative_usage: usage.clone(),
            usage_digest,
            receipt_digest: Digest::raw_json(br#"{"receipt":"charge"}"#),
        };
        let release_digest = BudgetReleaseRequest::compute_digest(scope_id, reservation_id, id(3))
            .expect("release digest");
        let release_request = BudgetReleaseRequest {
            scope_id,
            reservation_id,
            terminal_run_id: id(3),
            request_digest: release_digest,
        };
        let release_receipt = BudgetReleaseReceipt {
            scope_id,
            reservation_id,
            terminal_run_id: id(3),
            released_unused: BudgetRequest::default(),
            request_digest: release_digest,
            receipt_digest: Digest::raw_json(br#"{"receipt":"release"}"#),
        };
        let ledger = Arc::new(AmbiguousReserveLedger {
            log: log.clone(),
            receipt: reserve_receipt,
            charge_receipt: Some(charge_receipt.clone()),
            release_receipt: Some(release_receipt.clone()),
            reserved: Mutex::new(false),
            fail_after_reserve_once: Mutex::new(false),
            reserve_calls: Mutex::new(0),
            charge_calls: Mutex::new(0),
            release_calls: Mutex::new(0),
        });
        let invoker = Arc::new(IdempotentChildInvoker {
            log,
            accepted: Mutex::new(BTreeMap::new()),
            physical_starts: Mutex::new(0),
        });
        let child_coordinator =
            ChildRunCoordinator::new(invoker).with_budget_ledger(ledger.clone());
        let child_context = ChildRunContext {
            parent: parent.clone(),
            parent_effect_id: id(344),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: Arc::from("oidc"),
                assurance_level: Arc::from("high"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
        };
        let child_request = ChildRunRequest {
            agent: AgentRef {
                id: crate::AgentId::parse("finstack.agent.budget-child").expect("agent id"),
                bundle: None,
                spec_digest: Digest::raw_json(br#"{"agent":"budget-child"}"#),
            },
            input: Arc::from([ContentBlock::Text(
                TextBlock::try_new("budgeted work").expect("text"),
            )]),
            placement: ChildPlacement::CompatibleLaneInParentSession,
            locator: child_locator,
            requested_deadline: None,
            requested_budget: budget,
            delegation_id: None,
            metadata: Metadata::empty(),
            request_digest: Digest::raw_json(br#"{"request":"budget-child"}"#),
        };
        block_on(child_coordinator.start_or_attach(
            &mut commit,
            child_context,
            child_request,
            Some(reserve),
            ChildCoordinationIds {
                preparation_batch_id: id(401),
                preparation_record_id: id(402),
                reservation_request_record_id: Some(id(403)),
                reservation_settlement: Some(BudgetOperationIds {
                    batch_id: id(404),
                    record_id: id(405),
                }),
            },
            timestamp(1_050),
        ))
        .expect("prepare budgeted child");

        let budget_coordinator = BudgetCoordinator::new(ledger.clone());
        assert!(matches!(
            block_on(budget_coordinator.charge_committed(
                &mut commit,
                &parent,
                charge_request.clone(),
                BudgetOperationIds {
                    batch_id: id(406),
                    record_id: id(407),
                },
                timestamp(1_350),
            )),
            Err(CompositionError::InvalidRequest {
                code: "effect_usage_not_committed"
            })
        ));
        assert_eq!(*ledger.charge_calls.lock().expect("calls"), 0);

        block_on(drive_accepted_to_model_request(&mut commit));
        let completion = EffectCompleted::try_new(
            id(103),
            output_contract(),
            RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
            Some(usage),
            vec![],
            ProviderIds::empty(),
            Some("budget-completion"),
            None,
        )
        .expect("completion");
        let assistant_message = Message::try_new(
            id(104),
            MessageRole::Assistant,
            vec![ContentBlock::Text(
                TextBlock::try_new("hello").expect("text"),
            )],
            timestamp(1_400),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("assistant message");
        block_on(commit.submit(
            env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[104], 105),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: id(101),
                model_request_id: id(102),
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message,
                },
            }),
        ))
        .expect("settle model");

        let charge_ids = BudgetOperationIds {
            batch_id: id(406),
            record_id: id(407),
        };
        let charged = block_on(budget_coordinator.charge_committed(
            &mut commit,
            &parent,
            charge_request.clone(),
            charge_ids,
            timestamp(1_450),
        ))
        .expect("charge committed usage");
        assert_eq!(charged, charge_receipt);
        assert_eq!(
            block_on(budget_coordinator.charge_committed(
                &mut commit,
                &parent,
                charge_request,
                charge_ids,
                timestamp(1_450),
            ))
            .expect("equal charge retry"),
            charge_receipt
        );
        assert_eq!(*ledger.charge_calls.lock().expect("calls"), 1);

        block_on(commit.submit(
            env(1_500, &[9], &[], &[], &[], &[], &[], 106),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        ))
        .expect("after model");
        block_on(commit.submit(
            env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 107),
            stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        ))
        .expect("terminal commit");

        let release_ids = BudgetOperationIds {
            batch_id: id(408),
            record_id: id(409),
        };
        let released = block_on(budget_coordinator.release_committed(
            &mut commit,
            &parent,
            release_request.clone(),
            release_ids,
            timestamp(1_650),
        ))
        .expect("release after terminal");
        assert_eq!(released, release_receipt);
        assert_eq!(
            block_on(budget_coordinator.release_committed(
                &mut commit,
                &parent,
                release_request,
                release_ids,
                timestamp(1_650),
            ))
            .expect("equal release retry"),
            release_receipt
        );
        assert_eq!(*ledger.release_calls.lock().expect("calls"), 1);
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
