//! In-process, in-memory [`MemoryStore`] reference implementation.
//!
//! The companion [`finstack_ai_runtime::artifact::InProcessArtifactStore`]
//! is re-exported from [`crate::store`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{Digest, Timestamp};
use finstack_ai_runtime::ports::PortFuture;

use crate::record::{
    INLINE_BODY_MAX_BYTES, KEYWORD_MAX_BYTES, KEYWORDS_MAX_COUNT, MemoryBody, MemoryClock,
    MemoryId, MemoryRecord, MemoryScope,
};

use super::{
    MEMORY_IDEMPOTENCY_KEY_MAX_BYTES, MatchEvidence, MemoryArtifactAction, MemoryHit,
    MemoryListing, MemoryPage, MemoryQuery, MemoryStore, MemoryStoreDescriptor, MemoryStoreError,
    MemoryStoreLimits, PutOutcome, artifact_transition_actions, normalize_search_tokens,
    validate_new_record_lifecycle,
};

/// Whether writing `incoming` at its id conflicts with `existing`.
///
/// A live record always conflicts: ids are a global key, and a collision
/// with another scope's record must not silently replace it. A *tombstoned*
/// record in the identical scope does not conflict — it is the same owner
/// re-remembering something they forgot, and refusing that would strand the
/// id forever, since the supersession route rejects a replacement whose id
/// equals the one it supersedes.
fn conflicts_with(existing: Option<&MemoryRecord>, incoming: &MemoryRecord) -> bool {
    existing.is_some_and(|existing| !(existing.tombstoned && existing.scope == incoming.scope))
}

fn lock_error() -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from("memory store lock failed"),
    }
}

/// In-process, in-memory [`MemoryStore`] reference implementation.
///
/// Records and idempotency receipts live in one [`Mutex`]-guarded state so
/// every mutation, capacity check, and receipt is atomic.
pub struct InProcessMemoryStore {
    state: Mutex<MemoryState>,
    clock: MemoryClock,
    limits: MemoryStoreLimits,
}

#[derive(Debug, Default)]
struct MemoryState {
    records: BTreeMap<(MemoryScope, MemoryId), MemoryRecord>,
    receipts: BTreeMap<(MemoryScope, Arc<str>), Receipt>,
    artifact_actions: VecDeque<MemoryArtifactAction>,
    artifact_action_ids: BTreeSet<Digest>,
    total_inline_bytes: u64,
}

/// Fingerprint plus apply time, so write-path sweeps can age receipts out.
#[derive(Debug, Clone, Copy)]
struct Receipt {
    fingerprint: Digest,
    applied_at: Timestamp,
}

