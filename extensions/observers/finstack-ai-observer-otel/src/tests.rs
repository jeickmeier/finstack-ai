use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    EventTag, Id, IdTag, LaneTag, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, RunEventBody, RunTag, SessionTag, Timestamp,
};
use finstack_ai_runtime::{Observer, ObserverBackpressure, RunEvent, Sensitivity};
use finstack_ai_test::check_observer_conformance;

use super::{OtelObserver, otlp_feature_enabled};

const CANARY: &str = "CANARY_SECRET_VALUE";

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn event(sensitivity: Sensitivity) -> RunEvent {
    RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(10),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        None,
        None,
        None,
        None,
        None,
        1,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        sensitivity,
        RunEventBody::QueueDepthWarning(QueueDepthWarning { depth: 1, limit: 8 }),
    )
    .expect("event")
}

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let observer = OtelObserver::try_redacted(8, ObserverBackpressure::DropProgress).expect("otel");
    check_observer_conformance(&observer, Arc::from([]))
        .await
        .expect("conformance");
    assert!(!otlp_feature_enabled());
}

#[tokio::test]
async fn redacted_spans_keep_ids_and_omit_secret_bodies() {
    let observer = OtelObserver::try_redacted(8, ObserverBackpressure::DropProgress).expect("otel");
    observer
        .observe(Arc::from([
            event(Sensitivity::Public),
            event(Sensitivity::Secret),
            event(Sensitivity::Credential),
        ]))
        .await
        .expect("observe");
    let spans = observer.snapshot().expect("snapshot");
    assert_eq!(spans.len(), 3);
    assert!(spans.iter().all(|span| span.name == "finstack.run"));
    let dumped = format!("{spans:?}");
    assert!(dumped.contains("finstack.session_id"));
    assert!(!dumped.contains(CANARY));
    let secret = &spans[1];
    assert!(
        secret
            .attributes
            .iter()
            .all(|(key, _)| key != "finstack.body")
    );
}

#[tokio::test]
async fn drop_progress_overflow_is_diagnosed() {
    let observer = OtelObserver::try_redacted(1, ObserverBackpressure::DropProgress).expect("otel");
    observer
        .observe(Arc::from([
            event(Sensitivity::Public),
            event(Sensitivity::Public),
        ]))
        .await
        .expect("observe");
    assert!(observer.dropped() >= 1);
    assert_eq!(
        observer.last_diagnostic().expect("diagnostic").code,
        "observer_queue_overflow"
    );
}

#[tokio::test]
async fn block_bounded_timeout_does_not_hang() {
    let observer = OtelObserver::try_redacted(
        1,
        ObserverBackpressure::BlockBounded {
            timeout: Duration::from_millis(1),
        },
    )
    .expect("otel");
    observer
        .observe(Arc::from([
            event(Sensitivity::Public),
            event(Sensitivity::Public),
        ]))
        .await
        .expect("observe");
    assert!(observer.dropped() >= 1);
}
