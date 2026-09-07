use crate::JournalSearchSource;
use finstack_ai_kernel::{Digest, SessionId};
use finstack_ai_protocol::{ChainAnchor, verify_chain_from};
use finstack_ai_runtime::ports::journal::{LoadRequest, ScanRequest, StoreError};
use finstack_ai_search_core::{SearchError, SearchEvidence};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Bounded committed-record maintenance outcome for one authorized session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalIndexReport {
    /// Session that was explicitly authorized by the host.
    pub session: SessionId,
    /// Committed envelopes examined during this call.
    pub examined: usize,
    /// Searchable entries written during this call.
    pub indexed: usize,
    /// Next committed sequence for a subsequent incremental drain.
    pub next_sequence: u64,
    /// True when this call reached the available journal head.
    pub complete: bool,
    /// False after snapshot reconstruction or retained-prefix loss.
    pub historical_complete: bool,
    /// Whether verified full-load/snapshot reconstruction replaced scan.
    pub used_load: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Checkpoint {
    pub(crate) next_sequence: u64,
    pub(crate) checksum: Option<Digest>,
    pub(crate) anchor_sequence: u64,
    pub(crate) anchor_checksum: Option<Digest>,
    pub(crate) historical_complete: bool,
    pub(crate) complete: bool,
}
impl Default for Checkpoint {
    fn default() -> Self {
        Self {
            next_sequence: 1,
            checksum: None,
            anchor_sequence: 1,
            anchor_checksum: None,
            historical_complete: true,
            complete: false,
        }
    }
}
pub(crate) struct IndexedEntry {
    pub(crate) evidence: SearchEvidence,
    pub(crate) timestamp: Option<finstack_ai_kernel::Timestamp>,
}
impl JournalSearchSource {
    /// Read and index at most `max_records` committed envelopes. Repeated calls
    /// continue from a durable checksum-bound cursor. An observer hint merely
    /// schedules this method; event payloads are never indexing input.
    ///
    /// # Errors
    /// Rejects unauthorized sessions, invalid bounds, broken journal chains,
    /// unavailable reads, or exhausted entry capacity. Scan-unavailable/pruned
    /// histories use verified load/snapshot reconstruction with explicit gaps.
    pub async fn sync_session(
        &self,
        session: SessionId,
        max_records: usize,
    ) -> Result<JournalIndexReport, SearchError> {
        self.authorize_session(session)?;
        if max_records == 0 || max_records > self.config.limits.max_scan_records {
            return Err(SearchError::invalid("journal_backfill_limit"));
        }
        let _guard = self
            .maintenance
            .try_lock()
            .map_err(|_| SearchError::SearchUnavailable)?;
        let mut state = self.checkpoint(session).await?.unwrap_or_default();
        if state.anchor_checksum.is_some() && !self.anchor_matches(session, &state).await? {
            return self.replace_from_load(session, max_records).await;
        }
        let mut report = JournalIndexReport {
            session,
            examined: 0,
            indexed: 0,
            next_sequence: state.next_sequence,
            complete: false,
            historical_complete: state.historical_complete,
            used_load: false,
        };
        while report.examined < max_records {
            let limit = u32::try_from((max_records - report.examined).min(256))
                .map_err(|_| SearchError::invalid("journal_page_limit"))?;
            let page = match self
                .read(self.store.scan(ScanRequest {
                    session_id: session,
                    from_sequence: state.next_sequence,
                    limit,
                }))
                .await
            {
                Ok(page) => page,
                Err(SearchError::SearchUnsupported) => {
                    return self.replace_from_load(session, max_records).await;
                }
                Err(error) => return Err(error),
            };
            if page.session_id != session || page.records.len() > limit as usize {
                return Err(SearchError::invalid("journal_scan_page"));
            }
            if page
                .records
                .first()
                .is_some_and(|r| r.sequence() != state.next_sequence)
            {
                return self.replace_from_load(session, max_records).await;
            }
            if page.records.is_empty() && state.next_sequence == 1 {
                return self.replace_from_load(session, max_records).await;
            }
            if page.records.is_empty() && page.next_sequence.is_some() {
                return Err(SearchError::invalid("journal_scan_cursor"));
            }
            let prior = state.clone();
            let anchor = ChainAnchor::try_new(session, state.next_sequence, state.checksum)
                .map_err(|_| SearchError::invalid("journal_anchor"))?;
            state.checksum = verify_chain_from(&page.records, anchor)
                .map_err(|_| SearchError::invalid("journal_chain"))?;
            if let Some(first) = page.records.first()
                && state.anchor_checksum.is_none()
            {
                state.anchor_sequence = first.sequence();
                state.anchor_checksum = Some(first.checksum());
            }
            if let Some(last) = page.records.last() {
                state.next_sequence = last
                    .sequence()
                    .checked_add(1)
                    .ok_or_else(|| SearchError::invalid("journal_sequence"))?;
            }
            if page
                .next_sequence
                .is_some_and(|next| next != state.next_sequence)
            {
                return Err(SearchError::invalid("journal_scan_cursor"));
            }
            state.complete = page.next_sequence.is_none();
            let entries = crate::extract::records(self, &page.records)?;
            report.examined += page.records.len();
            report.indexed += entries.len();
            self.write_entries(session, Some(prior), state.clone(), entries, false)
                .await?;
            if state.complete {
                break;
            }
        }
        report.next_sequence = state.next_sequence;
        report.complete = state.complete;
        Ok(report)
    }
    async fn replace_from_load(
        &self,
        session: SessionId,
        max_records: usize,
    ) -> Result<JournalIndexReport, SearchError> {
        let expected = self.checkpoint(session).await?.unwrap_or_default();
        let loaded = self
            .read(self.store.load(LoadRequest {
                session_id: session,
            }))
            .await?;
        let view = crate::extract::loaded(self, session, &loaded, max_records)?;
        let report = JournalIndexReport {
            session,
            examined: view.examined,
            indexed: view.entries.len(),
            next_sequence: view.state.next_sequence,
            complete: true,
            historical_complete: view.state.historical_complete,
            used_load: true,
        };
        self.write_entries(session, Some(expected), view.state, view.entries, true)
            .await?;
        Ok(report)
    }
    pub(crate) async fn checkpoint(
        &self,
        session: SessionId,
    ) -> Result<Option<Checkpoint>, SearchError> {
        let scope = self.scope_digest.clone();
        self.database
            .call(move |db| checkpoint(db, &scope, session))
            .await
    }
    pub(crate) async fn anchor_matches(
        &self,
        session: SessionId,
        state: &Checkpoint,
    ) -> Result<bool, SearchError> {
        match self
            .read(self.store.scan(ScanRequest {
                session_id: session,
                from_sequence: 0,
                limit: 1,
            }))
            .await
        {
            Ok(page) => Ok(page.session_id == session
                && page.records.len() == 1
                && page.records.first().is_some_and(|r| {
                    r.sequence() == state.anchor_sequence
                        && Some(r.checksum()) == state.anchor_checksum
                })),
            Err(SearchError::SearchUnsupported) => {
                let loaded = self
                    .read(self.store.load(LoadRequest {
                        session_id: session,
                    }))
                    .await?;
                let view = crate::extract::loaded(
                    self,
                    session,
                    &loaded,
                    self.config.limits.max_scan_records,
                )?;
                Ok(view.state.anchor_sequence == state.anchor_sequence
                    && view.state.anchor_checksum == state.anchor_checksum)
            }
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn live(
        &self,
        session: SessionId,
        state: &Checkpoint,
    ) -> Result<bool, SearchError> {
        if !self.anchor_matches(session, state).await? {
            return Err(SearchError::invalid("journal_rebuild_required"));
        }
        match self
            .read(self.store.scan(ScanRequest {
                session_id: session,
                from_sequence: state.next_sequence,
                limit: 1,
            }))
            .await
        {
            Ok(page) => {
                if page.session_id != session || page.records.len() > 1 {
                    return Err(SearchError::invalid("journal_scan_page"));
                }
                if let Some(record) = page.records.first() {
                    let anchor = ChainAnchor::try_new(session, state.next_sequence, state.checksum)
                        .map_err(|_| SearchError::invalid("journal_anchor"))?;
                    verify_chain_from(&page.records, anchor)
                        .map_err(|_| SearchError::invalid("journal_chain"))?;
                    if record.sequence() != state.next_sequence {
                        return Err(SearchError::invalid("journal_rebuild_required"));
                    }
                    crate::extract::authorize_record(self, record)?;
                    Ok(false)
                } else {
                    Ok(state.complete)
                }
            }
            Err(SearchError::SearchUnsupported) => {
                let loaded = self
                    .read(self.store.load(LoadRequest {
                        session_id: session,
                    }))
                    .await?;
                let view = crate::extract::loaded(
                    self,
                    session,
                    &loaded,
                    self.config.limits.max_scan_records,
                )?;
                Ok(view.state.next_sequence == state.next_sequence
                    && view.state.checksum == state.checksum)
            }
            Err(error) => Err(error),
        }
    }
    async fn write_entries(
        &self,
        session: SessionId,
        expected: Option<Checkpoint>,
        state: Checkpoint,
        entries: Vec<IndexedEntry>,
        replace: bool,
    ) -> Result<(), SearchError> {
        let scope = self.scope_digest.clone();
        let max = self.config.max_entries;
        self.database.call(move |db| {
            let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(sql_error)?;
            let current=checkpoint(&tx,&scope,session)?;
            if expected.is_some_and(|expected|current.unwrap_or_default()!=expected) {return Err(SearchError::SearchUnavailable)}
            if replace {tx.execute("DELETE FROM entries WHERE scope=?1 AND session=?2",params![scope,session.to_string()]).map_err(sql_error)?;}
            for entry in entries {write_entry(&tx,&scope,session,&entry)?;}
            let count:usize=tx.query_row("SELECT count(*) FROM entries WHERE scope=?1",[&scope],|r|r.get(0)).map_err(sql_error)?;
            if count>max {return Err(SearchError::SearchCapacityExceeded {resource:"journal_entries".into()})}
            let encoded=serde_json::to_string(&state).map_err(|_|SearchError::invalid("journal_checkpoint"))?;
            tx.execute("INSERT INTO checkpoints VALUES(?1,?2,?3) ON CONFLICT(scope,session) DO UPDATE SET state=excluded.state",params![scope,session.to_string(),encoded]).map_err(sql_error)?;
            tx.commit().map_err(sql_error)
        }).await
    }
}
fn write_entry(
    db: &Connection,
    scope: &str,
    session: SessionId,
    entry: &IndexedEntry,
) -> Result<(), SearchError> {
    let finstack_ai_search_core::SourceRef::JournalSpan {
        session: found,
        lane,
        entry: id,
        ..
    } = &entry.evidence.hit.reference
    else {
        return Err(SearchError::invalid("journal_reference"));
    };
    if *found != session {
        return Err(SearchError::SearchScopeDenied);
    }
    let hit = serde_json::to_string(&entry.evidence.hit)
        .map_err(|_| SearchError::invalid("journal_entry"))?;
    db.execute("INSERT INTO entries(scope,session,lane,entry,timestamp,hit_json,text,complete) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(scope,session,lane,entry) DO UPDATE SET timestamp=excluded.timestamp,hit_json=excluded.hit_json,text=excluded.text,complete=excluded.complete",params![scope,session.to_string(),lane.to_string(),id.to_string(),entry.timestamp.map(|t|t.to_string()),hit,entry.evidence.text.as_ref(),entry.evidence.complete]).map_err(sql_error)?;
    Ok(())
}
fn checkpoint(
    db: &Connection,
    scope: &str,
    session: SessionId,
) -> Result<Option<Checkpoint>, SearchError> {
    let text: Option<String> = db
        .query_row(
            "SELECT state FROM checkpoints WHERE scope=?1 AND session=?2",
            params![scope, session.to_string()],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    text.map(|text| {
        serde_json::from_str(&text).map_err(|_| SearchError::invalid("journal_checkpoint"))
    })
    .transpose()
}
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn sql_error(_: rusqlite::Error) -> SearchError {
    SearchError::SearchUnavailable
}
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn store_error(error: StoreError) -> SearchError {
    match error {
        StoreError::InvalidRequest {
            reason_code: "scan_unsupported",
        } => SearchError::SearchUnsupported,
        StoreError::Corruption { .. } | StoreError::Integrity { .. } => {
            SearchError::invalid("journal_integrity")
        }
        _ => SearchError::SearchUnavailable,
    }
}
