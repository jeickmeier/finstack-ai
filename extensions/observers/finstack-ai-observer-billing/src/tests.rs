use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, CostAmount, Digest, EffectCompleted, EffectDeferred,
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested, EffectTag,
    EventTag, ExternalHandleRef, Id, IdTag, InvocationRecovery, LaneTag, ModelRequestTag,
    ProviderIds, RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RawJson, ReconciliationPolicy,
    RetrySafety, RunEvent, RunEventBody, RunTag, Sensitivity, SessionTag, Timestamp, ToolBatchTag,
    ToolCallTag, TurnTag, Usage, Version,
};
use finstack_ai_runtime::{Observer, ObserverBackpressure};
use finstack_ai_test::check_observer_conformance;

use super::BillingObserver;

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn model_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"schema"),
    }
}

fn tool_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ToolResult,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"tool-schema"),
    }
}

/// Durable model-effect event on session 1 / run 3 with the given effect id.
fn model_event(sequence: u64, effect: u64, body: RunEventBody) -> RunEvent {
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
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Confidential,
        body,
    )
    .expect("event")
}

fn usage(input: u64, output: u64, cost: Option<CostAmount>) -> Usage {
    Usage::try_new(Some(input), Some(output), None, cost, BTreeMap::new()).expect("usage")
}

fn completed(effect: u64, usage_value: Option<Usage>) -> RunEventBody {
    RunEventBody::EffectCompleted(
        EffectCompleted::try_new(
            id::<EffectTag>(effect),
            model_contract(),
            RawJson::parse("{}").expect("json"),
            usage_value,
            vec![],
            ProviderIds::empty(),
            None::<&str>,
            None,
        )
        .expect("completed"),
    )
}

fn requested(effect: u64, request_json: &str) -> RunEventBody {
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
                request: RawJson::parse(request_json).expect("json"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("requested"),
    )
}

fn cost(unit: &str, micros: u64, policy: &str) -> CostAmount {
    CostAmount::try_new(unit, micros, policy).expect("cost")
}

fn tool_completed(effect: u64, usage_value: Option<Usage>) -> RunEventBody {
    RunEventBody::EffectCompleted(
        EffectCompleted::try_new(
            id::<EffectTag>(effect),
            tool_contract(),
            RawJson::parse("{}").expect("json"),
            usage_value,
            vec![],
            ProviderIds::empty(),
            None::<&str>,
            None,
        )
        .expect("completed"),
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

/// Durable tool-effect event on session 1 / run 3 with the given effect id.
fn tool_event(sequence: u64, effect: u64, body: RunEventBody) -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(4)),
        None,
        Some(id::<ToolBatchTag>(6)),
        Some(id::<EffectTag>(effect)),
        Some(id::<ToolCallTag>(7)),
        sequence,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Confidential,
        body,
    )
    .expect("event")
}

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let billing =
        BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 64).expect("billing");
    check_observer_conformance(&billing, Arc::from([]))
        .await
        .expect("conformance");
}

#[tokio::test]
async fn invalid_bounds_are_rejected() {
    assert!(BillingObserver::try_new(0, ObserverBackpressure::DropProgress, 64).is_err());
    assert!(BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 0).is_err());
    assert!(BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 1_000_001).is_err());
}

