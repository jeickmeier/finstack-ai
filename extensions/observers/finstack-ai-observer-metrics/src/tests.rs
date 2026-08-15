use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    ContentBlock, Digest, EventTag, Id, IdTag, LaneTag, Message, MessageRole, Metadata,
    ProviderIds, QueueDepthWarning, RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RunEventBody,
    RunTag, SessionTag, TextBlock, Timestamp,
};
use finstack_ai_runtime::{
    CompactionEvidence, CompactionResult, Observer, ObserverBackpressure, PromptCacheImpact,
    RunEvent, Sensitivity,
};
use finstack_ai_test::check_observer_conformance;

use super::MetricsObserver;

const CANARY: &str = "CANARY_SECRET_VALUE";

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn event(kind_body: RunEventBody) -> RunEvent {
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
        Sensitivity::Public,
        kind_body,
    )
    .expect("event")
}

fn evidence() -> CompactionEvidence {
    CompactionEvidence {
        strategy_id: Arc::from("finstack.compaction.sliding_window"),
        strategy_version: 1,
        configuration_digest: Digest::raw_json(b"cfg"),
        model_context_profile_digest: Digest::raw_json(b"profile"),
        source_digest: Digest::raw_json(b"source"),
        protected_item_set_digest: Digest::raw_json(b"protected"),
        covered_entry_ids: Arc::from([id::<finstack_ai_kernel::EntryTag>(1)]),
        retained_entry_ids: Arc::from([id::<finstack_ai_kernel::EntryTag>(2)]),
        projection_digest: Digest::raw_json(b"projection"),
        estimated_tokens_before: 80,
        estimated_tokens_after: 20,
        summary_digest: Some(Digest::raw_json(b"summary")),
        cache_impact: PromptCacheImpact::StablePrefixPreserved,
    }
}

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let metrics = MetricsObserver::try_new(8, ObserverBackpressure::DropProgress).expect("metrics");
    check_observer_conformance(&metrics, Arc::from([]))
        .await
        .expect("conformance");
}

#[tokio::test]
async fn encode_prometheus_omits_canary_and_compaction_text() {
    let metrics = MetricsObserver::try_new(8, ObserverBackpressure::DropProgress).expect("metrics");
    metrics
        .observe(Arc::from([event(RunEventBody::QueueDepthWarning(
            QueueDepthWarning { depth: 3, limit: 8 },
        ))]))
        .await
        .expect("observe");
    let message = Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(9),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new(CANARY).expect("text"),
        )],
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message");
    let result = CompactionResult {
        replacement_messages: Arc::from([message]),
        derived_summaries: Arc::from([]),
        evidence: evidence(),
        checkpoint: None,
    };
    metrics.record_compaction_result(&result);
    metrics.record_compaction_failure();
    metrics.record_compaction_summary_latency(0.01);
    let text = metrics.encode_prometheus();
    assert!(text.contains("finstack_compaction_tokens"));
    assert!(text.contains("finstack.compaction.sliding_window"));
    assert!(!text.contains(CANARY));
    assert!(!text.contains("replacement"));
}

#[tokio::test]
async fn drop_progress_overflow_is_diagnosed() {
    let metrics = MetricsObserver::try_new(1, ObserverBackpressure::DropProgress).expect("metrics");
    metrics
        .observe(Arc::from([
            event(RunEventBody::QueueDepthWarning(QueueDepthWarning {
                depth: 1,
                limit: 8,
            })),
            event(RunEventBody::QueueDepthWarning(QueueDepthWarning {
                depth: 2,
                limit: 8,
            })),
        ]))
        .await
        .expect("observe");
    assert!(metrics.dropped() >= 1);
    assert_eq!(
        metrics.last_diagnostic().expect("diagnostic").code,
        "observer_queue_overflow"
    );
    assert!(
        metrics
            .encode_prometheus()
            .contains("finstack_observer_dropped_total")
    );
}

#[tokio::test]
async fn block_bounded_timeout_does_not_hang() {
    let metrics = MetricsObserver::try_new(
        1,
        ObserverBackpressure::BlockBounded {
            timeout: Duration::from_millis(1),
        },
    )
    .expect("metrics");
    metrics
        .observe(Arc::from([
            event(RunEventBody::QueueDepthWarning(QueueDepthWarning {
                depth: 1,
                limit: 8,
            })),
            event(RunEventBody::QueueDepthWarning(QueueDepthWarning {
                depth: 2,
                limit: 8,
            })),
        ]))
        .await
        .expect("observe");
    assert!(metrics.dropped() >= 1);
}
