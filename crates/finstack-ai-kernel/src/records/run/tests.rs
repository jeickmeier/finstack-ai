use super::*;
use crate::primitives::ComponentId;
use crate::primitives::ComponentRef;
use crate::primitives::Digest;
use crate::primitives::EffectId;
use crate::primitives::PrincipalRef;
use crate::primitives::RunId;
use crate::primitives::Sensitivity;
use crate::primitives::Timestamp;
use crate::records::policy::RunLimits;

fn sample_security() -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("iss", "sub", Some("tenant-a")).expect("p"),
        "oidc",
        "high",
        "policy-1",
        "decision-1",
        None,
    )
    .expect("security")
}

#[test]
fn root_and_child_lineage() {
    let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run");
    let root_rel = RunRelation::root(run).expect("root");
    let root = RunAccepted::try_new(
        run,
        root_rel,
        sample_security(),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"lock":1}"#),
        None,
    )
    .expect("root accepted");

    let child_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("child");
    let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("effect");
    let child_rel = RunRelation::try_new(
        run,
        Some(run),
        Some(effect),
        RunRelationKind::ChildAgent,
        1,
        None,
        None::<&str>,
    )
    .expect("child rel");
    let child = RunAccepted::try_new(
        child_run,
        child_rel,
        sample_security(),
        None,
        RunLimits::empty(),
        root.propagation(),
        Digest::raw_json(br#"{"lock":1}"#),
        Some(&root),
    )
    .expect("child");
    assert_eq!(child.relation().depth(), 1);
    let json = serde_json::to_string(&child).expect("serialize child");
    let decoded: RunAccepted = serde_json::from_str(&json).expect("decode child");
    assert!(!decoded.lineage_is_validated());
    let revalidated = decoded
        .validate_against_parent(&root)
        .expect("revalidate child");
    assert!(revalidated.lineage_is_validated());

    assert!(
        RunRelation::try_new(
            run,
            None,
            None,
            RunRelationKind::Root,
            17,
            None,
            None::<&str>,
        )
        .is_err()
    );

    let late = Timestamp::from_unix_ms(2).expect("ts");
    let early = Timestamp::from_unix_ms(1).expect("ts");
    let parent_with_deadline = RunAccepted::try_new(
        run,
        RunRelation::root(run).expect("root"),
        sample_security(),
        Some(early),
        RunLimits::empty(),
        root.propagation(),
        Digest::raw_json(br#"{"lock":1}"#),
        None,
    )
    .expect("parent deadline");
    let err = RunAccepted::try_new(
        child_run,
        child.relation().clone(),
        sample_security(),
        Some(late),
        RunLimits::empty(),
        root.propagation(),
        Digest::raw_json(br#"{"lock":1}"#),
        Some(&parent_with_deadline),
    )
    .expect_err("widened deadline");
    assert_eq!(err.code(), "run_not_attenuated");
}

#[test]
fn non_root_run_requires_parent_context() {
    let root_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("root");
    let child_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("child");
    let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("effect");
    let relation = RunRelation::try_new(
        root_run,
        Some(root_run),
        Some(effect),
        RunRelationKind::ChildAgent,
        1,
        None,
        None::<&str>,
    )
    .expect("relation");
    let err = RunAccepted::try_new(
        child_run,
        relation,
        sample_security(),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br"{}"),
        None,
    )
    .expect_err("parent context required");
    assert_eq!(err.code(), "invalid_run_relation");
}

#[test]
fn inherited_principal_cannot_change_via_delegated_from_marker() {
    let parent_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("parent");
    let child_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("child");
    let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("effect");
    let parent = RunAccepted::try_new(
        parent_run,
        RunRelation::root(parent_run).expect("root"),
        sample_security(),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br"{}"),
        None,
    )
    .expect("parent");
    let child_security = RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("iss", "other", Some("tenant-a")).expect("child principal"),
        "oidc",
        "high",
        "policy-1",
        "decision-2",
        Some(parent.security().principal().clone()),
    )
    .expect("child security");
    let relation = RunRelation::try_new(
        parent_run,
        Some(parent_run),
        Some(effect),
        RunRelationKind::ChildAgent,
        1,
        None,
        None::<&str>,
    )
    .expect("relation");
    let err = RunAccepted::try_new(
        child_run,
        relation,
        child_security,
        None,
        RunLimits::empty(),
        parent.propagation(),
        Digest::raw_json(br"{}"),
        Some(&parent),
    )
    .expect_err("inherit cannot change principal");
    assert_eq!(err.code(), "run_not_attenuated");
}

#[test]
fn security_context_rejects_mismatched_principal_tenant() {
    let err = RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("iss", "sub", Some("tenant-b")).expect("principal"),
        "oidc",
        "high",
        "policy-1",
        "decision-1",
        None,
    )
    .expect_err("tenant mismatch");
    assert_eq!(err.code(), "invalid_run_security_context");
}