impl core::fmt::Debug for InProcessMemoryStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("InProcessMemoryStore")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Default for InProcessMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessMemoryStore {
    /// Construct an empty store.
    #[must_use]
    pub fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let clock = crate::record::system_clock();
        #[cfg(target_arch = "wasm32")]
        let clock: MemoryClock = Arc::new(|| finstack_ai_kernel::UNIX_EPOCH);
        Self {
            state: Mutex::new(MemoryState::default()),
            clock,
            limits: MemoryStoreLimits::default(),
        }
    }

    /// Override the deterministic operation clock.
    #[must_use]
    pub fn with_clock(mut self, clock: MemoryClock) -> Self {
        self.clock = clock;
        self
    }

    /// Override finite store limits.
    #[must_use]
    pub const fn with_limits(mut self, limits: MemoryStoreLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl MemoryStore for InProcessMemoryStore {
    fn descriptor(&self) -> MemoryStoreDescriptor {
        MemoryStoreDescriptor {
            store_id: Arc::from("memory.in-process-v2"),
            durable: false,
            manages_artifact_ownership: true,
            limits: self.limits,
        }
    }

    fn put(
        &self,
        idempotency_key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>> {
        let outcome = (|| -> Result<PutOutcome, MemoryStoreError> {
            validate_record(&record)?;
            validate_new_record_lifecycle(&record)?;
            validate_idempotency_key(&idempotency_key)?;
            let fingerprint = operation_fingerprint("put", &record)?;
            let record_key = (record.scope.clone(), record.id.clone());
            let now = (self.clock)();
            let mut state = self.state.lock().map_err(|_| lock_error())?;
            cleanup_expired(&mut state, now, self.limits)?;
            if receipt_replay(&state, &record.scope, &idempotency_key, fingerprint)? {
                return Ok(PutOutcome::AlreadyApplied);
            }
            if conflicts_with(state.records.get(&record_key), &record) {
                return Err(MemoryStoreError::IdConflict);
            }
            reserve_receipt(&state, self.limits)?;
            reserve_record(&state, &record, self.limits)?;
            let actions = artifact_transition_actions(
                &idempotency_key,
                state.records.get(&record_key),
                Some(&record),
                now,
            )?;
            reserve_artifact_actions(&state, &actions, self.limits)?;
            let replaced = state.records.insert(record_key, record.clone());
            state.total_inline_bytes = state
                .total_inline_bytes
                .saturating_sub(replaced.as_ref().map_or(0, inline_bytes))
                .saturating_add(inline_bytes(&record));
            state.receipts.insert(
                (record.scope.clone(), idempotency_key),
                Receipt {
                    fingerprint,
                    applied_at: now,
                },
            );
            enqueue_artifact_actions(&mut state, actions);
            Ok(PutOutcome::Inserted)
        })();
        Box::pin(async move { outcome })
    }

    fn get(
        &self,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>> {
        let result = (|| -> Result<Option<MemoryRecord>, MemoryStoreError> {
            validate_scope(&scope)?;
            // Reads filter expiry; they do not sweep.
            let now = (self.clock)();
            let state = self.state.lock().map_err(|_| lock_error())?;
            Ok(state
                .records
                .get(&(scope, id))
                .filter(|record| {
                    !record.tombstoned
                        && record.superseded_by.is_none()
                        && !record.is_expired_at(now)
                })
                .cloned())
        })();
        Box::pin(async move { result })
    }

    fn search(
        &self,
        scope: MemoryScope,
        query: MemoryQuery,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>> {
        let result = (|| -> Result<Vec<MemoryHit>, MemoryStoreError> {
            validate_scope(&scope)?;
            validate_query(&query, limit, self.limits)?;
            let now = (self.clock)();
            let state = self.state.lock().map_err(|_| lock_error())?;
            let mut hits: Vec<MemoryHit> = state
                .records
                .values()
                .filter(|record| {
                    scope == record.scope
                        && !record.tombstoned
                        && record.superseded_by.is_none()
                        && !record.is_expired_at(now)
                })
                .filter_map(|record| match_record(record, &query))
                .collect();
            hits.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| a.record.id.cmp(&b.record.id))
            });
            hits.truncate(limit);
            Ok(hits)
        })();
        Box::pin(async move { result })
    }

    fn forget(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        let result = (|| -> Result<(), MemoryStoreError> {
            validate_scope(&scope)?;
            validate_idempotency_key(&idempotency_key)?;
            let fingerprint = operation_fingerprint("forget", &(&scope, &id))?;
            let record_key = (scope.clone(), id.clone());
            let mut state = self.state.lock().map_err(|_| lock_error())?;
            let now = (self.clock)();
            cleanup_expired(&mut state, now, self.limits)?;
            if receipt_replay(&state, &scope, &idempotency_key, fingerprint)? {
                return Ok(());
            }
            let record = state
                .records
                .get(&record_key)
                .ok_or(MemoryStoreError::NotFound)?;
            if record.tombstoned || record.superseded_by.is_some() {
                return Err(MemoryStoreError::NotFound);
            }
            reserve_receipt(&state, self.limits)?;
            let actions = artifact_transition_actions(
                &idempotency_key,
                state.records.get(&record_key),
                None,
                now,
            )?;
            reserve_artifact_actions(&state, &actions, self.limits)?;
            let record = state
                .records
                .get_mut(&record_key)
                .ok_or(MemoryStoreError::NotFound)?;
            record.tombstoned = true;
            state.receipts.insert(
                (scope, idempotency_key),
                Receipt {
                    fingerprint,
                    applied_at: now,
                },
            );
            enqueue_artifact_actions(&mut state, actions);
            Ok(())
        })();
        Box::pin(async move { result })
    }

    fn correct(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        mut replacement: MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        let result = (|| -> Result<(), MemoryStoreError> {
            validate_record(&replacement)?;
            validate_scope(&scope)?;
            validate_idempotency_key(&idempotency_key)?;
            // A record that supersedes itself would be hidden from search and
            // recall forever, with no surviving replacement to find.
            if replacement.id == old {
                return Err(MemoryStoreError::InvalidRecord {
                    reason: "memory_self_supersession",
                });
            }
            if replacement.tombstoned {
                return Err(MemoryStoreError::InvalidRecord {
                    reason: "memory_record_lifecycle_not_initial",
                });
            }
            if replacement.supersedes.is_some() || replacement.superseded_by.is_some() {
                return Err(MemoryStoreError::InvalidRecord {
                    reason: "memory_replacement_already_linked",
                });
            }
            let fingerprint = operation_fingerprint("correct", &(&scope, &old, &replacement))?;
            let old_key = (scope.clone(), old.clone());
            let replacement_key = (replacement.scope.clone(), replacement.id.clone());
            let mut state = self.state.lock().map_err(|_| lock_error())?;
            let now = (self.clock)();
            cleanup_expired(&mut state, now, self.limits)?;
            if receipt_replay(&state, &scope, &idempotency_key, fingerprint)? {
                return Ok(());
            }
            let old_record = state
                .records
                .get(&old_key)
                .ok_or(MemoryStoreError::NotFound)?;
            if old_record.tombstoned || old_record.superseded_by.is_some() {
                return Err(MemoryStoreError::NotFound);
            }
            if scope != old_record.scope || replacement.scope != old_record.scope {
                return Err(MemoryStoreError::NotFound);
            }
            if conflicts_with(state.records.get(&replacement_key), &replacement) {
                return Err(MemoryStoreError::IdConflict);
            }
            reserve_receipt(&state, self.limits)?;
            reserve_record(&state, &replacement, self.limits)?;
            let actions = artifact_transition_actions(
                &idempotency_key,
                Some(old_record),
                Some(&replacement),
                now,
            )?;
            reserve_artifact_actions(&state, &actions, self.limits)?;
            replacement.supersedes = Some(old.clone());
            let replacement_id = replacement.id.clone();
            let replaced = state.records.insert(replacement_key, replacement.clone());
            state.total_inline_bytes = state
                .total_inline_bytes
                .saturating_sub(replaced.as_ref().map_or(0, inline_bytes))
                .saturating_add(inline_bytes(&replacement));
            if let Some(old_record) = state.records.get_mut(&old_key) {
                old_record.superseded_by = Some(replacement_id);
            }
            state.receipts.insert(
                (scope, idempotency_key),
                Receipt {
                    fingerprint,
                    applied_at: now,
                },
            );
            enqueue_artifact_actions(&mut state, actions);
            Ok(())
        })();
        Box::pin(async move { result })
    }

    fn list(
        &self,
        scope: MemoryScope,
        page: MemoryPage,
    ) -> PortFuture<Result<MemoryListing, MemoryStoreError>> {
        let result = (|| -> Result<MemoryListing, MemoryStoreError> {
            validate_scope(&scope)?;
            if page.limit > self.limits.max_page_size {
                return Err(MemoryStoreError::InvalidRequest {
                    reason: "memory_page_limit_exceeded",
                });
            }
            let now = (self.clock)();
            let state = self.state.lock().map_err(|_| lock_error())?;
            let matching: Vec<MemoryRecord> = state
                .records
                .values()
                .filter(|record| {
                    scope == record.scope
                        && !record.tombstoned
                        && record.superseded_by.is_none()
                        && !record.is_expired_at(now)
                })
                .cloned()
                .collect();
            let total = matching.len();
            let records = matching
                .into_iter()
                .skip(page.offset)
                .take(page.limit)
                .collect();
            Ok(MemoryListing { records, total })
        })();
        Box::pin(async move { result })
    }

    fn pending_artifact_actions(
        &self,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryArtifactAction>, MemoryStoreError>> {
        let result = (|| {
            if limit > self.limits.max_artifact_actions {
                return Err(MemoryStoreError::InvalidRequest {
                    reason: "memory_artifact_action_limit_exceeded",
                });
            }
            let state = self.state.lock().map_err(|_| lock_error())?;
            Ok(state.artifact_actions.iter().take(limit).cloned().collect())
        })();
        Box::pin(async move { result })
    }

    fn acknowledge_artifact_action(
        &self,
        action_id: Digest,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        let result = (|| {
            let mut state = self.state.lock().map_err(|_| lock_error())?;
            if state.artifact_action_ids.remove(&action_id) {
                state
                    .artifact_actions
                    .retain(|action| action.action_id() != action_id);
            }
            Ok(())
        })();
        Box::pin(async move { result })
    }
}

fn validate_record(record: &MemoryRecord) -> Result<(), MemoryStoreError> {
    record
        .validate()
        .map_err(|error| MemoryStoreError::InvalidRecord {
            reason: match error {
                crate::record::MemoryError::InvalidRecord { reason }
                | crate::record::MemoryError::Configuration { reason } => reason,
            },
        })
}

fn validate_scope(scope: &MemoryScope) -> Result<(), MemoryStoreError> {
    scope
        .validate()
        .map_err(|_| MemoryStoreError::InvalidRequest {
            reason: "memory_scope_invalid",
        })
}

fn validate_idempotency_key(key: &str) -> Result<(), MemoryStoreError> {
    if key.is_empty() || key.len() > MEMORY_IDEMPOTENCY_KEY_MAX_BYTES || key.as_bytes().contains(&0)
    {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_idempotency_key_invalid",
        });
    }
    Ok(())
}

