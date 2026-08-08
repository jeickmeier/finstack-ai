//! Integration tests for golden-trace loading and validation (PR-005).

use finstack_ai_test::{
    DurabilityClass, TraceError, compare_normalized_bytes, compatibility_fixture,
    load_golden_trace, load_noop_trace, normalize_json_value, validate_against_schema,
};
use serde_json::json;

#[test]
fn noop_trace_loads_and_compares_byte_for_byte() {
    let first = load_noop_trace().expect("load noop");
    let second = load_noop_trace().expect("reload noop");
    let first_bytes = first.to_normalized_bytes().expect("normalize first");
    let second_bytes = second.to_normalized_bytes().expect("normalize second");
    assert_eq!(first_bytes, second_bytes);

    let value = serde_json::to_value(&first).expect("serialize");
    compare_normalized_bytes(&value, &value).expect("self-compare");
}

#[test]
fn durable_and_transient_events_are_distinguished() {
    let path = compatibility_fixture("golden-trace/v1/trace/valid--durable-vs-transient.json");
    let trace = load_golden_trace(&path).expect("load durable/transient fixture");
    let durable = trace.durable_events();
    let transient = trace.transient_events();
    assert_eq!(durable.len(), 1);
    assert_eq!(transient.len(), 1);
    assert_eq!(durable[0].durability, DurabilityClass::Durable);
    assert_eq!(transient[0].durability, DurabilityClass::Transient);
    assert_ne!(durable[0].kind, transient[0].kind);
}

#[test]
fn unknown_fields_are_rejected() {
    let text = std::fs::read_to_string(compatibility_fixture(
        "golden-trace/v1/trace/invalid--unknown-field.json",
    ))
    .expect("read invalid fixture");
    let value: serde_json::Value = serde_json::from_str(&text).expect("parse");
    let error = validate_against_schema("golden-trace", 1, "trace", &value)
        .expect_err("unknown field must fail");
    assert!(matches!(error, TraceError::Schema(_)));
}

#[test]
fn oversized_payload_declarations_are_rejected() {
    let text = std::fs::read_to_string(compatibility_fixture(
        "golden-trace/v1/trace/invalid--oversized-envelope-declaration.json",
    ))
    .expect("read oversized fixture");
    let value: serde_json::Value = serde_json::from_str(&text).expect("parse");
    let error = validate_against_schema("golden-trace", 1, "trace", &value)
        .expect_err("oversized declaration must fail");
    assert!(matches!(error, TraceError::Schema(_)));

    let scripted = std::fs::read_to_string(compatibility_fixture(
        "golden-trace/v1/scripted-input/invalid--oversized-string-declaration.json",
    ))
    .expect("read scripted oversized");
    let scripted_value: serde_json::Value = serde_json::from_str(&scripted).expect("parse");
    let scripted_error =
        validate_against_schema("golden-trace", 1, "scripted-input", &scripted_value)
            .expect_err("oversized string declaration must fail");
    assert!(matches!(scripted_error, TraceError::Schema(_)));
}

#[test]
fn normalization_is_order_independent_for_object_keys() {
    let left = json!({"z": 1, "a": [ {"b": 2, "a": 3} ]});
    let right = json!({"a": [ {"a": 3, "b": 2} ], "z": 1});
    assert_eq!(normalize_json_value(&left), normalize_json_value(&right));
}
