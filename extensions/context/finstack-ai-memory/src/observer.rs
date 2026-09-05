//! [`Observer`] adapter that captures extracted candidate memories into a
//! [`MemoryStore`].
//!
//! Capture is best-effort: a failing [`MemoryStore::put`] is recorded in
//! bounded adapter-owned diagnostics but never surfaced as an
//! [`ObserverError`], because losing an opportunistic memory capture must
//! never look like run impact.
//! Only misconfiguration at construction time — an invalid component id —
//! fails [`MemoryObserver::try_new`].
//!
//! Captured bodies are always inline, so a candidate whose body exceeds
//! [`INLINE_BODY_MAX_BYTES`] is skipped rather than stored unbounded.
//!
//! With an embedder ([`MemoryObserver::try_new_with_embedder`]), each
//! observed batch ends in a bounded, error-swallowed drain of the store's
//! embedding index; the same failure isolation applies to it absolutely.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_embeddings::embedder::TextEmbedder;
use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, RunEvent, Sensitivity, Version};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::observer::{
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode,
};

use crate::extract::MemoryExtractor;
use crate::record::{
    ExtractionMethod, INLINE_BODY_MAX_BYTES, MemoryBody, MemoryClock, MemoryError,
    MemoryProvenance, MemoryRecord, MemoryScope, RetentionPolicy, preview_of,
};
use crate::store::{MemoryStore, reconcile_memory_embeddings};

/// Bounded per-batch embedding-index drain size.
///
/// Small on purpose: the drain runs inline after every observed batch, so
/// it must stay cheap; anything it does not reach stays pending for the
/// next batch (or an application-driven backfill).
const EMBEDDING_DRAIN_LIMIT: usize = 16;

/// Captures candidate memories extracted from observed run events.
pub struct MemoryObserver {
    descriptor: ObserverDescriptor,
    store: Arc<dyn MemoryStore>,
    scope: MemoryScope,
    extractor: Arc<dyn MemoryExtractor>,
    clock: MemoryClock,
    embedder: Option<Arc<dyn TextEmbedder>>,
    diagnostics: Arc<ObserverDiagnostics>,
}

#[derive(Debug, Default)]
struct ObserverDiagnostics {
    attempted: AtomicU64,
    stored: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicU64,
    last_diagnostic: Mutex<Option<&'static str>>,
}

/// Non-authoritative health snapshot for memory capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryObserverDiagnostics {
    /// Candidate writes attempted.
    pub attempted: u64,
    /// Candidate writes stored or idempotently replayed.
    pub stored: u64,
    /// Candidates deliberately dropped by a local bound or policy.
    pub dropped: u64,
    /// Candidate writes that failed.
    pub failed: u64,
    /// Stable code for the most recent drop/failure, if any.
    pub last_diagnostic: Option<&'static str>,
}

impl MemoryObserver {
    /// Construct a memory-capture observer.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Configuration`] when the observer's fixed
    /// component identity fails to parse (not caller-controllable; a defect
    /// if it ever happens).
    pub fn try_new(
        store: Arc<dyn MemoryStore>,
        scope: MemoryScope,
        extractor: Arc<dyn MemoryExtractor>,
        clock: MemoryClock,
    ) -> Result<Self, MemoryError> {
        Self::build(store, scope, extractor, clock, None)
    }

    /// Construct a memory-capture observer that additionally drains the
    /// store's embedding index through `embedder` after each observed
    /// batch's captures.
    ///
    /// The drain is bounded and best-effort: a failing embedder (or store)
    /// changes no capture result, no diagnostics, and no run outcome —
    /// unindexed records simply stay pending for a later drain.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Configuration`] exactly as
    /// [`MemoryObserver::try_new`] does.
    pub fn try_new_with_embedder(
        store: Arc<dyn MemoryStore>,
        scope: MemoryScope,
        extractor: Arc<dyn MemoryExtractor>,
        clock: MemoryClock,
        embedder: Arc<dyn TextEmbedder>,
    ) -> Result<Self, MemoryError> {
        Self::build(store, scope, extractor, clock, Some(embedder))
    }

    fn build(
        store: Arc<dyn MemoryStore>,
        scope: MemoryScope,
        extractor: Arc<dyn MemoryExtractor>,
        clock: MemoryClock,
        embedder: Option<Arc<dyn TextEmbedder>>,
    ) -> Result<Self, MemoryError> {
        scope.validate()?;
        let component = ComponentId::parse("finstack.observer.memory").map_err(|_| {
            MemoryError::Configuration {
                reason: "invalid_component_id",
            }
        })?;
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    component,
                    Some(Version {
                        major: 0,
                        minor: 1,
                        patch: 0,
                    }),
                ),
                // Extraction needs event bodies, so this observer requests
                // the include-body projection. `Full` still withholds
                // credential-sensitivity bodies; `observe` additionally
                // drops credential-sensitivity events before extraction to
                // honor that same rule for its own event handling.
                payload_mode: ObserverPayloadMode::Full,
                metadata: Metadata::empty(),
            },
            store,
            scope,
            extractor,
            clock,
            embedder,
            diagnostics: Arc::new(ObserverDiagnostics::default()),
        })
    }

    /// Snapshot best-effort capture health without affecting run semantics.
    #[must_use]
    pub fn diagnostics(&self) -> MemoryObserverDiagnostics {
        MemoryObserverDiagnostics {
            attempted: self.diagnostics.attempted.load(Ordering::Relaxed),
            stored: self.diagnostics.stored.load(Ordering::Relaxed),
            dropped: self.diagnostics.dropped.load(Ordering::Relaxed),
            failed: self.diagnostics.failed.load(Ordering::Relaxed),
            last_diagnostic: self
                .diagnostics
                .last_diagnostic
                .lock()
                .ok()
                .and_then(|diagnostic| *diagnostic),
        }
    }
}

