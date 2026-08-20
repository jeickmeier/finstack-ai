use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, CostAmount, Digest, EffectCompleted, EffectInput, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectRequested, EffectTag, EventTag, Id, IdTag,
    InvocationRecovery, LaneTag, ModelRequestTag, ProviderIds, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, RawJson, RetrySafety, RunEvent, RunEventBody, RunTag, Sensitivity,
    SessionTag, Timestamp, TurnTag, Usage, Version,
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
    assert!(billing.dropped() >= 1);
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
