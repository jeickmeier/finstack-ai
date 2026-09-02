use std::sync::Arc;

use crate::ports::journal::{
    LoadFromRequest, LoadRequest, LoadWindow, LoadedSession, StateSnapshotRequest, StoreError,
};

use finstack_ai_kernel::{
    AppendRequest, CommittedBatch, Decision, Diagnostic, KernelError, KernelInput,
    PostCommitAction, RecordBody, Timestamp, TransitionEnv,
};

use super::{CommitCoordinator, CommitCoordinatorError, CommitOutcome, ReplayScope, RunFault};

use super::dispatch::{
    RuntimeDispatch, action_is_authorized, context_dispatch_seed, model_dispatch_seed,
    timer_dispatch_seed, tool_dispatch_seed,
};
use super::recover::{
    adopt_session_head, continuation_after_replay, project_loaded, replay_scoped,
    update_last_model_continuation, used_snapshot_sequence,
};
use super::session_commit::apply_batch_to_session;

impl CommitCoordinator {
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
                    self.reload_after_conflict(session_id, "conflict_reload_failed")
                        .await?;
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
                    return Err(self.boundary_fault_with_store("store_integrity_uncertain", &error));
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
            if let Err(code) =
                update_last_model_continuation(&mut self.last_model_continuation, &committed)
            {
                return Err(self.boundary_fault(code));
            }
            self.next_transient_sequence = self
                .next_transient_sequence
                .checked_add(
                    u64::try_from(events.len())
                        .map_err(|_| self.boundary_fault("transient_event_sequence_exhausted"))?,
                )
                .ok_or_else(|| self.boundary_fault("transient_event_sequence_exhausted"))?;
            self.apply_committed_session(&committed)?;
            self.maybe_commit_conversation_siblings(&committed).await?;

            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            self.publish_live_state();

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

    pub(super) async fn reload_after_conflict(
        &mut self,
        session_id: finstack_ai_kernel::SessionId,
        fault_code: &'static str,
    ) -> Result<(), CommitCoordinatorError> {
        if matches!(self.replay_scope, ReplayScope::StructuralOnly)
            && let Some(prior_checksum) = self.head_checksum
        {
            let from_sequence = self
                .kernel
                .state()
                .last_applied_sequence()
                .saturating_add(1);
            if from_sequence > 1 {
                let loaded = self
                    .store
                    .load_from(LoadFromRequest {
                        session_id,
                        window: LoadWindow::FromSequence {
                            from_sequence,
                            prior_checksum,
                        },
                    })
                    .await
                    .map_err(|_| self.boundary_fault(fault_code))?;
                return self.catch_up_structural(&loaded);
            }
        }
        let loaded = self
            .store
            .load(LoadRequest { session_id })
            .await
            .map_err(|_| self.boundary_fault(fault_code))?;
        self.reload_from_loaded(&loaded)
    }

    pub(super) fn reload_from_loaded(
        &mut self,
        loaded: &LoadedSession,
    ) -> Result<(), CommitCoordinatorError> {
        let prior_next_transient_sequence = self.next_transient_sequence;
        let (kernel, next_transient_sequence, pending_timer_scheduled_at, used_snapshot) =
            replay_scoped(loaded, self.replay_scope).map_err(|code| self.boundary_fault(code))?;
        self.kernel = kernel;
        // A live event hub has already observed every event emitted before the
        // conflict. Scoped replay can omit sibling-run records and therefore
        // derive a smaller historical count; never move the process-local
        // sequence cursor backwards across a reload.
        self.next_transient_sequence = prior_next_transient_sequence.max(next_transient_sequence);
        self.pending_timer_scheduled_at = pending_timer_scheduled_at;
        self.last_model_continuation =
            continuation_after_replay(loaded, self.replay_scope, used_snapshot)
                .map_err(|code| self.boundary_fault(code))?;
        #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
        {
            self.replayed_completed_effects = super::recover::completed_effect_pairs(loaded);
            self.replayed_extension_envelopes = super::recover::extension_request_envelopes(loaded);
            self.record_kinds = super::recover::loaded_record_kinds(loaded);
            // A reload is a recovery boundary. Checkpoints are deliberately
            // not reconstructed from journal or snapshot state.
            self.discard_compaction_checkpoint();
        }
        self.last_snapshot_sequence = used_snapshot_sequence(loaded, used_snapshot);
        self.head_checksum = loaded.head_checksum;
        if loaded.omits_prefix() {
            for batch in loaded.committed_batches.iter() {
                apply_batch_to_session(&mut self.session, batch)
                    .map_err(|code| self.boundary_fault(code))?;
            }
        } else {
            self.session = project_loaded(loaded).map_err(|code| self.boundary_fault(code))?;
        }
        #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
        self.publish_live_state();
        Ok(())
    }

