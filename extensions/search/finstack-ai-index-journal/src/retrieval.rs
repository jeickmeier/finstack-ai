use crate::{JournalSearchSource, indexing::sql_error};
use finstack_ai_kernel::SessionId;
use finstack_ai_search_core::{
    LexicalKind, SearchError, SearchEvidence, SearchHit, SearchLimits, SearchQuery, SearchStrategy,
    SourceRef, SourceResult, SourceStatus,
};
use rusqlite::{Connection, OptionalExtension, params};

impl JournalSearchSource {
    pub(crate) async fn retrieve(
        &self,
        query: SearchQuery,
        limit: usize,
    ) -> Result<SourceResult, SearchError> {
        let sessions = if query.journal.sessions.is_empty() {
            self.config.sessions.clone()
        } else {
            query.journal.sessions.clone()
        };
        if sessions.is_empty() {
            return Ok(SourceResult::completed(vec![], 0));
        }
        let mut alive = Vec::new();
        let mut result = SourceResult::completed(vec![], 0);
        for session in sessions {
            let Some(state) = self.checkpoint(session).await? else {
                result.truncate("journal_index_pending");
                result.historical_complete = false;
                continue;
            };
            if !state.historical_complete {
                result.historical_complete = false;
                result.truncate("journal_history_pruned");
            }
            match self.live(session, &state).await {
                Ok(complete) => {
                    alive.push(session);
                    if !complete {
                        result.truncate("journal_index_pending");
                    }
                }
                Err(SearchError::SearchScopeDenied) => return Err(SearchError::SearchScopeDenied),
                Err(SearchError::SearchInvalid { .. }) => {
                    result.truncate("journal_rebuild_required");
                    result.historical_complete = false;
                }
                Err(_) => result.truncate("journal_sources_unavailable"),
            }
        }
        if alive.is_empty() {
            result.status = SourceStatus::Unavailable;
            return Ok(result);
        }
        let scope = self.scope_digest.clone();
        let limits = self.config.limits;
        let selected = query.clone();
        let retrieved = self
            .database
            .call(move |db| search(db, &scope, &alive, &selected, limits, limit))
            .await?;
        result.examined = retrieved.examined as u64;
        if retrieved.truncated {
            result.truncate("journal_scan_or_result_limit");
        }
        if retrieved.incomplete {
            result.truncate("journal_entry_text_incomplete");
        }
        for hit in &retrieved.hits {
            self.validate_hit(hit, &query)?;
        }
        result.hits = retrieved.hits;
        Ok(result)
    }
    fn validate_hit(&self, hit: &SearchHit, query: &SearchQuery) -> Result<(), SearchError> {
        hit.validate(&self.config.limits)?;
        if hit.source != self.config.source_id || hit.provenance.scope != self.config.scope {
            return Err(SearchError::SearchScopeDenied);
        }
        let SourceRef::JournalSpan { session, lane, .. } = &hit.reference else {
            return Err(SearchError::invalid("journal_reference"));
        };
        self.authorize_session(*session)?;
        if (!query.journal.sessions.is_empty() && !query.journal.sessions.contains(session))
            || (!query.journal.lanes.is_empty() && !query.journal.lanes.contains(lane))
        {
            return Err(SearchError::SearchScopeDenied);
        }
        Ok(())
    }
    pub(crate) async fn exact_evidence(
        &self,
        reference: SourceRef,
    ) -> Result<Option<SearchEvidence>, SearchError> {
        let SourceRef::JournalSpan {
            session,
            lane,
            entry,
            ..
        } = &reference
        else {
            return Err(SearchError::SearchUnsupported);
        };
        let session = *session;
        self.authorize_session(session)?;
        let Some(state) = self.checkpoint(session).await? else {
            return Err(SearchError::SearchUnavailable);
        };
        self.live(session, &state).await?;
        let scope = self.scope_digest.clone();
        let lane = lane.to_string();
        let entry = entry.to_string();
        let evidence=self.database.call(move |db| {
            let row:Option<(String,String,bool)>=db.query_row("SELECT hit_json,text,complete FROM entries WHERE scope=?1 AND session=?2 AND lane=?3 AND entry=?4",params![scope,session.to_string(),lane,entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql_error)?;
            row.map(|(hit,text,complete)|Ok(SearchEvidence {hit:serde_json::from_str(&hit).map_err(|_|SearchError::invalid("journal_entry"))?,text:text.into(),complete})).transpose()
        }).await?;
        if let Some(evidence) = &evidence {
            self.validate_hit(
                &evidence.hit,
                &SearchQuery::lexical("evidence", LexicalKind::Literal),
            )?;
            evidence.validate(&self.config.limits)?;
            if evidence.hit.reference != reference {
                return Ok(None);
            }
        }
        Ok(evidence)
    }
}
struct Retrieved {
    hits: Vec<SearchHit>,
    examined: usize,
    truncated: bool,
    incomplete: bool,
}
const FILTER: &str = "e.scope=?1 AND e.session IN (SELECT value FROM json_each(?2)) AND (?3='[]' OR e.lane IN (SELECT value FROM json_each(?3))) AND (?4 IS NULL OR e.timestamp>=?4) AND (?5 IS NULL OR e.timestamp<=?5)";
fn search(
    db: &Connection,
    scope: &str,
    sessions: &[SessionId],
    query: &SearchQuery,
    limits: SearchLimits,
    limit: usize,
) -> Result<Retrieved, SearchError> {
    let sessions =
        serde_json::to_string(sessions).map_err(|_| SearchError::invalid("journal_sessions"))?;
    let lanes = serde_json::to_string(&query.journal.lanes)
        .map_err(|_| SearchError::invalid("journal_lanes"))?;
    let after = query.journal.after.map(|t| t.to_string());
    let before = query.journal.before.map(|t| t.to_string());
    let total: usize = db
        .query_row(
            &format!("SELECT count(*) FROM entries e WHERE {FILTER}"),
            params![scope, sessions, lanes, after, before],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let mut result = Retrieved {
        hits: Vec::new(),
        examined: total.min(limits.max_scan_records),
        truncated: total > limits.max_scan_records,
        incomplete: false,
    };
    if matches!(
        query.strategy,
        SearchStrategy::Lexical(LexicalKind::Keyword | LexicalKind::Bm25)
    ) {
        let expression = fts_expression(&query.text)?;
        let sql = format!(
            "WITH bounded AS MATERIALIZED (SELECT e.rowid,e.hit_json,e.complete,e.session,e.lane,e.entry FROM entries e WHERE {FILTER} ORDER BY e.session,e.lane,e.entry LIMIT ?6) SELECT e.hit_json,e.complete FROM bounded e JOIN entries_fts ON entries_fts.rowid=e.rowid WHERE entries_fts MATCH ?7 ORDER BY bm25(entries_fts),e.session,e.lane,e.entry LIMIT ?8"
        );
        let mut stmt = db.prepare(&sql).map_err(sql_error)?;
        let rows = stmt
            .query_map(
                params![
                    scope,
                    sessions,
                    lanes,
                    after,
                    before,
                    limits.max_scan_records,
                    expression,
                    limit + 1
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .map_err(sql_error)?;
        for row in rows {
            let (hit, complete) = row.map_err(sql_error)?;
            if result.hits.len() == limit {
                result.truncated = true;
                break;
            }
            let mut hit: SearchHit =
                serde_json::from_str(&hit).map_err(|_| SearchError::invalid("journal_hit"))?;
            hit.score = u32::MAX
                - u32::try_from(result.hits.len())
                    .map_err(|_| SearchError::invalid("journal_rank"))?;
            result.incomplete |= !complete;
            result.hits.push(hit);
        }
    } else {
        let regex = if matches!(query.strategy, SearchStrategy::Lexical(LexicalKind::Regex)) {
            Some(query.regex(&limits)?)
        } else {
            None
        };
        let mut stmt=db.prepare(&format!("SELECT e.hit_json,e.text,e.complete FROM entries e WHERE {FILTER} ORDER BY e.session,e.lane,e.entry LIMIT ?6")).map_err(sql_error)?;
        let mut rows = stmt
            .query(params![
                scope,
                sessions,
                lanes,
                after,
                before,
                limits.max_scan_records
            ])
            .map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let text: String = row.get(1).map_err(sql_error)?;
            if !regex.as_ref().map_or_else(
                || text.contains(query.text.as_ref()),
                |regex| regex.is_match(&text),
            ) {
                continue;
            }
            if result.hits.len() == limit {
                result.truncated = true;
                continue;
            }
            let encoded: String = row.get(0).map_err(sql_error)?;
            let hit =
                serde_json::from_str(&encoded).map_err(|_| SearchError::invalid("journal_hit"))?;
            result.incomplete |= !row.get::<_, bool>(2).map_err(sql_error)?;
            result.hits.push(hit);
        }
    }
    Ok(result)
}
pub(crate) fn fts_expression(text: &str) -> Result<String, SearchError> {
    finstack_ai_search_core::fts_expression(text)
        .map_err(|_| SearchError::invalid("journal_lexical_terms"))
}
