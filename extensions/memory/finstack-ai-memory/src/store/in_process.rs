//! In-process reference implementations: [`InProcessMemoryStore`] (this
//! crate's `MemoryStore`) and [`InProcessArtifactStore`], moved here
//! verbatim from the predecessor `finstack-ai-context-memory` crate so the
//! Python bindings can keep linking against it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::Digest;
use finstack_ai_runtime::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, ArtifactStoreLimits, Bytes,
    PortFuture,
};

use crate::record::{MemoryId, MemoryRecord, MemoryScope};

use super::{
    MatchEvidence, MemoryHit, MemoryListing, MemoryPage, MemoryQuery, MemoryStore,
    MemoryStoreError, PutOutcome,
};

/// In-process [`ArtifactStore`] used only by this reference provider.
#[derive(Debug, Default)]
pub struct InProcessArtifactStore {
    bodies: Mutex<BTreeMap<finstack_ai_kernel::ArtifactId, Bytes>>,
    limits: ArtifactStoreLimits,
}

impl InProcessArtifactStore {
    /// Override the artifact byte ceiling for this in-process store.
    #[must_use]
    pub fn with_max_artifact_bytes(mut self, max_artifact_bytes: usize) -> Self {
        self.limits = ArtifactStoreLimits { max_artifact_bytes };
        self
    }
}

impl ArtifactStore for InProcessArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<finstack_ai_kernel::ArtifactRef, ArtifactError>> {
        let digest = Digest::blob_content(&content);
        let mut artifact_id = [0_u8; 16];
        artifact_id.copy_from_slice(&digest.as_bytes()[..16]);
        let artifact_id = finstack_ai_kernel::ArtifactId::from_bytes(artifact_id);
        let stored = self
            .bodies
            .lock()
            .map(|mut guard| {
                guard.insert(artifact_id, content.clone());
            })
            .map_err(|_| ());
        Box::pin(async move {
            stored.map_err(|()| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
            let blob = finstack_ai_kernel::BlobRef::try_new(
                digest.to_hex(),
                metadata.media_type.as_ref(),
                u64::try_from(content.len()).unwrap_or(0),
                Some(digest),
                metadata.name.as_deref(),
            )
            .map_err(|error| ArtifactError::InvalidMetadata {
                message: Arc::from(error.to_string()),
            })?;
            finstack_ai_kernel::ArtifactRef::try_new(
                artifact_id,
                metadata.kind.as_ref(),
                blob,
                digest,
                scope.digest()?,
                metadata.attributes,
            )
            .map_err(|error| ArtifactError::InvalidMetadata {
                message: Arc::from(error.to_string()),
            })
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        artifact: finstack_ai_kernel::ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        let bodies = self
            .bodies
            .lock()
            .map(|guard| guard.get(&artifact.id()).cloned())
            .map_err(|_| ());
        Box::pin(async move {
            let stored = bodies.map_err(|()| ArtifactError::Unavailable {
                message: Arc::from("memory artifact lock failed"),
            })?;
            stored.ok_or(ArtifactError::NotFound)
        })
    }

    fn limits(&self) -> ArtifactStoreLimits {
        self.limits
    }
}

fn lock_error() -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from("memory store lock failed"),
    }
}

/// In-process, in-memory [`MemoryStore`] reference implementation.
///
/// Records and applied idempotency keys live in [`Mutex`]-guarded
/// collections; nothing is persisted across process restarts.
#[derive(Debug, Default)]
pub struct InProcessMemoryStore {
    records: Mutex<BTreeMap<MemoryId, MemoryRecord>>,
    applied_keys: Mutex<BTreeSet<Arc<str>>>,
}

impl InProcessMemoryStore {
    /// Construct an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Mutex::new(BTreeMap::new()),
            applied_keys: Mutex::new(BTreeSet::new()),
        }
    }

    /// Claim `idempotency_key`, returning `true` when it was newly applied
    /// and `false` when it had already been applied.
    fn claim_key(&self, idempotency_key: &Arc<str>) -> Result<bool, MemoryStoreError> {
        let mut keys = self.applied_keys.lock().map_err(|_| lock_error())?;
        Ok(keys.insert(Arc::clone(idempotency_key)))
    }
}

impl MemoryStore for InProcessMemoryStore {
    fn put(
        &self,
        idempotency_key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>> {
        let outcome = (|| -> Result<PutOutcome, MemoryStoreError> {
            record
                .validate()
                .map_err(|_| MemoryStoreError::InvalidRecord {
                    reason: "memory_record_invalid",
                })?;
            // Lock ordering: `records` before `applied_keys`, consistently
            // across every method that needs both, to avoid deadlock.
            let mut records = self.records.lock().map_err(|_| lock_error())?;
            let newly_applied = self.claim_key(&idempotency_key)?;
            if !newly_applied {
                return Ok(PutOutcome::AlreadyApplied);
            }
            records.insert(record.id.clone(), record);
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
            let records = self.records.lock().map_err(|_| lock_error())?;
            Ok(records
                .get(&id)
                .filter(|record| scope.permits(&record.scope))
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
            let records = self.records.lock().map_err(|_| lock_error())?;
            let mut hits: Vec<MemoryHit> = records
                .values()
                .filter(|record| {
                    scope.permits(&record.scope)
                        && !record.tombstoned
                        && record.superseded_by.is_none()
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
            let mut records = self.records.lock().map_err(|_| lock_error())?;
            let record = records.get_mut(&id).ok_or(MemoryStoreError::NotFound)?;
            if !scope.permits(&record.scope) {
                return Err(MemoryStoreError::NotFound);
            }
            let newly_applied = self.claim_key(&idempotency_key)?;
            if !newly_applied {
                return Ok(());
            }
            record.tombstoned = true;
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
            replacement
                .validate()
                .map_err(|_| MemoryStoreError::InvalidRecord {
                    reason: "memory_record_invalid",
                })?;
            let mut records = self.records.lock().map_err(|_| lock_error())?;
            {
                let old_record = records.get(&old).ok_or(MemoryStoreError::NotFound)?;
                if !scope.permits(&old_record.scope) {
                    return Err(MemoryStoreError::NotFound);
                }
            }
            let newly_applied = self.claim_key(&idempotency_key)?;
            if !newly_applied {
                return Ok(());
            }
            replacement.supersedes = Some(old.clone());
            let replacement_id = replacement.id.clone();
            records.insert(replacement_id.clone(), replacement);
            if let Some(old_record) = records.get_mut(&old) {
                old_record.superseded_by = Some(replacement_id);
            }
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
            let records = self.records.lock().map_err(|_| lock_error())?;
            let matching: Vec<MemoryRecord> = records
                .values()
                .filter(|record| scope.permits(&record.scope))
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
            let needle = text.to_ascii_lowercase();
            let preview_hit = record.preview.to_ascii_lowercase().contains(&needle);
            let body_hit = match &record.body {
                crate::record::MemoryBody::Inline(body) => {
                    body.to_ascii_lowercase().contains(&needle)
                }
                crate::record::MemoryBody::Blob(_) => false,
            };
            if preview_hit || body_hit {
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
