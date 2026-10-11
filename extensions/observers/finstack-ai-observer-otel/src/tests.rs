use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    EventTag, Id, IdTag, LaneTag, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, RunEventBody, RunTag, SessionTag, Timestamp,
};
use finstack_ai_kernel::{RunEvent, Sensitivity};
use finstack_ai_runtime::ports::observer::{Observer, ObserverPayloadMode};
use finstack_ai_test::check_observer_conformance;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SimpleSpanProcessor, SpanData, SpanExporter};

use super::OtelObserver;

/// One exported span: name plus attribute keys.
type ExportedSpan = (String, Vec<String>);

/// Minimal in-memory exporter: records each exported span's name and
/// attribute keys. Kept local so the crate does not need the SDK's
/// `testing` feature (which pulls in metrics, logs, and tokio).
#[derive(Debug, Clone, Default)]
struct RecordingExporter {
    spans: Arc<Mutex<Vec<ExportedSpan>>>,
}

impl SpanExporter for RecordingExporter {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let spans = Arc::clone(&self.spans);
        async move {
            if let Ok(mut recorded) = spans.lock() {
                for span in batch {
                    let keys = span
                        .attributes
                        .iter()
                        .map(|attribute| attribute.key.to_string())
                        .collect();
                    recorded.push((span.name.into_owned(), keys));
                }
            }
            Ok(())
        }
    }
}

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
    let observer = OtelObserver::try_redacted(8).expect("otel");
    check_observer_conformance(&observer, Arc::from([]))
        .await
        .expect("conformance");
}

#[tokio::test]
async fn redacted_spans_keep_ids_and_omit_secret_bodies() {
    let observer = OtelObserver::try_redacted(8).expect("otel");
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
    let observer = OtelObserver::try_redacted(1).expect("otel");
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
        "otel_capture_saturated"
    );
}

#[tokio::test]
async fn captured_spans_stay_within_the_queue_capacity() {
    let observer = OtelObserver::try_redacted(2).expect("otel");
    for _ in 0..8 {
        observer
            .observe(Arc::from([event(Sensitivity::Public)]))
            .await
            .expect("observe");
    }
    let spans = observer.snapshot().expect("snapshot");
    assert!(spans.len() <= 2);
}

#[tokio::test]
async fn operator_provider_receives_spans() {
    let recorder = RecordingExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(recorder.clone()))
        .build();
    let observer = OtelObserver::try_new_with_provider(ObserverPayloadMode::Redacted, 8, provider)
        .expect("otel");
    observer
        .observe(Arc::from([
            event(Sensitivity::Public),
            event(Sensitivity::Secret),
        ]))
        .await
        .expect("observe");

    // The simple processor exports on span end; no explicit flush is needed.
    let exported = recorder.spans.lock().expect("exported").clone();
    assert_eq!(exported.len(), 2);
    assert!(exported.iter().all(|(name, _)| name == "finstack.run"));
    assert!(
        exported
            .iter()
            .all(|(_, keys)| keys.iter().any(|key| key == "finstack.session_id"))
    );
    // Redacted mode exports the secret event's identifiers only.
    let (_, secret_keys) = &exported[1];
    assert!(secret_keys.iter().all(|key| key != "finstack.body"));
    assert!(!format!("{exported:?}").contains(CANARY));
    assert_eq!(observer.snapshot().expect("snapshot").len(), 2);

    observer.force_flush();
    assert_eq!(recorder.spans.lock().expect("exported").len(), 2);
}

#[test]
fn try_new_with_provider_rejects_invalid_capacity() {
    let provider = SdkTracerProvider::builder().build();
    assert!(
        OtelObserver::try_new_with_provider(ObserverPayloadMode::Redacted, 0, provider).is_err()
    );
}
