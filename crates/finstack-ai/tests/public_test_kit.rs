//! Public SDK plus public test-kit golden driver proof.

use finstack_ai::AgentSpec;
use finstack_ai_test::{
    ConformanceRunner, GoldenScenarioId, ReducerRustAdapter, compatibility_fixture,
    load_pr023_golden_scenarios,
};

#[test]
fn sdk_agent_spec_and_reducer_golden_run_without_private_internals() {
    let spec_path = compatibility_fixture("agent-spec/v1/agent-spec/valid--minimal.json");
    let spec = AgentSpec::from_json(&std::fs::read(spec_path).expect("agent spec"))
        .expect("public SDK spec ingress");
    assert!(!spec.fingerprint().expect("fingerprint").to_hex().is_empty());

    let suite = load_pr023_golden_scenarios().expect("public scenario suite");
    assert!(
        suite
            .scenario(GoldenScenarioId::BeforeFinalizeContinuation)
            .is_some()
    );
    let report = ConformanceRunner::new()
        .run_path(
            compatibility_fixture(
                "golden-trace/v1/trace/valid--pr009-before-finalize-continue.json",
            ),
            &ReducerRustAdapter,
        )
        .expect("public golden runner");
    assert!(report.passed, "{}", report.message);
}
