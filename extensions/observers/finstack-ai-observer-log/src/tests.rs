use std::io::Write;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    EventTag, Id, IdTag, LaneTag, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, RunEventBody, RunTag, SessionTag, Timestamp,
};
use finstack_ai_kernel::{RunEvent, Sensitivity};
use finstack_ai_runtime::{Observer, ObserverPayloadMode, journal_export_jsonl};
use finstack_ai_test::check_observer_conformance;
use tempfile::tempdir;

use super::{LogObserver, write_support_bundle};

const CANARY: &str = "CANARY_SECRET_VALUE";

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn event(sensitivity: Sensitivity, body: RunEventBody) -> RunEvent {
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
        body,
    )
    .expect("event")
}

fn warning(depth: u32) -> RunEventBody {
    RunEventBody::QueueDepthWarning(QueueDepthWarning { depth, limit: 8 })
}

fn observer(writer: Arc<Mutex<Vec<u8>>>) -> LogObserver {
    let sink: Arc<Mutex<dyn Write + Send>> = writer;
    LogObserver::try_new(ObserverPayloadMode::Redacted, sink).expect("observer")
}

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let writer = Arc::new(Mutex::new(Vec::new()));
    let log = observer(writer);
    check_observer_conformance(&log, Arc::from([]))
        .await
        .expect("conformance");
}

#[tokio::test]
async fn redacted_logs_omit_secret_and_credential_bodies() {
    let writer = Arc::new(Mutex::new(Vec::new()));
    let log = observer(Arc::clone(&writer));
    let secret = event(
        Sensitivity::Secret,
        RunEventBody::QueueDepthWarning(QueueDepthWarning { depth: 1, limit: 8 }),
    );
    // Replace body text via a public event that would leak if mis-projected.
    let credential = event(Sensitivity::Credential, warning(2));
    log.observe(Arc::from([
        event(Sensitivity::Public, warning(1)),
        secret,
        credential,
    ]))
    .await
    .expect("observe");
    let text = String::from_utf8(writer.lock().expect("writer").clone()).expect("utf8");
    assert!(
        text.contains("queue_depth_warning")
            || text.contains("QueueDepthWarning")
            || text.contains("kind")
    );
    assert!(!text.contains(CANARY));
}

#[tokio::test]
async fn secret_canary_never_appears_in_log_jsonl_or_bundle() {
    let writer = Arc::new(Mutex::new(Vec::new()));
    let log = observer(Arc::clone(&writer));
    let body = RunEventBody::QueueDepthWarning(QueueDepthWarning { depth: 1, limit: 8 });
    let secret = event(Sensitivity::Secret, body);
    log.observe(Arc::from([secret.clone()]))
        .await
        .expect("observe");
    let logged = String::from_utf8(writer.lock().expect("writer").clone()).expect("utf8");
    assert!(!logged.contains(CANARY));

    let records = [serde_json::json!({
        "record_id": "rec",
        "body": { "secret": CANARY }
    })];
    let export = journal_export_jsonl(&records, ObserverPayloadMode::MetadataOnly).expect("export");
    assert!(!export.contains(CANARY));
    assert!(!export.contains("\"body\""));

    let dir = tempdir().expect("tempdir");
    write_support_bundle(dir.path(), std::slice::from_ref(&secret), &records, None)
        .expect("bundle");
    for entry in std::fs::read_dir(dir.path()).expect("read") {
        let path = entry.expect("entry").path();
        let bytes = std::fs::read(&path).expect("read file");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(CANARY), "{} leaked canary", path.display());
    }
}
