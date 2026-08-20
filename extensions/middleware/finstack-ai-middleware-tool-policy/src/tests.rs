use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::ToolId;

use crate::{JailbreakAction, ToolPolicyConfig, ToolPolicyError};

fn tid(s: &str) -> ToolId {
    ToolId::parse(s).expect("tool id")
}

#[test]
fn role_allowlist_rejects_oversized_role_set() {
    let roles: BTreeMap<Arc<str>, BTreeSet<ToolId>> = (0..129)
        .map(|i| (Arc::from(format!("role-{i}").as_str()), BTreeSet::new()))
        .collect();
    let err = ToolPolicyConfig::try_new()
        .expect("empty config")
        .with_role_allowlist(roles, BTreeSet::new())
        .expect_err("129 roles must be rejected");
    assert!(matches!(
        err,
        ToolPolicyError::Configuration {
            reason: "too_many_roles"
        }
    ));
}

#[test]
fn jailbreak_rejects_empty_and_oversized_patterns() {
    let base = || ToolPolicyConfig::try_new().expect("empty config");
    assert!(
        base()
            .with_jailbreak_triggers(vec![Arc::from("")], JailbreakAction::Fail)
            .is_err()
    );
    assert!(
        base()
            .with_jailbreak_triggers(
                vec![Arc::from("x".repeat(257).as_str())],
                JailbreakAction::Fail
            )
            .is_err()
    );
    assert!(
        base()
            .with_jailbreak_triggers(
                vec![Arc::from("ignore previous instructions")],
                JailbreakAction::Fail
            )
            .is_ok()
    );
}

#[test]
fn child_depth_gate_rejects_depth_over_kernel_cap() {
    let err = ToolPolicyConfig::try_new()
        .expect("empty config")
        .with_child_depth_gate(0, 17, BTreeSet::from([tid("finstack.tools.subagent")]))
        .expect_err("max_depth 17 exceeds kernel cap 16");
    assert!(matches!(
        err,
        ToolPolicyError::Configuration {
            reason: "depth_exceeds_kernel_cap"
        }
    ));
}

#[test]
fn config_serialization_is_deterministic_for_digest() {
    let build = || {
        ToolPolicyConfig::try_new()
            .expect("empty config")
            .with_write_budget(3)
            .expect("budget")
    };
    let a = serde_json::to_vec(&build()).expect("serialize a");
    let b = serde_json::to_vec(&build()).expect("serialize b");
    assert_eq!(a, b);
}
