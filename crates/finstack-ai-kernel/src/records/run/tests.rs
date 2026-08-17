use super::*;
use crate::primitives::Digest;
use crate::primitives::EffectId;
use crate::primitives::PrincipalRef;
use crate::primitives::RunId;
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
