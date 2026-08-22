use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, EffectTag, EventTag, Id, IdTag, LaneTag, ModelRequestTag, ModelTextDelta,
    RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RunEvent, RunEventBody, RunTag, Sensitivity,
    SessionTag, Timestamp, TurnTag,
};
use finstack_ai_runtime::{Observer, PortFuture};

use crate::extract::{CandidateMemory, MemoryExtractor, RuleBasedExtractor};
use crate::observer::MemoryObserver;
use crate::record::{ExtractionMethod, MemoryScope, system_clock};
use crate::store::{InProcessMemoryStore, MemoryPage, MemoryStore, MemoryStoreError, PutOutcome};

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn text_event(text: &str) -> RunEvent {
    text_event_with(10, 1, text)
}

/// Build a `ModelTextDelta` fixture with explicit event and model-request
/// seeds, so tests can construct multiple deltas that either share or
/// differ on `model_request_id`.
fn text_event_with(event_seed: u64, model_request_seed: u64, text: &str) -> RunEvent {
    RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(event_seed),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(1)),
        Some(id::<ModelRequestTag>(model_request_seed)),
        None,
        Some(id::<EffectTag>(4)),
        None,
        1,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Confidential,
        RunEventBody::ModelTextDelta(ModelTextDelta::try_new(text).expect("delta")),
    )
    .expect("event")
}

#[test]
fn rule_based_extractor_finds_marked_lines() {
    let extractor = RuleBasedExtractor::default();
    let event = text_event("[[remember]] the user prefers dark mode\nother line");
    let candidates = extractor.extract(std::slice::from_ref(&event));
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].body.as_ref(), "the user prefers dark mode");
    assert_eq!(
        candidates[0]
            .keywords
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>(),
        vec!["the", "user", "prefers", "dark", "mode"]
    );
    assert_eq!(candidates[0].confidence, 60);
}

/// A marker line split across two `ModelTextDelta` chunks for the *same*
/// model request must still be recognized: the extractor concatenates
/// same-group delta text in arrival order before line-scanning.
#[test]
fn rule_based_extractor_concatenates_split_marker_lines_within_one_model_request() {
    let extractor = RuleBasedExtractor::default();
    let first = text_event_with(10, 1, "[[remember]] the user pre");
    let second = text_event_with(11, 1, "fers dark mode\n");
    let candidates = extractor.extract(&[first, second]);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].body.as_ref(), "the user prefers dark mode");
    // Idempotency-key stability: source_ref is the FIRST contributing
    // delta's event id, not the id of whichever delta completed the line.
    assert_eq!(
        candidates[0].source_ref.as_deref(),
        Some(id::<EventTag>(10).to_string()).as_deref()
    );
}

/// The same split-line text, but the two deltas belong to different
/// `model_request_id`s: they must NOT be concatenated. Each delta's text is
/// scanned on its own, so only the (incomplete, marker-prefixed) first half
/// yields a candidate.
#[test]
fn rule_based_extractor_does_not_concatenate_across_different_model_requests() {
    let extractor = RuleBasedExtractor::default();
    let first = text_event_with(10, 1, "[[remember]] the user pre");
    let second = text_event_with(11, 2, "fers dark mode\n");
    let candidates = extractor.extract(&[first, second]);
    // The halves belong to different model requests, so they never join. The
    // first is an incomplete line and is carried as residual rather than
    // captured truncated; the second has no marker.
    assert!(candidates.is_empty());

    // Each request completing its own marker line yields its own candidate.
    let extractor = RuleBasedExtractor::default();
    let candidates = extractor.extract(&[
        text_event_with(12, 1, "[[remember]] first fact\n"),
        text_event_with(13, 2, "[[remember]] second fact\n"),
    ]);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].body.as_ref(), "first fact");
    assert_eq!(candidates[1].body.as_ref(), "second fact");
}