fn capture_key(
    id: Option<&crate::record::MemoryId>,
    run: &str,
    source: &str,
    index: usize,
) -> Arc<str> {
    Arc::from(id.map_or_else(
        || format!("capture:{run}:{source}:{index}"),
        |id| {
            format!(
                "capture-id:{}",
                finstack_ai_kernel::fixed_domain_digest!(
                    "memory-capture-key",
                    1,
                    format!("{run}\0{source}\0{}", id.as_str()).as_bytes(),
                )
                .to_hex()
            )
        },
    ))
}

impl Observer for MemoryObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let store = Arc::clone(&self.store);
        let scope = self.scope.clone();
        let extractor = Arc::clone(&self.extractor);
        let clock = self.clock.clone();
        let embedder = self.embedder.clone();
        let diagnostics = Arc::clone(&self.diagnostics);
        Box::pin(async move {
            let filtered: Vec<RunEvent> = batch
                .iter()
                .filter(|event| event.sensitivity() != Sensitivity::Credential)
                .cloned()
                .collect();
            let candidates = extractor.extract(&filtered);

            // Candidate indices reset per originating (run, event) pair so
            // the idempotency key `capture:{run_id}:{event_id}:{index}`
            // stays stable across redelivery of the same batch, regardless
            // of how many events an extractor scanned in one call.
            let mut next_index: HashMap<(Arc<str>, Arc<str>), usize> = HashMap::new();

            for candidate in candidates {
                diagnostics.attempted.fetch_add(1, Ordering::Relaxed);
                let run_text = candidate
                    .source_run
                    .clone()
                    .unwrap_or_else(|| Arc::from("unknown"));
                let ref_text = candidate
                    .source_ref
                    .clone()
                    .unwrap_or_else(|| Arc::from("unknown"));

                let slot = next_index
                    .entry((Arc::clone(&run_text), Arc::clone(&ref_text)))
                    .or_insert(0);
                let index = *slot;
                *slot += 1;

                // Oversized candidates are dropped rather than stored inline.
                // A body over the inline ceiling belongs in blob storage, and
                // this observer holds no `ArtifactStore` to stage one into;
                // skipping is the conservative default, and losing an
                // opportunistic capture is never run impact. The index is
                // allocated above first, so the surviving candidates'
                // idempotency keys do not depend on the cap.
                if candidate.body.len() > INLINE_BODY_MAX_BYTES {
                    diagnostics.dropped.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut last) = diagnostics.last_diagnostic.lock() {
                        *last = Some("memory_capture_body_too_large");
                    }
                    continue;
                }

                // Extractor-assigned identities survive delivery boundaries;
                // batch-local indices are only a fallback for custom extractors.
                let idempotency_key =
                    capture_key(candidate.id.as_ref(), &run_text, &ref_text, index);
                let id = if let Some(id) = candidate.id.clone() {
                    id
                } else {
                    let digest = finstack_ai_kernel::fixed_domain_digest!(
                        "memory-observer-capture",
                        1,
                        format!("{run_text}\0{ref_text}\0{index}\0{}", candidate.body).as_bytes(),
                    );
                    // 64 hex chars: always within MEMORY_ID_MAX_BYTES.
                    #[allow(clippy::unwrap_used)]
                    crate::record::MemoryId::parse(&digest.to_hex()).unwrap()
                };

                let now = clock();
                let record = MemoryRecord {
                    id,
                    scope: scope.clone(),
                    keywords: Arc::from(candidate.keywords),
                    preview: preview_of(&candidate.body),
                    body: MemoryBody::Inline(candidate.body),
                    sensitivity: candidate.sensitivity,
                    provenance: MemoryProvenance {
                        source_session: None,
                        source_run: Some(Arc::clone(&run_text)),
                        source_ref: Some(Arc::clone(&ref_text)),
                        extraction: ExtractionMethod::ObserverCapture,
                        confidence: candidate.confidence,
                    },
                    created_at: now,
                    last_confirmed_at: now,
                    supersedes: None,
                    superseded_by: None,
                    retention: RetentionPolicy::KeepUntilDeleted,
                    tombstoned: false,
                };

                // Store failures never propagate: losing an opportunistic
                // capture is not run impact. Genuine misconfiguration is
                // caught in `try_new` instead.
                if store.put(idempotency_key, record).await.is_ok() {
                    diagnostics.stored.fetch_add(1, Ordering::Relaxed);
                } else {
                    diagnostics.failed.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut last) = diagnostics.last_diagnostic.lock() {
                        *last = Some("memory_capture_store_failed");
                    }
                }
            }

            // Post-batch, best-effort embedding-index drain. The index is
            // derived data, so failure isolation here is absolute: a
            // failing embedder or store changes no capture result, no
            // diagnostics, and no run outcome — unindexed records stay
            // pending and re-surface on the next drain.
            if let Some(embedder) = &embedder {
                let _ = reconcile_memory_embeddings(
                    store.as_ref(),
                    embedder.as_ref(),
                    EMBEDDING_DRAIN_LIMIT,
                )
                .await;
            }

            Ok(())
        })
    }
}
