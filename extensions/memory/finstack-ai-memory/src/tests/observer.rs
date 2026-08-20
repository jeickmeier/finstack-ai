use std::sync::Arc;

use finstack_ai_kernel::{
    EffectTag, EventTag, Id, IdTag, LaneTag, ModelRequestTag, ModelTextDelta,
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
    RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(10),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(1)),
        Some(id::<ModelRequestTag>(1)),
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

    let batch: Arc<[RunEvent]> = Arc::from([text_event("[[remember]] the user prefers dark mode")]);

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

    let batch: Arc<[RunEvent]> = Arc::from([text_event("[[remember]] this will fail to store")]);
    observer
        .observe(batch)
        .await
        .expect("observe swallows put failure");
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