/// Terminal event for the same run the text fixtures belong to.
fn run_completed_event() -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(99),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(1)),
        Some(id::<ModelRequestTag>(8)),
        None,
        Some(id::<EffectTag>(4)),
        None,
        1,
        0,
        Timestamp::from_unix_ms(2_000).expect("timestamp"),
        Sensitivity::Internal,
        RunEventBody::RunCompleted {
            result_digest: Digest::raw_json(b"{}"),
        },
    )
    .expect("event")
}

#[tokio::test]
async fn observer_capture_is_idempotent_on_redelivery() {
    let store = Arc::new(InProcessMemoryStore::new());
    let scope = MemoryScope::try_new("t1").expect("scope");
    let extractor: Arc<dyn MemoryExtractor> = Arc::new(RuleBasedExtractor::default());
    let observer = MemoryObserver::try_new(
        store.clone() as Arc<dyn MemoryStore>,
        scope.clone(),
        extractor,
        system_clock(),
    )
    .expect("observer");

    let batch: Arc<[RunEvent]> =
        Arc::from([text_event("[[remember]] the user prefers dark mode\n")]);

    observer.observe(batch.clone()).await.expect("observe 1");
    observer.observe(batch).await.expect("observe 2");

    let listing = store
        .list(
            scope,
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(listing.total, 1);
    assert_eq!(
        listing.records[0].provenance.extraction,
        ExtractionMethod::ObserverCapture
    );
}

/// The observer stores bodies inline and has no `ArtifactStore` to stage a
/// large one into, so an oversized marker line is dropped while a normal one
/// in the same batch is still captured.
#[tokio::test]
async fn observer_skips_candidates_over_the_inline_body_cap() {
    let store = Arc::new(InProcessMemoryStore::new());
    let scope = MemoryScope::try_new("t1").expect("scope");
    let extractor: Arc<dyn MemoryExtractor> = Arc::new(RuleBasedExtractor::default());
    let observer = MemoryObserver::try_new(
        store.clone() as Arc<dyn MemoryStore>,
        scope.clone(),
        extractor,
        system_clock(),
    )
    .expect("observer");

    let oversized = "x".repeat(crate::record::INLINE_BODY_MAX_BYTES + 1);
    let batch: Arc<[RunEvent]> = Arc::from([
        text_event_with(20, 1, &format!("[[remember]] {oversized}\n")),
        text_event_with(21, 2, "[[remember]] a normal sized memory\n"),
    ]);
    observer.observe(batch).await.expect("observe");

    let listing = store
        .list(
            scope,
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(listing.total, 1);
    assert_eq!(listing.records[0].preview.as_ref(), "a normal sized memory");
    assert_eq!(
        observer.diagnostics(),
        crate::observer::MemoryObserverDiagnostics {
            attempted: 2,
            stored: 1,
            dropped: 1,
            failed: 0,
            last_diagnostic: Some("memory_capture_body_too_large"),
        }
    );
}

struct FailingStore;

impl MemoryStore for FailingStore {
    fn put(
        &self,
        _idempotency_key: Arc<str>,
        _record: crate::record::MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>> {
        Box::pin(async move {
            Err(MemoryStoreError::Unavailable {
                message: Arc::from("stub_unavailable"),
            })
        })
    }

    fn get(
        &self,
        _scope: MemoryScope,
        _id: crate::record::MemoryId,
    ) -> PortFuture<Result<Option<crate::record::MemoryRecord>, MemoryStoreError>> {
        Box::pin(async move { Ok(None) })
    }

    fn search(
        &self,
        _scope: MemoryScope,
        _query: crate::store::MemoryQuery,
        _limit: usize,
    ) -> PortFuture<Result<Vec<crate::store::MemoryHit>, MemoryStoreError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn forget(
        &self,
        _idempotency_key: Arc<str>,
        _scope: MemoryScope,
        _id: crate::record::MemoryId,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        Box::pin(async move { Err(MemoryStoreError::NotFound) })
    }

    fn correct(
        &self,
        _idempotency_key: Arc<str>,
        _scope: MemoryScope,
        _old: crate::record::MemoryId,
        _replacement: crate::record::MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        Box::pin(async move { Err(MemoryStoreError::NotFound) })
    }

    fn list(
        &self,
        _scope: MemoryScope,
        _page: MemoryPage,
    ) -> PortFuture<Result<crate::store::MemoryListing, MemoryStoreError>> {
        Box::pin(async move {
            Ok(crate::store::MemoryListing {
                records: Vec::new(),
                total: 0,
            })
        })
    }
}

#[tokio::test]
async fn observer_swallows_store_failures() {
    let store: Arc<dyn MemoryStore> = Arc::new(FailingStore);
    let scope = MemoryScope::try_new("t1").expect("scope");
    let extractor: Arc<dyn MemoryExtractor> = Arc::new(RuleBasedExtractor::default());
    let observer =
        MemoryObserver::try_new(store, scope, extractor, system_clock()).expect("observer");

    let batch: Arc<[RunEvent]> = Arc::from([text_event("[[remember]] this will fail to store\n")]);
    observer
        .observe(batch)
        .await
        .expect("observe swallows put failure");
    assert_eq!(
        observer.diagnostics(),
        crate::observer::MemoryObserverDiagnostics {
            attempted: 1,
            stored: 0,
            dropped: 0,
            failed: 1,
            last_diagnostic: Some("memory_capture_store_failed"),
        }
    );
}

/// Guards against `MemoryExtractor` being read-only over its input: an
/// extractor that returns candidates unrelated to any event must still be
/// handled without panicking (no `source_run`/`source_ref`).
#[tokio::test]
async fn observer_handles_candidates_without_event_correlation() {
    struct FixedExtractor;
    impl MemoryExtractor for FixedExtractor {
        fn extract(&self, _events: &[RunEvent]) -> Vec<CandidateMemory> {
            vec![CandidateMemory {
                id: None,
                keywords: vec![Arc::from("alpha")],
                body: Arc::from("fixed body"),
                sensitivity: Sensitivity::Internal,
                confidence: 60,
                source_run: None,
                source_ref: None,
            }]
        }
    }

    let store = Arc::new(InProcessMemoryStore::new());
    let scope = MemoryScope::try_new("t1").expect("scope");
    let observer = MemoryObserver::try_new(
        store.clone() as Arc<dyn MemoryStore>,
        scope.clone(),
        Arc::new(FixedExtractor),
        system_clock(),
    )
    .expect("observer");

    observer
        .observe(Arc::from([]))
        .await
        .expect("observe with no events");

    let listing = store
        .list(
            scope,
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .expect("list");
    assert_eq!(listing.total, 1);
}

#[test]
fn extractor_carries_a_partial_line_across_batches() {
    let extractor = RuleBasedExtractor::default();
    // Same model request, split across two deliveries.
    let first = extractor.extract(&[text_event_with(20, 7, "[[remember]] the user pre")]);
    assert!(first.is_empty(), "an incomplete line must not be captured");

    let second = extractor.extract(&[text_event_with(21, 7, "fers dark mode\n")]);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].body.as_ref(), "the user prefers dark mode");
    // Attribution stays with the event where the line began, keeping the
    // observer's idempotency key stable.
    assert_eq!(
        second[0].source_ref.as_deref(),
        Some(text_event_with(20, 7, "x").event_id().to_string().as_str())
    );
}

#[test]
fn extractor_flushes_an_unterminated_line_when_the_run_ends() {
    let extractor = RuleBasedExtractor::default();
    let held = extractor.extract(&[text_event_with(30, 8, "[[remember]] a final fact")]);
    assert!(held.is_empty());

    let flushed = extractor.extract(&[run_completed_event()]);
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].body.as_ref(), "a final fact");
}