fn validate_query(
    query: &MemoryQuery,
    limit: usize,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    if limit > limits.max_search_results {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_search_limit_exceeded",
        });
    }
    match query {
        MemoryQuery::ExactId(_) => Ok(()),
        MemoryQuery::Keywords(keywords) => {
            if keywords.is_empty()
                || keywords.len() > KEYWORDS_MAX_COUNT
                || keywords.iter().any(|keyword| {
                    keyword.is_empty()
                        || keyword.len() > KEYWORD_MAX_BYTES
                        || keyword.as_bytes().contains(&0)
                })
            {
                return Err(MemoryStoreError::InvalidRequest {
                    reason: "memory_query_keywords_invalid",
                });
            }
            Ok(())
        }
        MemoryQuery::FullText(text) => {
            if text.len() > INLINE_BODY_MAX_BYTES || text.as_bytes().contains(&0) {
                return Err(MemoryStoreError::InvalidRequest {
                    reason: "memory_query_text_invalid",
                });
            }
            Ok(())
        }
    }
}

fn operation_fingerprint<T: serde::Serialize>(
    operation: &'static str,
    payload: &T,
) -> Result<Digest, MemoryStoreError> {
    let encoded = serde_json_canonicalizer::to_vec(&(operation, payload)).map_err(|_| {
        MemoryStoreError::InvalidRequest {
            reason: "memory_idempotency_payload_invalid",
        }
    })?;
    Digest::domain_separated("memory-idempotency", 1, &encoded).map_err(|_| {
        MemoryStoreError::InvalidRequest {
            reason: "memory_idempotency_payload_invalid",
        }
    })
}