#[tokio::test]
async fn costed_completions_aggregate_by_unit_and_policy() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, completed(7, Some(usage(10, 20, Some(cost("USD", 250, "prices-v1")))))),
            model_event(2, 8, completed(8, Some(usage(1, 2, Some(cost("USD", 750, "prices-v1")))))),
            model_event(3, 9, completed(9, Some(usage(0, 0, Some(cost("USD", 5, "prices-v2")))))),
            model_event(4, 10, completed(10, Some(usage(3, 4, Some(cost("EUR", 9, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.spend.len(), 3);
    let usd_v1 = snapshot
        .spend
        .iter()
        .find(|row| row.unit.as_ref() == "USD" && row.pricing_policy_version.as_ref() == "prices-v1")
        .expect("usd v1 row");
    assert_eq!(usd_v1.micros, 1_000);
    assert_eq!(usd_v1.costed_effects, 2);
    assert_eq!(usd_v1.session_id, id::<SessionTag>(1));
    assert_eq!(usd_v1.run_id, id::<RunTag>(3));
    let usage_row = snapshot.usage.first().expect("usage row");
    assert_eq!(usage_row.input_tokens, 14);
    assert_eq!(usage_row.output_tokens, 26);
    assert_eq!(usage_row.effects, 4);
    assert_eq!(usage_row.uncosted_effects, 0);
}

#[tokio::test]
async fn uncosted_completions_are_counted_never_priced() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, completed(7, Some(usage(10, 20, None)))),
            model_event(2, 8, completed(8, None)),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert!(snapshot.spend.is_empty());
    let row = snapshot.usage.first().expect("usage row");
    assert_eq!(row.input_tokens, 10);
    assert_eq!(row.effects, 2);
    assert_eq!(row.uncosted_effects, 2);
}

#[tokio::test]
async fn model_and_provider_are_attributed_from_the_request() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, requested(7, r#"{"model":"demo-model-1","messages":[]}"#)),
            model_event(2, 7, completed(7, Some(usage(10, 20, Some(cost("USD", 100, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    let row = snapshot.spend.first().expect("spend row");
    assert_eq!(row.model.as_deref(), Some("demo-model-1"));
    let provider = row.provider.clone().expect("provider");
    assert_eq!(provider.id().to_string(), "finstack.model.demo");
    assert_eq!(snapshot.unattributed_effects, 0);
}

#[tokio::test]
async fn untracked_completions_count_as_unattributed() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([model_event(
            1,
            7,
            completed(7, Some(usage(1, 1, Some(cost("USD", 1, "prices-v1"))))),
        )]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.unattributed_effects, 1);
    let row = snapshot.spend.first().expect("spend row");
    assert!(row.model.is_none());
    assert_eq!(row.micros, 1);
}

#[tokio::test]
async fn oversized_or_missing_model_names_fall_back_to_none() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    let long_model = "m".repeat(300);
    let request = format!(r#"{{"model":"{long_model}"}}"#);
    billing
        .observe(Arc::from([
            model_event(1, 7, requested(7, &request)),
            model_event(2, 7, completed(7, Some(usage(1, 1, None)))),
            model_event(3, 8, requested(8, r#"{"messages":[]}"#)),
            model_event(4, 8, completed(8, Some(usage(1, 1, None)))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.usage.len(), 1);
    assert!(snapshot.usage[0].model.is_none());
    assert_eq!(snapshot.usage[0].effects, 2);
    assert_eq!(snapshot.unattributed_effects, 0);
}

fn session_event(session: u64, sequence: u64, effect: u64, body: RunEventBody) -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(sequence),
        id::<SessionTag>(session),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(4)),
        Some(id::<ModelRequestTag>(5)),
        None,
        Some(id::<EffectTag>(effect)),
        None,
        sequence,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Confidential,
        body,
    )
    .expect("event")
}

#[tokio::test]
async fn ledger_saturation_is_counted_and_diagnosed() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 1).expect("billing");
    billing
        .observe(Arc::from([
            session_event(1, 1, 7, completed(7, Some(usage(1, 1, Some(cost("USD", 1, "prices-v1")))))),
            session_event(2, 2, 8, completed(8, Some(usage(1, 1, Some(cost("USD", 1, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.spend.len(), 1);
    assert_eq!(snapshot.overflowed_events, 1);
    assert_eq!(
        billing.last_diagnostic().expect("diagnostic").code,
        "billing_ledger_saturated"
    );
    // The existing key keeps aggregating after saturation.
    billing
        .observe(Arc::from([session_event(
            1, 3, 9,
            completed(9, Some(usage(1, 1, Some(cost("USD", 4, "prices-v1"))))),
        )]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.spend[0].micros, 5);
}

#[tokio::test]
async fn drop_progress_overflow_is_diagnosed() {
    let billing =
        BillingObserver::try_new(1, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, completed(7, None)),
            model_event(2, 8, completed(8, None)),
        ]))
        .await
        .expect("observe");
    // Capacity-1 queue, 2 events in one batch: exactly one drop, not two.
    assert_eq!(billing.dropped(), 1);
    assert_eq!(billing.snapshot().dropped_events, billing.dropped());
}

const CANARY: &str = "CANARY_SECRET_VALUE";

#[tokio::test]
async fn export_jsonl_renders_decimal_strings_and_no_payloads() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    let request = format!(r#"{{"model":"demo-model-1","system":"{CANARY}"}}"#);
    billing
        .observe(Arc::from([
            model_event(1, 7, requested(7, &request)),
            model_event(2, 7, completed(7, Some(usage(10, 20, Some(cost("USD", 1_250_000, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let text = billing.export_jsonl();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    let spend: serde_json::Value = serde_json::from_str(lines[0]).expect("spend json");
    assert_eq!(spend["kind"], "spend");
    assert_eq!(spend["micros"], "1250000");
    assert_eq!(spend["unit"], "USD");
    assert_eq!(spend["pricing_policy_version"], "prices-v1");
    assert_eq!(spend["model"], "demo-model-1");
    assert!(spend["micros"].is_string());
    let usage_line: serde_json::Value = serde_json::from_str(lines[1]).expect("usage json");
    assert_eq!(usage_line["kind"], "usage");
    assert_eq!(usage_line["input_tokens"], "10");
    let summary: serde_json::Value = serde_json::from_str(lines[2]).expect("summary json");
    assert_eq!(summary["kind"], "summary");
    assert_eq!(summary["unattributed_effects"], "0");
    assert!(!text.contains(CANARY));
}

#[tokio::test]
async fn pending_map_saturation_is_diagnosed_and_new_origins_are_unattributed() {
    let billing =
        BillingObserver::try_new(16_384, ObserverBackpressure::DropProgress, 1_000_000)
            .expect("billing");
    // Fill the pending-attribution bound (4096) with distinct model-effect
    // requests that never settle, then request one more to trip saturation.
    let mut events = Vec::with_capacity(4_100);
    let mut sequence = 1_u64;
    for effect in 1..=4_096_u64 {
        events.push(model_event(
            sequence,
            effect,
            requested(effect, r#"{"model":"demo-model-1"}"#),
        ));
        sequence += 1;
    }
    // This 4097th request finds the pending map full and is rejected.
    let over_cap_effect = 5_000_u64;
    events.push(model_event(
        sequence,
        over_cap_effect,
        requested(over_cap_effect, r#"{"model":"demo-model-1"}"#),
    ));
    sequence += 1;
    billing
        .observe(Arc::from(events.into_boxed_slice()))
        .await
        .expect("observe");
    assert_eq!(
        billing.last_diagnostic().expect("diagnostic").code,
        "billing_pending_saturated"
    );
    // The rejected effect's completion lands with no attributed origin.
    billing
        .observe(Arc::from([model_event(
            sequence,
            over_cap_effect,
            completed(over_cap_effect, Some(usage(1, 1, None))),
        )]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.unattributed_effects, 1);
    let row = snapshot
        .usage
        .iter()
        .find(|row| row.model.is_none())
        .expect("unattributed usage row");
    assert_eq!(row.effects, 1);
}

#[tokio::test]
async fn deferred_model_effect_still_settles_as_unattributed() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, requested(7, r#"{"model":"demo-model-1"}"#)),
            model_event(2, 7, deferred(7)),
            model_event(3, 7, completed(7, Some(usage(1, 1, None)))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    let row = snapshot.usage.first().expect("usage row");
    assert!(row.model.is_none());
    assert_eq!(snapshot.unattributed_effects, 1);
}

#[tokio::test]
async fn tool_effect_settlement_aggregates_as_non_model_and_is_never_unattributed() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([tool_event(
            1,
            7,
            tool_completed(7, Some(usage(1, 1, None))),
        )]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.unattributed_effects, 0);
    let row = snapshot.usage.first().expect("usage row");
    assert!(row.model.is_none());
    assert_eq!(row.input_tokens, 1);
    assert_eq!(row.effects, 1);
    assert_eq!(row.uncosted_effects, 1);
}
