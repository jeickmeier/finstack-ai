use std::sync::Arc;
use std::time::Duration;

use crate::{AcceleratedRestore, JournalStore, LoadRequest, LoadedSession, SnapshotSchedule};

use finstack_ai_kernel::{
    CommittedBatch, Kernel, RecordEnvelope, RunId, SessionProjection, Timestamp,
};

use super::session_commit::apply_batch_to_session;
use super::submit::update_pending_timer_timestamp;
use super::{CommitCoordinator, CommitCoordinatorError, ReplayScope};

impl CommitCoordinator {
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
        let session = project_loaded(&loaded)
            .map_err(|code| CommitCoordinatorError::BoundaryFault { code })?;
        Ok(Self {
            kernel,
            session,
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
            last_store_reason: None,
            dispatcher: None,
            #[cfg(feature = "native-tokio")]
            manual_drive: None,
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            event_publisher: None,
            replay_scope: ReplayScope::Primary,
            middleware_chain: None,
        })
    }

    /// Reconstruct a coordinator for one run, or for structural-only mutation.
    ///
    /// `target_run_id = None` applies structural records only and leaves the
    /// kernel empty at the session head so a new sibling `AcceptRun` can
    /// proceed. `Some(run_id)` applies that run's records and structural
    /// records. Other runs' `RunAccepted` records are not applied. The
    /// session snapshot is used only when it already belongs to `run_id`.
    ///
    /// # Errors
    ///
    /// Returns a boundary fault when load metadata or any committed batch is invalid.
    pub async fn recover_run(
        store: Arc<dyn JournalStore>,
        session_id: finstack_ai_kernel::SessionId,
        target_run_id: Option<RunId>,
    ) -> Result<Self, CommitCoordinatorError> {
        let loaded = store
            .load(LoadRequest { session_id })
            .await
            .map_err(CommitCoordinatorError::Store)?;
        let scope = match target_run_id {
            None => ReplayScope::StructuralOnly,
            Some(run_id) => ReplayScope::Run(run_id),
        };
        let (kernel, next_transient_sequence, pending_timer_scheduled_at, used_snapshot) =
            replay_scoped(&loaded, scope)
                .map_err(|code| CommitCoordinatorError::BoundaryFault { code })?;
        let session = project_loaded(&loaded)
            .map_err(|code| CommitCoordinatorError::BoundaryFault { code })?;
        Ok(Self {
            kernel,
            session,
            store,
            next_transient_sequence,
            pending_timer_scheduled_at,
            snapshot_schedule: SnapshotSchedule {
                every_n_records: u64::MAX,
                write_timeout: Duration::from_millis(50),
            },
            last_snapshot_sequence: used_snapshot
                .then(|| {
                    loaded
                        .accelerated
                        .as_ref()
                        .map(|snapshot| snapshot.sequence)
                })
                .flatten(),
            fault: None,
            last_store_reason: None,
            dispatcher: None,
            #[cfg(feature = "native-tokio")]
            manual_drive: None,
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            event_publisher: None,
            replay_scope: scope,
            middleware_chain: None,
        })
    }
}

pub(super) fn replay_scoped(
    loaded: &LoadedSession,
    scope: ReplayScope,
) -> Result<(Kernel, u64, Option<Timestamp>, bool), &'static str> {
    match scope {
        ReplayScope::Primary => replay_loaded(loaded),
        ReplayScope::StructuralOnly => {
            replay_filtered(loaded, None).map(|(kernel, next, timer)| (kernel, next, timer, false))
        }
        ReplayScope::Run(run_id) => {
            if let Some(accelerated) = loaded.accelerated.as_ref()
                && accelerated
                    .state
                    .accepted
                    .as_ref()
                    .is_some_and(|accepted| accepted.run_id() == run_id)
                && let Ok((kernel, next, timer)) = replay_from_snapshot(loaded, accelerated)
            {
                return Ok((kernel, next, timer, true));
            }
            replay_filtered(loaded, Some(run_id))
                .map(|(kernel, next, timer)| (kernel, next, timer, false))
        }
    }
}

fn replay_filtered(
    loaded: &LoadedSession,
    target: Option<RunId>,
) -> Result<(Kernel, u64, Option<Timestamp>), &'static str> {
    let mut kernel = Kernel::default();
    let mut next_transient_sequence = 0_u64;
    let mut last_batch_sequence = 0_u64;
    let mut pending_timer_scheduled_at = None;
    for batch in loaded.committed_batches.iter() {
        last_batch_sequence = batch.last_sequence;
        for record in batch.records.iter() {
            if !record_belongs_to_scope(record, target) {
                continue;
            }
            let prior = record
                .sequence()
                .checked_sub(1)
                .ok_or("filtered_sequence_invalid")?;
            adopt_session_head(&mut kernel, prior)?;
            let single = CommittedBatch::try_new(
                batch.batch_id,
                record.sequence(),
                record.sequence(),
                vec![record.clone()],
            )
            .map_err(|_| "filtered_batch_invalid")?;
            let events = kernel
                .apply(&single, next_transient_sequence)
                .map_err(|_| "journal_replay_failed")?;
            next_transient_sequence = next_transient_sequence
                .checked_add(
                    u64::try_from(events.len())
                        .map_err(|_| "transient_event_sequence_exhausted")?,
                )
                .ok_or("transient_event_sequence_exhausted")?;
            update_pending_timer_timestamp(&mut pending_timer_scheduled_at, &single);
        }
    }
    if last_batch_sequence != loaded.head_sequence {
        return Err("loaded_head_mismatch");
    }
    adopt_session_head(&mut kernel, loaded.head_sequence)?;
    if kernel.state().last_applied_sequence != loaded.head_sequence {
        return Err("loaded_head_mismatch");
    }
    Ok((kernel, next_transient_sequence, pending_timer_scheduled_at))
}

fn record_belongs_to_scope(record: &RecordEnvelope, target: Option<RunId>) -> bool {
    if record.body().is_structural() || record.run_id().is_none() {
        return true;
    }
    target.is_some_and(|run_id| record.run_id() == Some(run_id))
}

fn adopt_session_head(kernel: &mut Kernel, sequence: u64) -> Result<(), &'static str> {
    let current = kernel.state().last_applied_sequence;
    if current == sequence {
        return Ok(());
    }
    if current > sequence {
        return Err("filtered_sequence_regression");
    }
    let mut state = kernel.state().clone();
    state.last_applied_sequence = sequence;
    *kernel = Kernel::try_restore(state).map_err(|_| "filtered_head_invalid")?;
    Ok(())
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

pub(crate) fn project_loaded(loaded: &LoadedSession) -> Result<SessionProjection, &'static str> {
    let mut session = SessionProjection::new(loaded.session_id);
    for batch in loaded.committed_batches.iter() {
        apply_batch_to_session(&mut session, batch)?;
    }
    Ok(session)
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