fn receipt_replay(
    state: &MemoryState,
    scope: &MemoryScope,
    key: &Arc<str>,
    fingerprint: Digest,
) -> Result<bool, MemoryStoreError> {
    match state.receipts.get(&(scope.clone(), Arc::clone(key))) {
        None => Ok(false),
        Some(existing) if existing.fingerprint == fingerprint => Ok(true),
        Some(_) => Err(MemoryStoreError::IdempotencyConflict),
    }
}

fn reserve_receipt(state: &MemoryState, limits: MemoryStoreLimits) -> Result<(), MemoryStoreError> {
    if state.receipts.len() >= limits.max_idempotency_keys {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "idempotency_keys",
            limit: u64::try_from(limits.max_idempotency_keys).unwrap_or(u64::MAX),
        });
    }
    Ok(())
}

fn reserve_record(
    state: &MemoryState,
    incoming: &MemoryRecord,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let existing = state
        .records
        .get(&(incoming.scope.clone(), incoming.id.clone()));
    if existing.is_none() && state.records.len() >= limits.max_records {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "records",
            limit: u64::try_from(limits.max_records).unwrap_or(u64::MAX),
        });
    }
    let next_bytes = state
        .total_inline_bytes
        .saturating_sub(existing.map_or(0, inline_bytes))
        .checked_add(inline_bytes(incoming))
        .ok_or(MemoryStoreError::CapacityExceeded {
            resource: "inline_bytes",
            limit: limits.max_inline_bytes,
        })?;
    if next_bytes > limits.max_inline_bytes {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "inline_bytes",
            limit: limits.max_inline_bytes,
        });
    }
    Ok(())
}

