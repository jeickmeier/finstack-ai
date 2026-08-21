//! [`Observer`] adapter that captures extracted candidate memories into a
//! [`MemoryStore`].
//!
//! Capture is best-effort: a failing [`MemoryStore::put`] is swallowed
//! (logged nowhere yet, but never surfaced as an [`ObserverError`]) because
//! losing an opportunistic memory capture must never look like run impact.
//! Only misconfiguration at construction time — an invalid component id —
//! fails [`MemoryObserver::try_new`].
//!
//! Captured bodies are always inline, so a candidate whose body exceeds
//! [`INLINE_BODY_MAX_BYTES`] is skipped rather than stored unbounded.

use std::collections::HashMap;
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, RunEvent, Sensitivity, Version};
use finstack_ai_runtime::{
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode, PortFuture,
};

use crate::extract::MemoryExtractor;
use crate::record::{
    ExtractionMethod, INLINE_BODY_MAX_BYTES, MemoryBody, MemoryClock, MemoryError,
    MemoryProvenance, MemoryRecord, MemoryScope, RetentionPolicy, preview_of,
};
use crate::store::MemoryStore;

/// Captures candidate memories extracted from observed run events.
pub struct MemoryObserver {
    descriptor: ObserverDescriptor,
    store: Arc<dyn MemoryStore>,
    scope: MemoryScope,
    extractor: Arc<dyn MemoryExtractor>,
    clock: MemoryClock,
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
        })
    }
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
                    continue;
                }

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

                let idempotency_key: Arc<str> =
                    Arc::from(format!("capture:{run_text}:{ref_text}:{index}"));

                // Store failures never propagate: losing an opportunistic
                // capture is not run impact. Genuine misconfiguration is
                // caught in `try_new` instead.
                let _ = store.put(idempotency_key, record).await;
            }

            Ok(())
        })
    }
}
