use crate::ports::journal::StoreError;

use finstack_ai_kernel::{
    AppendBatchId, AppendRequest, CommittedBatch, ConversationEntry, EntryId, KernelState, LaneId,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordId, SessionId,
    SessionProjection, Timestamp,
};

use super::{CommitCoordinator, CommitCoordinatorError};

impl CommitCoordinator {
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
                    self.reload_after_conflict(session_id, "composition_conflict_reload_failed")
                        .await?;
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
                Err(error) => return Err(self.sidecar_append_error(error)),
            };
            let events = self
                .kernel
                .apply(&committed, self.next_transient_sequence)
                .map_err(|_| self.boundary_fault("composition_apply_failed"))?;
            if !events.is_empty() {
                return Err(self.boundary_fault("composition_emitted_events"));
            }
            self.apply_committed_session(&committed)?;
            return Ok(Some(committed));
        }
    }

    /// Commit zero-event session and conversation records through the append/apply boundary.
    ///
    /// # Errors
    ///
    /// Returns a store/boundary failure when the batch is invalid or cannot be applied.
    pub async fn commit_session_records(
        &mut self,
        batch_id: AppendBatchId,
        records: Vec<RecordDraft>,
    ) -> Result<Option<CommittedBatch>, CommitCoordinatorError> {
        if records.is_empty()
            || records.iter().any(|record| {
                !record.derived_event_ids().is_empty()
                    || !matches!(
                        record.body(),
                        RecordBody::SessionCreated(_)
                            | RecordBody::LaneCreated(_)
                            | RecordBody::LaneMoved(_)
                            | RecordBody::ConversationEntry(_)
                            | RecordBody::SnapshotWritten(_)
                    )
            })
        {
            return Err(CommitCoordinatorError::BoundaryFault {
                code: "session_records_invalid",
            });
        }
        let session_id = records[0].session_id();
        let mut conflicts = 0_u8;
        loop {
            let expected_sequence = self
                .kernel
                .state()
                .last_applied_sequence
                .checked_add(1)
                .ok_or_else(|| self.boundary_fault("session_sequence_overflow"))?;
            let records = align_conversation_sequences(records.clone(), expected_sequence)?;
            preview_session_records(&self.session, &records)?;
            let request = AppendRequest::try_new(batch_id, session_id, expected_sequence, records)
                .map_err(|_| CommitCoordinatorError::BoundaryFault {
                    code: "session_append_request_invalid",
                })?;
            let committed = match self.append_frozen(request).await {
                Ok(committed) => committed,
                Err(StoreError::Conflict { .. }) if conflicts == 0 => {
                    conflicts = 1;
                    self.reload_after_conflict(session_id, "session_conflict_reload_failed")
                        .await?;
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
                Err(error) => return Err(self.sidecar_append_error(error)),
            };
            let events = self
                .kernel
                .apply(&committed, self.next_transient_sequence)
                .map_err(|_| self.boundary_fault("session_apply_failed"))?;
            if !events.is_empty() {
                return Err(self.boundary_fault("session_emitted_events"));
            }
            self.apply_committed_session(&committed)?;
            return Ok(Some(committed));
        }
    }

    pub(super) fn apply_committed_session(
        &mut self,
        committed: &CommittedBatch,
    ) -> Result<(), CommitCoordinatorError> {
        apply_batch_to_session(&mut self.session, committed)
            .map_err(|code| self.boundary_fault(code))?;
        self.note_head_checksum(committed);
        #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
        self.record_kinds.extend(
            committed
                .records
                .iter()
                .map(|record| std::sync::Arc::from(record.body().kind_name())),
        );
        Ok(())
    }

    pub(super) async fn maybe_commit_conversation_siblings(
        &mut self,
        committed: &CommittedBatch,
    ) -> Result<(), CommitCoordinatorError> {
        if self.session.lanes().is_empty() {
            return Ok(());
        }
        let mut drafts = Vec::new();
        for record in committed.records.iter() {
            if !self.session.lanes().contains_key(&record.lane_id()) {
                continue;
            }
            let message = match record.body() {
                RecordBody::EntryAppended(entry) => &entry.message,
                RecordBody::ToolCallSettled(settled) => &settled.message,
                _ => continue,
            };
            let entry_id = EntryId::from_bytes(message.id().to_bytes());
            let parent_id = self
                .session
                .entries()
                .get(&entry_id)
                .and_then(ConversationEntry::parent_id);
            let entry = ConversationEntry::from_message(message, parent_id, record.lane_id(), 0)
                .map_err(|_| CommitCoordinatorError::BoundaryFault {
                    code: "session_records_invalid",
                })?;
            drafts.push(session_draft(
                RecordId::from_bytes(entry.id().to_bytes()),
                record.session_id(),
                record.lane_id(),
                record.timestamp(),
                RecordBody::ConversationEntry(entry.clone()),
            )?);
            drafts.push(session_draft(
                lane_moved_record_id(entry.id()),
                record.session_id(),
                record.lane_id(),
                record.timestamp(),
                RecordBody::LaneMoved(finstack_ai_kernel::LaneMoved::new(entry.id())),
            )?);
        }
        if drafts.is_empty() {
            return Ok(());
        }
        let batch_id = AppendBatchId::from_bytes(drafts[0].record_id().to_bytes());
        self.commit_session_records(batch_id, drafts).await?;
        Ok(())
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
pub(super) fn apply_batch_to_session(
    session: &mut SessionProjection,
    batch: &CommittedBatch,
) -> Result<(), &'static str> {
    for record in batch.records.iter() {
        session
            .apply_envelope(record)
            .map_err(|_| "session_projection_failed")?;
    }
    Ok(())
}

fn align_conversation_sequences(
    records: Vec<RecordDraft>,
    first_sequence: u64,
) -> Result<Vec<RecordDraft>, CommitCoordinatorError> {
    records
        .into_iter()
        .enumerate()
        .map(|(offset, draft)| {
            let sequence = first_sequence
                .checked_add(u64::try_from(offset).map_err(|_| {
                    CommitCoordinatorError::BoundaryFault {
                        code: "session_sequence_overflow",
                    }
                })?)
                .ok_or(CommitCoordinatorError::BoundaryFault {
                    code: "session_sequence_overflow",
                })?;
            let RecordBody::ConversationEntry(entry) = draft.body() else {
                return Ok(draft);
            };
            if entry.sequence() == sequence {
                return Ok(draft);
            }
            let aligned = ConversationEntry::try_new(
                entry.id(),
                entry.parent_id(),
                entry.lane_id(),
                sequence,
                entry.body().clone(),
            )
            .map_err(|_| CommitCoordinatorError::BoundaryFault {
                code: "session_records_invalid",
            })?;
            RecordDraft::try_new(
                draft.format_version(),
                draft.kind_version(),
                draft.record_id(),
                draft.session_id(),
                draft.lane_id(),
                draft.run_id(),
                draft.timestamp(),
                draft.derived_event_ids().to_vec(),
                RecordBody::ConversationEntry(aligned),
            )
            .map_err(|_| CommitCoordinatorError::BoundaryFault {
                code: "session_records_invalid",
            })
        })
        .collect()
}

pub(crate) fn session_draft(
    record_id: RecordId,
    session_id: SessionId,
    lane_id: LaneId,
    timestamp: Timestamp,
    body: RecordBody,
) -> Result<RecordDraft, CommitCoordinatorError> {
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        record_id,
        session_id,
        lane_id,
        None,
        timestamp,
        Vec::new(),
        body,
    )
    .map_err(|_| CommitCoordinatorError::BoundaryFault {
        code: "session_records_invalid",
    })
}

fn preview_session_records(
    session: &SessionProjection,
    records: &[RecordDraft],
) -> Result<(), CommitCoordinatorError> {
    session
        .preview_structural_drafts(records)
        .map_err(|_| CommitCoordinatorError::BoundaryFault {
            code: "session_records_invalid",
        })
        .map(|_| ())
}

fn lane_moved_record_id(entry_id: EntryId) -> RecordId {
    let mut bytes = entry_id.to_bytes();
    bytes[15] ^= 0xA5;
    RecordId::from_bytes(bytes)
}
