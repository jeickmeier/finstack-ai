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

mod middleware_tests {
    use finstack_ai_kernel::Stage;
    use finstack_ai_runtime::{Middleware, MiddlewareRole, OrderTier};

    use crate::ToolPolicyMiddleware;

    fn any_config() -> crate::ToolPolicyConfig {
        crate::ToolPolicyConfig::try_new()
            .expect("empty config")
            .with_write_budget(3)
            .expect("budget")
    }

    #[test]
    fn descriptor_declares_both_filter_stages_and_request_shaping_tier() {
        let mw = ToolPolicyMiddleware::try_new(any_config()).expect("leaf");
        let d = mw.descriptor();
        assert!(d.stages.contains(Stage::BeforeModel));
        assert!(d.stages.contains(Stage::BeforeToolBatch));
        assert!(!d.stages.contains(Stage::BeforeFinalize));
        assert!(matches!(d.order.tier, OrderTier::RequestShaping));
        assert!(matches!(d.role, MiddlewareRole::Standard));
        assert_eq!(
            d.invocation.component.as_str(),
            "finstack.middleware.tool-policy"
        );
    }

    #[test]
    fn empty_policy_is_rejected() {
        let err = ToolPolicyMiddleware::try_new(
            crate::ToolPolicyConfig::try_new().expect("empty config"),
        )
        .expect_err("a policy with zero rules is a no-op and must be rejected");
        assert!(matches!(
            err,
            crate::ToolPolicyError::Configuration {
                reason: "empty_policy"
            }
        ));
    }

    #[test]
    fn distinct_configs_produce_distinct_digests() {
        let a = ToolPolicyMiddleware::try_new(any_config()).expect("leaf a");
        let b = ToolPolicyMiddleware::try_new(
            crate::ToolPolicyConfig::try_new()
                .expect("cfg")
                .with_write_budget(4)
                .expect("budget"),
        )
        .expect("leaf b");
        assert_ne!(
            a.descriptor().invocation.configuration_digest,
            b.descriptor().invocation.configuration_digest
        );
    }
}

mod eval_tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use finstack_ai_kernel::ToolId;

    use crate::ToolPolicyConfig;
    use crate::eval::narrow_universe;

    fn tid(s: &str) -> ToolId {
        ToolId::parse(s).expect("tool id")
    }

    fn universe() -> BTreeSet<ToolId> {
        BTreeSet::from([tid("t.read"), tid("t.write"), tid("t.spawn")])
    }

    #[test]
    fn role_allowlist_is_deny_by_default_union_of_granted_roles() {
        let cfg = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_role_allowlist(
                BTreeMap::from([
                    (Arc::<str>::from("reader"), BTreeSet::from([tid("t.read")])),
                    (Arc::<str>::from("writer"), BTreeSet::from([tid("t.write")])),
                ]),
                BTreeSet::new(),
            )
            .expect("roles");
        let granted = [Arc::<str>::from("reader")];
        assert_eq!(
            narrow_universe(&cfg, &universe(), &granted),
            BTreeSet::from([tid("t.read")])
        );
        let both = [Arc::<str>::from("reader"), Arc::<str>::from("writer")];
        assert_eq!(
            narrow_universe(&cfg, &universe(), &both),
            BTreeSet::from([tid("t.read"), tid("t.write")])
        );
        // no granted roles, empty default → everything filtered
        assert!(narrow_universe(&cfg, &universe(), &[]).is_empty());
    }

    #[test]
    fn child_depth_gate_hides_restricted_tools_at_threshold() {
        let cfg = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_child_depth_gate(2, 2, BTreeSet::from([tid("t.spawn")]))
            .expect("gate");
        assert_eq!(
            narrow_universe(&cfg, &universe(), &[]),
            BTreeSet::from([tid("t.read"), tid("t.write")])
        );
        let below = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_child_depth_gate(1, 2, BTreeSet::from([tid("t.spawn")]))
            .expect("gate");
        assert_eq!(narrow_universe(&below, &universe(), &[]), universe());
    }

    #[test]
    fn rules_compose_by_intersection() {
        let cfg = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_role_allowlist(
                BTreeMap::from([(
                    Arc::<str>::from("agent"),
                    BTreeSet::from([tid("t.read"), tid("t.spawn")]),
                )]),
                BTreeSet::new(),
            )
            .expect("roles")
            .with_child_depth_gate(3, 2, BTreeSet::from([tid("t.spawn")]))
            .expect("gate");
        let granted = [Arc::<str>::from("agent")];
        assert_eq!(
            narrow_universe(&cfg, &universe(), &granted),
            BTreeSet::from([tid("t.read")])
        );
    }
}