#[test]
fn child_run_cannot_reuse_parent_run_identity() {
    let parent_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("parent");
    let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("effect");
    let parent = RunAccepted::try_new(
        parent_run,
        RunRelation::root(parent_run).expect("root"),
        sample_security(),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br"{}"),
        None,
    )
    .expect("parent");
    let relation = RunRelation::try_new(
        parent_run,
        Some(parent_run),
        Some(effect),
        RunRelationKind::ChildAgent,
        1,
        None,
        None::<&str>,
    )
    .expect("relation");
    let err = RunAccepted::try_new(
        parent_run,
        relation.clone(),
        sample_security(),
        None,
        RunLimits::empty(),
        parent.propagation(),
        Digest::raw_json(br"{}"),
        Some(&parent),
    )
    .expect_err("child must not reuse parent run_id");
    assert_eq!(err.code(), "child_run_identity_reuse");

    let child_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("child");
    let decoded = RunAccepted::try_new(
        child_run,
        relation,
        sample_security(),
        None,
        RunLimits::empty(),
        parent.propagation(),
        Digest::raw_json(br"{}"),
        Some(&parent),
    )
    .expect("distinct child");
    let json = serde_json::to_string(&decoded).expect("serialize");
    let mut reused: RunAccepted = serde_json::from_str(&json).expect("decode");
    reused = {
        let mut value = serde_json::to_value(&reused).expect("child json");
        value["run_id"] = serde_json::json!(parent_run.to_string());
        serde_json::from_value(value).expect("structurally valid reuse")
    };
    let err = reused
        .validate_against_parent(&parent)
        .expect_err("revalidation must reject identity reuse");
    assert_eq!(err.code(), "child_run_identity_reuse");
}

fn sample_compaction_auth(sensitivity: Sensitivity) -> CompactionAuthorization {
    CompactionAuthorization::new(
        ComponentRef::new(
            ComponentId::parse("fixture.child-model").expect("component"),
            None,
        ),
        sensitivity,
        Digest::raw_json(b"residency"),
    )
}

#[test]
fn historical_security_deserializes_missing_compaction_authorization_as_deny() {
    let json = serde_json::json!({
        "tenant_scope": "tenant-a",
        "principal": {
            "issuer": "iss",
            "subject": "sub",
            "tenant_scope": "tenant-a"
        },
        "authentication_method": "oidc",
        "assurance_level": "high",
        "authorization_policy_version": "policy-1",
        "authorization_decision_id": "decision-1"
    });
    let security: RunSecurityContext = serde_json::from_value(json).expect("decode");
    assert!(security.compaction_authorization().is_none());
}

#[test]
fn child_may_omit_or_attenuate_compaction_authorization_but_not_broaden() {
    let parent = sample_security()
        .with_compaction_authorization(sample_compaction_auth(Sensitivity::Confidential));
    let omitted = sample_security();
    assert!(parent.allows_child_attenuation(&omitted, PrincipalPropagation::Inherit));

    let attenuated = sample_security()
        .with_compaction_authorization(sample_compaction_auth(Sensitivity::Internal));
    assert!(parent.allows_child_attenuation(&attenuated, PrincipalPropagation::Inherit));

    let same = sample_security()
        .with_compaction_authorization(sample_compaction_auth(Sensitivity::Confidential));
    assert!(parent.allows_child_attenuation(&same, PrincipalPropagation::Inherit));

    let raised = sample_security()
        .with_compaction_authorization(sample_compaction_auth(Sensitivity::Secret));
    assert!(!parent.allows_child_attenuation(&raised, PrincipalPropagation::Inherit));

    let invented = sample_security()
        .with_compaction_authorization(sample_compaction_auth(Sensitivity::Internal));
    assert!(!sample_security().allows_child_attenuation(&invented, PrincipalPropagation::Inherit));

    let different_model =
        sample_security().with_compaction_authorization(CompactionAuthorization::new(
            ComponentRef::new(
                ComponentId::parse("fixture.other-model").expect("component"),
                None,
            ),
            Sensitivity::Internal,
            Digest::raw_json(b"residency"),
        ));
    assert!(!parent.allows_child_attenuation(&different_model, PrincipalPropagation::Inherit));
}

#[test]
fn compaction_authorization_requires_exact_model_digest_and_sensitivity_ceiling() {
    let auth = sample_compaction_auth(Sensitivity::Internal);
    let model = ComponentRef::new(
        ComponentId::parse("fixture.child-model").expect("component"),
        None,
    );
    assert!(auth.authorizes(&model, Sensitivity::Public, &Digest::raw_json(b"residency")));
    assert!(auth.authorizes(
        &model,
        Sensitivity::Internal,
        &Digest::raw_json(b"residency")
    ));
    assert!(!auth.authorizes(
        &model,
        Sensitivity::Confidential,
        &Digest::raw_json(b"residency")
    ));
    assert!(!auth.authorizes(&model, Sensitivity::Internal, &Digest::raw_json(b"other")));
    assert!(!auth.authorizes(
        &ComponentRef::new(
            ComponentId::parse("fixture.other-model").expect("component"),
            None,
        ),
        Sensitivity::Internal,
        &Digest::raw_json(b"residency"),
    ));
}
