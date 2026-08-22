use super::*;
use finstack_ai_kernel::Version;

fn component(id: &str) -> ComponentRef {
    ComponentRef::new(
        ComponentId::parse(id).expect("component id"),
        Some(Version {
            major: 1,
            minor: 0,
            patch: 0,
        }),
    )
}

#[test]
fn builder_and_json_have_identical_fingerprints() {
    let instruction = InstructionSpec::try_new("Inspect before editing.").expect("instruction");
    let built = AgentBuilder::new(
        AgentId::parse("research-agent").expect("agent id"),
        component("finstack.model.test"),
        component("finstack.store.memory"),
    )
    .instructions(Arc::from([instruction]))
    .build()
    .expect("builder spec");
    let json = serde_json::to_vec(&built).expect("json");
    let decoded = AgentSpec::from_json(&json).expect("JSON spec");
    assert_eq!(built, decoded);
    assert_eq!(built.fingerprint(), decoded.fingerprint());
}

#[test]
fn minimal_agent_has_no_capability_object() {
    let spec = AgentBuilder::new(
        AgentId::parse("minimal-agent").expect("agent id"),
        component("finstack.model.test"),
        component("finstack.store.memory"),
    )
    .build()
    .expect("minimal spec");
    assert!(spec.capabilities.is_empty());
    let encoded = serde_json::to_value(spec).expect("JSON");
    assert_eq!(encoded["capabilities"], serde_json::json!([]));
}

#[test]
fn declarative_spec_may_omit_store_until_executable_resolution() {
    let spec = AgentBuilder::new(
        AgentId::parse("declarative-agent").expect("agent id"),
        component("finstack.model.test"),
        component("finstack.store.memory"),
    )
    .build()
    .expect("spec");
    let mut json = serde_json::to_value(spec).expect("JSON");
    json.as_object_mut().expect("object").remove("store");
    let decoded = AgentSpec::from_json(&serde_json::to_vec(&json).expect("JSON bytes"))
        .expect("declarative spec");
    assert!(decoded.store.is_none());
}

#[test]
fn strict_json_rejects_unknown_version_and_duplicate_capability() {
    let base = AgentBuilder::new(
        AgentId::parse("strict-agent").expect("agent id"),
        component("finstack.model.test"),
        component("finstack.store.memory"),
    )
    .build()
    .expect("spec");
    let mut unknown = serde_json::to_value(&base).expect("JSON");
    unknown["ambient_authority"] = serde_json::json!(true);
    assert!(AgentSpec::from_json(&serde_json::to_vec(&unknown).expect("JSON")).is_err());

    let mut version = serde_json::to_value(&base).expect("JSON");
    version["schema_version"] = serde_json::json!(2);
    assert!(AgentSpec::from_json(&serde_json::to_vec(&version).expect("JSON")).is_err());

    let capability = CapabilityRef {
        id: CapabilityId::parse("finstack.capability.research").expect("capability id"),
        bundle: None,
    };
    let error = AgentBuilder::new(
        AgentId::parse("duplicate-agent").expect("agent id"),
        component("finstack.model.test"),
        component("finstack.store.memory"),
    )
    .capabilities(Arc::from([capability.clone(), capability]))
    .build()
    .expect_err("duplicate must fail");
    assert!(matches!(error, AgentSpecError::DuplicateCapability { .. }));
}

#[test]
fn capability_serialization_is_declarative_only() {
    let capability = CapabilitySpec {
        id: CapabilityId::parse("finstack.capability.research").expect("capability id"),
        description: Arc::from("Research tools"),
        instructions: Arc::from([
            InstructionSpec::try_new("Cite primary sources.").expect("instruction")
        ]),
        toolsets: Arc::from([component("finstack.tools.search")]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation: CapabilityActivation::Application,
    };
    capability.validate().expect("capability");
    let encoded = serde_json::to_string(&capability).expect("JSON");
    assert!(!encoded.contains("function"));
    assert!(!encoded.contains("handle"));
    assert!(!encoded.contains("credential"));
    let decoded: CapabilitySpec = serde_json::from_str(&encoded).expect("strict decode");
    assert_eq!(decoded, capability);
}

#[test]
fn run_policy_approval_grant_defaults_to_per_call_and_omits_from_json() {
    let policy = RunPolicy::default();
    assert_eq!(policy.approval_grant, ApprovalGrantMode::PerCall);
    let encoded = serde_json::to_value(&policy).expect("json");
    assert!(encoded.get("approval_grant").is_none());
    let decoded: RunPolicy = serde_json::from_value(serde_json::json!({})).expect("default");
    assert_eq!(decoded.approval_grant, ApprovalGrantMode::PerCall);
    let batched = RunPolicy {
        approval_grant: ApprovalGrantMode::InformedBatch,
        ..RunPolicy::default()
    };
    let encoded = serde_json::to_value(&batched).expect("json");
    assert_eq!(encoded["approval_grant"], "informed_batch");
}

#[test]
fn compatibility_vectors_enforce_strict_spec_ingress() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/compatibility/agent-spec/v1/agent-spec");
    let valid = std::fs::read(root.join("valid--minimal.json")).expect("valid fixture");
    let spec = AgentSpec::from_json(&valid).expect("valid fixture must decode");
    assert_eq!(
        spec.id,
        AgentId::parse("finstack.agent.minimal").expect("fixture id")
    );

    for name in [
        "invalid--unknown-field.json",
        "invalid--unsupported-version.json",
    ] {
        let invalid = std::fs::read(root.join(name)).expect("invalid fixture");
        assert!(
            AgentSpec::from_json(&invalid).is_err(),
            "{name} must fail strict decoding"
        );
    }
}
