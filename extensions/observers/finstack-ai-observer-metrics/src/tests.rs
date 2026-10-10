use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, EffectCancelled, EffectCompleted,
    EffectDeferred, EffectFailed, EffectInput, EffectKind, EffectOutputContract, EffectOutputKind,
    EffectRequested, EffectTag, ErrorCategory, ErrorDescriptor, EventTag, ExternalHandleRef, Id,
    IdTag, InvocationRecovery, LaneTag, Message, MessageRole, Metadata, ModelRequestTag,
    ProviderIds, QueueDepthWarning, RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RawJson,
    ReconciliationPolicy, RetrySafety, RunEventBody, RunTag, SessionTag, TextBlock, Timestamp,
    TurnTag, Version,
};
use finstack_ai_kernel::{RunEvent, Sensitivity};
use finstack_ai_runtime::ports::middleware::{
    CompactionEvidence, CompactionResult, PromptCacheImpact,
};
use finstack_ai_runtime::ports::observer::Observer;
use finstack_ai_test::check_observer_conformance;

use super::{MAX_EFFECT_STARTS, MetricsObserver};

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

fn model_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"schema"),
    }
}

/// Durable model-effect event on session 1 / run 3 correlated to `effect`.
fn effect_event(sequence: u64, effect: u64, at_ms: i64, body: RunEventBody) -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(4)),
        Some(id::<ModelRequestTag>(5)),
        None,
        Some(id::<EffectTag>(effect)),
        None,
        sequence,
        sequence,
        Timestamp::from_unix_ms(at_ms).expect("timestamp"),
        Sensitivity::Confidential,
        body,
    )
    .expect("event")
}

fn requested(effect: u64) -> RunEventBody {
    RunEventBody::EffectRequested(
        EffectRequested::try_new(
            id::<EffectTag>(effect),
            EffectKind::Model,
            None,
            Some(ComponentInvocation {
                component: ComponentId::parse("finstack.model.demo").expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: Digest::raw_json(b"cfg"),
                recovery: InvocationRecovery::RecomputeSafe,
            }),
            None,
            model_contract(),
            EffectInput::Model {
                request: RawJson::parse("{}").expect("json"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("requested"),
    )
}

fn completed(effect: u64) -> RunEventBody {
    RunEventBody::EffectCompleted(
        EffectCompleted::try_new(
            id::<EffectTag>(effect),
            model_contract(),
            RawJson::parse("{}").expect("json"),
            None,
            vec![],
            ProviderIds::empty(),
            None::<&str>,
            None,
        )
        .expect("completed"),
    )
}

fn failed(effect: u64) -> RunEventBody {
    RunEventBody::EffectFailed(
        EffectFailed::try_new(
            id::<EffectTag>(effect),
            model_contract(),
            ErrorDescriptor::new(
                "provider_failed",
                "provider failed",
                ErrorCategory::Model,
                true,
            )
            .expect("error"),
            None,
            None::<&str>,
        )
        .expect("failed"),
    )
}

fn cancelled(effect: u64) -> RunEventBody {
    RunEventBody::EffectCancelled(
        EffectCancelled::try_new(
            id::<EffectTag>(effect),
            model_contract(),
            None::<&str>,
            None::<&str>,
        )
        .expect("cancelled"),
    )
}

fn deferred(effect: u64) -> RunEventBody {
    RunEventBody::EffectDeferred(EffectDeferred {
        effect_id: id::<EffectTag>(effect),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.model.demo").expect("component"),
            "job-1",
            RawJson::parse("{}").expect("json"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::Poll,
        next_poll_at: None,
        expires_at: None,
        output_contract: model_contract(),
    })
}

fn effect_starts_len(metrics: &MetricsObserver) -> usize {
    metrics.state.lock().expect("state").effect_starts.len()
}

fn effect_start_tracked(metrics: &MetricsObserver, effect: u64) -> bool {
    metrics
        .state
        .lock()
        .expect("state")
        .effect_starts
        .contains_key(&id::<EffectTag>(effect))
}

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let metrics = MetricsObserver::try_new().expect("metrics");
    check_observer_conformance(&metrics, Arc::from([]))
        .await
        .expect("conformance");
}

#[tokio::test]
async fn encode_prometheus_omits_canary_and_compaction_text() {
    let metrics = MetricsObserver::try_new().expect("metrics");
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
async fn effect_starts_is_bounded_and_cleared_on_failure() {
    let metrics = MetricsObserver::try_new().expect("metrics");
    let cap = u64::try_from(MAX_EFFECT_STARTS).expect("cap fits");

    // Every non-completion settlement releases the start entry.
    metrics
        .observe(Arc::from([
            effect_event(1, 1, 1_000, requested(1)),
            effect_event(2, 2, 1_000, requested(2)),
            effect_event(3, 3, 1_000, requested(3)),
            effect_event(4, 4, 1_000, requested(4)),
            effect_event(5, 1, 1_100, failed(1)),
            effect_event(6, 2, 1_100, cancelled(2)),
            effect_event(7, 3, 1_100, deferred(3)),
            effect_event(8, 4, 1_250, completed(4)),
        ]))
        .await
        .expect("observe");
    assert_eq!(effect_starts_len(&metrics), 0);
    let text = metrics.encode_prometheus();
    assert!(text.contains("finstack_recovery_total{class=\"fail\"} 1"));
    assert!(text.contains("finstack_recovery_total{class=\"uncertain\"} 1"));
    // Only the completed effect contributed a latency sample (250 ms).
    assert!(text.contains("finstack_effect_latency_seconds_count 1"));
    assert!(text.contains("finstack_effect_latency_seconds_sum 0.25"));

    // Fill to the bound with ascending timestamps, then overflow: the oldest
    // start is evicted and the map never exceeds the bound.
    let mut batch = Vec::with_capacity(MAX_EFFECT_STARTS + 2);
    for n in 0..cap {
        let effect = 100 + n;
        let at = 2_000 + i64::try_from(n).expect("offset fits");
        batch.push(effect_event(effect, effect, at, requested(effect)));
    }
    metrics.observe(Arc::from(batch)).await.expect("observe");
    assert_eq!(effect_starts_len(&metrics), MAX_EFFECT_STARTS);
    assert!(effect_start_tracked(&metrics, 100));

    let overflow = 100 + cap;
    metrics
        .observe(Arc::from([effect_event(
            overflow,
            overflow,
            9_000,
            requested(overflow),
        )]))
        .await
        .expect("observe");
    assert_eq!(effect_starts_len(&metrics), MAX_EFFECT_STARTS);
    assert!(!effect_start_tracked(&metrics, 100), "oldest start evicted");
    assert!(effect_start_tracked(&metrics, 101));
    assert!(effect_start_tracked(&metrics, overflow));

    // Re-requesting an already tracked effect does not evict anything.
    metrics
        .observe(Arc::from([effect_event(
            overflow + 1,
            overflow,
            9_001,
            requested(overflow),
        )]))
        .await
        .expect("observe");
    assert_eq!(effect_starts_len(&metrics), MAX_EFFECT_STARTS);
    assert!(effect_start_tracked(&metrics, 101));
}
