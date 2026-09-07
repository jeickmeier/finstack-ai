use crate::{JournalIndexReport, JournalSearchSource};
use finstack_ai_kernel::{
    ComponentId, ComponentRef, Metadata, RunEvent, RunEventClass, SessionId, Version,
};
use finstack_ai_runtime::ports::{
    PortFuture,
    observer::{Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode},
};
use finstack_ai_search_core::SearchError;
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// Read-only observer that coalesces committed event IDs into bounded session
/// hints. It never stores token events or reads their payloads. The application
/// explicitly drains hints outside the runtime's effect/append path.
#[derive(Clone)]
pub struct JournalIndexObserver {
    source: Arc<JournalSearchSource>,
    pending: Arc<Mutex<BTreeSet<SessionId>>>,
    dropped: Arc<AtomicU64>,
    descriptor: ObserverDescriptor,
}
impl JournalIndexObserver {
    /// Bind a hint observer to one immutable source/authorized-session list.
    ///
    /// # Errors
    /// Returns an error if immutable descriptor encoding fails.
    pub fn try_new(source: Arc<JournalSearchSource>) -> Result<Self, SearchError> {
        let descriptor=ObserverDescriptor {component:ComponentRef::new(ComponentId::parse("finstack.search.journal.hints").map_err(|_|SearchError::invalid("journal_observer_id"))?,Some(Version {major:0,minor:1,patch:0})),payload_mode:ObserverPayloadMode::MetadataOnly,metadata:Metadata::parse(serde_json::to_vec(&serde_json::json!({"configuration_digest":source.configuration_digest().to_hex()})).map_err(|_|SearchError::invalid("journal_observer_config"))?).map_err(|_|SearchError::invalid("journal_observer_config"))?};
        Ok(Self {
            source,
            pending: Arc::new(Mutex::new(BTreeSet::new())),
            dropped: Arc::new(AtomicU64::new(0)),
            descriptor,
        })
    }
    /// Count hints omitted because an incoming host batch exceeded 8,192 events.
    /// Applications should schedule explicit authorized backfill after a loss.
    #[must_use]
    pub fn dropped_hints(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    /// Drain at most 256 coalesced session hints through the same committed-record
    /// path as historical backfill. Failed hints remain pending for a later retry.
    ///
    /// # Errors
    /// Rejects invalid bounds or returns a source read/index failure.
    pub async fn drain(
        &self,
        max_sessions: usize,
        max_records: usize,
    ) -> Result<Vec<JournalIndexReport>, SearchError> {
        if max_sessions == 0 || max_sessions > 256 {
            return Err(SearchError::invalid("journal_hint_limit"));
        }
        let sessions: Vec<_> = self
            .pending
            .lock()
            .map_err(|_| SearchError::SearchUnavailable)?
            .iter()
            .take(max_sessions)
            .copied()
            .collect();
        let mut reports = Vec::new();
        for session in sessions {
            // Remove before awaiting; a concurrent observation can reinsert the
            // hint and is never erased by completion of this older drain.
            self.pending
                .lock()
                .map_err(|_| SearchError::SearchUnavailable)?
                .remove(&session);
            match self.source.sync_session(session, max_records).await {
                Ok(report) => {
                    if !report.complete {
                        self.pending
                            .lock()
                            .map_err(|_| SearchError::SearchUnavailable)?
                            .insert(session);
                    }
                    reports.push(report);
                }
                Err(error) => {
                    self.pending
                        .lock()
                        .map_err(|_| SearchError::SearchUnavailable)?
                        .insert(session);
                    return Err(error);
                }
            }
        }
        Ok(reports)
    }
}
impl Observer for JournalIndexObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }
    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let pending = self.pending.clone();
        let source = self.source.clone();
        let dropped = self.dropped.clone();
        Box::pin(async move {
            let Ok(mut hints) = pending.lock() else {
                dropped.fetch_add(batch.len() as u64, Ordering::Relaxed);
                return Ok(());
            };
            dropped.fetch_add(batch.len().saturating_sub(8192) as u64, Ordering::Relaxed);
            for event in batch.iter().take(8192) {
                if event.class() == RunEventClass::DurableDerived
                    && source.config.sessions.contains(&event.session_id())
                {
                    hints.insert(event.session_id());
                }
            }
            Ok(())
        })
    }
}