/// Write-path sweep: drop expired records (enqueueing artifact unpins) and
/// age out idempotency receipts.
fn cleanup_expired(
    state: &mut MemoryState,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let receipt_cutoff = now
        .as_unix_ms()
        .saturating_sub(i64::try_from(limits.max_receipt_age_ms).unwrap_or(i64::MAX));
    state
        .receipts
        .retain(|_, receipt| receipt.applied_at.as_unix_ms() > receipt_cutoff);
    let expired = state
        .records
        .iter()
        .filter_map(|(key, record)| record.is_expired_at(now).then_some(key.clone()))
        .collect::<Vec<_>>();
    let mut actions = Vec::new();
    for key in &expired {
        if let Some(record) = state.records.get(key) {
            actions.extend(artifact_transition_actions(
                &format!(
                    "expiry:{}:{}",
                    record.id.as_str(),
                    record.created_at.as_unix_ms()
                ),
                Some(record),
                None,
                now,
            )?);
        }
    }
    reserve_artifact_actions(state, &actions, limits)?;
    for key in expired {
        if let Some(record) = state.records.remove(&key) {
            state.total_inline_bytes = state
                .total_inline_bytes
                .saturating_sub(inline_bytes(&record));
        }
    }
    enqueue_artifact_actions(state, actions);
    Ok(())
}

fn reserve_artifact_actions(
    state: &MemoryState,
    actions: &[MemoryArtifactAction],
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let additional = actions
        .iter()
        .filter(|action| !state.artifact_action_ids.contains(&action.action_id()))
        .count();
    if state.artifact_actions.len().saturating_add(additional) > limits.max_artifact_actions {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "artifact_actions",
            limit: u64::try_from(limits.max_artifact_actions).unwrap_or(u64::MAX),
        });
    }
    Ok(())
}

fn enqueue_artifact_actions(state: &mut MemoryState, actions: Vec<MemoryArtifactAction>) {
    for action in actions {
        if state.artifact_action_ids.insert(action.action_id()) {
            state.artifact_actions.push_back(action);
        }
    }
}

fn inline_bytes(record: &MemoryRecord) -> u64 {
    match &record.body {
        MemoryBody::Inline(body) => u64::try_from(body.len()).unwrap_or(u64::MAX),
        MemoryBody::Blob { .. } => 0,
    }
}

fn match_record(record: &MemoryRecord, query: &MemoryQuery) -> Option<MemoryHit> {
    match query {
        MemoryQuery::ExactId(id) => {
            if &record.id == id {
                Some(MemoryHit {
                    record: record.clone(),
                    score: 100,
                    matched: MatchEvidence::ExactId,
                })
            } else {
                None
            }
        }
        MemoryQuery::Keywords(keywords) => {
            let mut matched_count = 0_u32;
            let mut first_match: Option<Arc<str>> = None;
            for keyword in keywords.iter() {
                if record
                    .keywords
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(keyword))
                {
                    matched_count += 1;
                    if first_match.is_none() {
                        first_match = Some(Arc::clone(keyword));
                    }
                }
            }
            first_match.map(|keyword| MemoryHit {
                record: record.clone(),
                score: matched_count,
                matched: MatchEvidence::Keyword(keyword),
            })
        }
        MemoryQuery::FullText(text) => {
            let haystack = {
                let mut haystack = record.preview.to_string();
                if let crate::record::MemoryBody::Inline(body) = &record.body {
                    haystack.push(' ');
                    haystack.push_str(body);
                }
                haystack
            };
            let haystack_tokens = normalize_search_tokens(&haystack);
            let matched_any = normalize_search_tokens(text).iter().any(|query| {
                haystack_tokens
                    .iter()
                    .any(|candidate| candidate.starts_with(query))
            });
            if matched_any {
                Some(MemoryHit {
                    record: record.clone(),
                    score: 10,
                    matched: MatchEvidence::FullText,
                })
            } else {
                None
            }
        }
    }
}
