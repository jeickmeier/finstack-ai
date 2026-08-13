//! Strict public test-kit golden-scenario corpus checks.

use finstack_ai_test::{
    GoldenScenarioId, TraceError, compatibility_fixture, load_golden_scenarios,
    validate_against_schema,
};

#[test]
fn required_golden_scenarios_are_unique_and_contract_labelled() {
    let suite = load_golden_scenarios().expect("scenario suite");
    for id in [
        GoldenScenarioId::ChildRunLineage,
        GoldenScenarioId::DeferredExternalCompletion,
        GoldenScenarioId::TypedInteractionSchema,
        GoldenScenarioId::DuplicateCompletion,
        GoldenScenarioId::BeforeFinalizeContinuation,
        GoldenScenarioId::CompactionProjection,
    ] {
        let scenario = suite.scenario(id).expect("required scenario");
        assert!(!scenario.contracts.is_empty());
        assert!(scenario.input.is_object());
        assert!(scenario.expected.is_object());
    }
}

#[test]
fn unknown_test_kit_fields_fail_strict_schema_validation() {
    let path = compatibility_fixture("golden-trace/v1/test-kit/invalid--unknown-field.json");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("fixture")).expect("json");
    let error =
        validate_against_schema("golden-trace", 1, "test-kit", &value).expect_err("unknown member");
    assert!(matches!(error, TraceError::Schema(_)));
}
