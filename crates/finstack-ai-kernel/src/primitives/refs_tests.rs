use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::primitives::{ComponentId, LimitKey, RecordId};

#[test]
fn principal_and_usage_roundtrip() {
    let principal = PrincipalRef::try_new("https://issuer.example", "user-1", Some("tenant-a"))
        .expect("principal");
    let json = serde_json::to_string(&principal).expect("ser");
    let back: PrincipalRef = serde_json::from_str(&json).expect("de");
    assert_eq!(back, principal);

    let usage = Usage {
        input_tokens: Some(10),
        output_tokens: Some(2),
        total_tokens: Some(12),
        cost: Some(CostAmount::try_new("USD", 1500, "price-v1").expect("cost")),
        extension_counters: BTreeMap::new(),
    };
    let json = serde_json::to_string(&usage).expect("ser");
    assert!(json.contains("\"1500\""));
    let back: Usage = serde_json::from_str(&json).expect("de");
    assert_eq!(back, usage);

    let component = ComponentRef::new(
        ComponentId::parse("finstack.model.example").expect("id"),
        Some(Version {
            major: 1,
            minor: 0,
            patch: 0,
        }),
    );
    assert_eq!(component.version().unwrap().major, 1);
}

#[test]
fn cost_micros_boundary_values_round_trip_as_canonical_decimal_strings() {
    for micros in [0, (1_u64 << 53) - 1, 1_u64 << 53, u64::MAX] {
        let amount = CostAmount::try_new("USD", micros, "price-v1").expect("cost");
        let encoded = serde_json::to_string(&amount).expect("serialize cost");
        assert!(encoded.contains(&format!(r#""micros":"{micros}""#)));
        let decoded: CostAmount = serde_json::from_str(&encoded).expect("deserialize cost");
        assert_eq!(decoded, amount);
    }
    for invalid in [
        r#"{"unit":"USD","micros":0,"pricing_policy_version":"price-v1"}"#,
        r#"{"unit":"USD","micros":"00","pricing_policy_version":"price-v1"}"#,
    ] {
        assert!(serde_json::from_str::<CostAmount>(invalid).is_err());
    }
}

#[test]
fn role_and_queue_assignee_hints_round_trip() {
    for hint in [
        AssigneeHint::Role(Arc::<str>::from("reviewer")),
        AssigneeHint::Queue(Arc::<str>::from("ops")),
    ] {
        let json = serde_json::to_string(&hint).expect("serialize hint");
        let round: AssigneeHint = serde_json::from_str(&json).expect("deserialize hint");
        assert_eq!(round, hint);
    }
}

#[test]
fn allocated_id_arrays_and_usage_maps_enforce_semantic_ceilings() {
    let record_id = RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("record");
    let ids = AllocatedIds {
        record_ids: vec![record_id; crate::content::CONTENT_MAX_ITEMS + 1],
        ..AllocatedIds::default()
    };
    let json = serde_json::to_string(&ids).expect("serialize ids");
    assert!(
        serde_json::from_str::<AllocatedIds>(&json).is_err(),
        "oversized allocated-id array must fail"
    );

    let usage = Usage {
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        cost: None,
        extension_counters: (0..33)
            .map(|index| {
                (
                    LimitKey::parse(format!("app.counter-{index}")).expect("key"),
                    1,
                )
            })
            .collect(),
    };
    let json = serde_json::to_string(&usage).expect("serialize usage");
    assert!(
        serde_json::from_str::<Usage>(&json).is_err(),
        "more than 32 extension counters must fail"
    );
}