    fn catch_up_structural(
        &mut self,
        loaded: &LoadedSession,
    ) -> Result<(), CommitCoordinatorError> {
        for batch in loaded.committed_batches.iter() {
            apply_batch_to_session(&mut self.session, batch)
                .map_err(|code| self.boundary_fault(code))?;
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            self.record_kinds.extend(
                batch
                    .records
                    .iter()
                    .map(|record| Arc::from(record.body().kind_name())),
            );
        }
        adopt_session_head(&mut self.kernel, loaded.head_sequence)
            .map_err(|code| self.boundary_fault(code))?;
        self.head_checksum = loaded.head_checksum;
        #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
        self.publish_live_state();
        Ok(())
    }

    pub(super) fn note_head_checksum(&mut self, committed: &CommittedBatch) {
        self.head_checksum = committed
            .records
            .last()
            .map(finstack_ai_kernel::RecordEnvelope::checksum);
    }

    async fn maybe_write_snapshot(&mut self, committed: &CommittedBatch) {
        if !matches!(self.replay_scope, ReplayScope::Primary)
            || self.snapshot_schedule.every_n_records == 0
        {
            return;
        }
        let head = self.kernel.state().last_applied_sequence();
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
            last_model_continuation: self.last_model_continuation.clone(),
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

    pub(super) async fn append_frozen(
        &self,
        frozen: AppendRequest,
    ) -> Result<CommittedBatch, StoreError> {
        match self.store.append(frozen.clone()).await {
            Err(StoreError::AmbiguousAcknowledgement) => self.store.append(frozen).await,
            result => result,
        }
    }

    /// Commit zero-event composition records through the same append/apply boundary.
    ///
    /// This is intentionally narrower than general effect dispatch: it accepts
    /// only child-and-budget sidecar contract child/budget sidecar records and never calls an external service.
    /// A sequence race reloads only the known session; equal durable identities
    /// converge and different content fails closed.
    ///
    /// # Errors
    ///
    /// Returns a store/boundary failure, or [`CommitCoordinatorError::SidecarConflict`]
    pub(super) async fn dispatch_after_recheck(
        &self,
        action: PostCommitAction,
        now: Timestamp,
        committed: &CommittedBatch,
    ) -> Result<(), &'static str> {
        if !action_is_authorized(self.kernel.state(), action, now, committed) {
            return Err("dispatch_precondition_failed");
        }
        if matches!(
            action,
            PostCommitAction::ExecuteEffect { effect_id }
                if self
                    .kernel
                    .state()
                    .pending_extension_effect()
                    .is_some_and(|pending| pending.requested.effect_id() == effect_id)
        ) {
            // Context and middleware are executed synchronously by the stage
            // driver immediately after this durable request commits. They do
            // not enter the detached model/tool dispatcher queues.
            return Ok(());
        }
        let Some(dispatcher) = &self.dispatcher else {
            return Err("effect_driver_unavailable");
        };
        let dispatch = RuntimeDispatch {
            action,
            model: model_dispatch_seed(
                self.kernel.state(),
                action,
                committed,
                self.last_model_continuation.clone(),
            ),
            tool: tool_dispatch_seed(self.kernel.state(), action, committed),
            timer: timer_dispatch_seed(
                self.kernel.state(),
                action,
                self.pending_timer_scheduled_at,
            ),
            context: context_dispatch_seed(self.kernel.state(), action, committed),
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

    pub(super) fn boundary_fault(&mut self, code: &'static str) -> CommitCoordinatorError {
        self.fault = Some(RunFault { code });
        CommitCoordinatorError::BoundaryFault { code }
    }

    pub(super) fn boundary_fault_with_store(
        &mut self,
        code: &'static str,
        error: &StoreError,
    ) -> CommitCoordinatorError {
        self.last_store_reason = Some(Arc::from(error.to_string()));
        self.boundary_fault(code)
    }

    pub(super) fn sidecar_append_error(&mut self, error: StoreError) -> CommitCoordinatorError {
        match error {
            StoreError::AmbiguousAcknowledgement => {
                self.boundary_fault("continued_ambiguous_acknowledgement")
            }
            error @ (StoreError::Corruption { .. } | StoreError::Integrity { .. }) => {
                self.boundary_fault_with_store("store_integrity_uncertain", &error)
            }
            error => CommitCoordinatorError::Store(error),
        }
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
pub(super) fn update_pending_timer_timestamp(
    pending: &mut Option<Timestamp>,
    committed: &CommittedBatch,
) {
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
