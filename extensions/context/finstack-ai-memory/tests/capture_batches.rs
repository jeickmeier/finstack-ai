//! Stable memory capture identities across streaming delivery batches.

use finstack_ai_kernel::*;
use finstack_ai_memory::{
    extract::RuleBasedExtractor,
    observer::MemoryObserver,
    record::MemoryScope,
    store::{InProcessMemoryStore, MemoryPage, MemoryStore},
};
use finstack_ai_runtime::ports::observer::Observer;
use std::sync::Arc;
fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}
fn text_event(seed: u64, text: &str) -> RunEvent {
    RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(seed),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(1)),
        Some(id::<ModelRequestTag>(1)),
        None,
        Some(id::<EffectTag>(4)),
        None,
        1,
        UNIX_EPOCH,
        Sensitivity::Confidential,
        RunEventBody::ModelTextDelta(ModelTextDelta::try_new(text).unwrap()),
    )
    .unwrap()
}
#[tokio::test]
async fn memory_split_line_preserves_both_facts() {
    let store = Arc::new(InProcessMemoryStore::new());
    let scope = MemoryScope::try_new("t1").unwrap();
    let observer = MemoryObserver::try_new(
        store.clone(),
        scope.clone(),
        Arc::new(RuleBasedExtractor::default()),
        Arc::new(|| UNIX_EPOCH),
    )
    .unwrap();
    observer
        .observe(Arc::from([text_event(
            10,
            "[[remember]] first fact\n[[remember]] second",
        )]))
        .await
        .unwrap();
    observer
        .observe(Arc::from([text_event(11, " fact\n")]))
        .await
        .unwrap();
    let result = store
        .list(
            scope.clone(),
            MemoryPage {
                offset: 0,
                limit: 10,
            },
        )
        .await
        .unwrap();
    println!(
        "split memory: count={}, diagnostics={:?}",
        result.total,
        observer.diagnostics()
    );
    assert_eq!(result.total, 2);
    assert_eq!(observer.diagnostics().failed, 0);
    // Literal replay does not create more records or collide with another line.
    observer
        .observe(Arc::from([text_event(
            10,
            "[[remember]] first fact\n[[remember]] second",
        )]))
        .await
        .unwrap();
    observer
        .observe(Arc::from([text_event(11, " fact\n")]))
        .await
        .unwrap();
    assert_eq!(
        store
            .list(
                scope.clone(),
                MemoryPage {
                    offset: 0,
                    limit: 10
                }
            )
            .await
            .unwrap()
            .total,
        2
    );
    assert_eq!(observer.diagnostics().failed, 0);
}
