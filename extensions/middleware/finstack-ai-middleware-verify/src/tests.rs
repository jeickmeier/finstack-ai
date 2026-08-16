use std::sync::Arc;

use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, Digest, EffectId, LaneId, Metadata, Middleware,
    MiddlewareContext, OperationLocator, PrincipalRef, RawJson, RunCallContext, RunId, SessionId,
    StageInput, StageOutcome, validate_stage_outcome,
};
use finstack_ai_test::{MiddlewareConformanceCase, check_middleware_conformance};

use super::*;

fn id<T>(value: u64, parse: impl FnOnce(&str) -> T) -> T {
    parse(&format!("00000000-0000-7000-8000-{value:012x}"))
}

fn ctx() -> MiddlewareContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    MiddlewareContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                id(1, |value| SessionId::parse(value).expect("session")),
                id(2, |value| LaneId::parse(value).expect("lane")),
                id(3, |value| RunId::parse(value).expect("run")),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal,
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: id(4, |value| EffectId::parse(value).expect("effect")),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        chain_digest: Digest::raw_json(b"chain"),
        chain_index: 0,
        compaction_resume: None,
    }
}

fn finalize_input() -> StageInput {
    StageInput::BeforeFinalize {
        candidate: RawJson::parse(br#""candidate""#).expect("candidate"),
    }
}

#[tokio::test]
async fn accept_continues_and_never_writes_a_store() {
    let middleware = VerifyMiddleware::try_accept().expect("verify");
    let outcome = middleware
        .invoke(ctx(), finalize_input())
        .await
        .expect("invoke");
    assert_eq!(outcome, StageOutcome::Continue);
    validate_stage_outcome(&middleware.descriptor(), &finalize_input(), &outcome).expect("allowed");
}

#[tokio::test]
async fn fail_and_interaction_are_allowed_before_finalize() {
    let fail = VerifyMiddleware::try_new(VerifyDecision::Fail).expect("fail");
    let outcome = fail.invoke(ctx(), finalize_input()).await.expect("fail");
    assert!(matches!(outcome, StageOutcome::Fail(_)));
    validate_stage_outcome(&fail.descriptor(), &finalize_input(), &outcome).expect("fail allowed");

    let interact = VerifyMiddleware::try_new(VerifyDecision::RequestInteraction).expect("interact");
    let outcome = interact
        .invoke(ctx(), finalize_input())
        .await
        .expect("interact");
    assert!(matches!(outcome, StageOutcome::RequestInteraction(_)));
    validate_stage_outcome(&interact.descriptor(), &finalize_input(), &outcome)
        .expect("interact allowed");
}

#[tokio::test]
async fn middleware_satisfies_the_published_port_conformance_suite() {
    let middleware = VerifyMiddleware::try_accept().expect("verify");
    let outcome = check_middleware_conformance(
        &middleware,
        MiddlewareConformanceCase {
            context: ctx(),
            input: finalize_input(),
            expected: StageOutcome::Continue,
        },
    )
    .await
    .expect("published middleware conformance suite");
    assert_eq!(outcome, StageOutcome::Continue);
}

#[tokio::test]
async fn replace_is_structurally_unrepresentable_at_finalize() {
    let middleware = VerifyMiddleware::try_accept().expect("verify");
    let error = validate_stage_outcome(
        &middleware.descriptor(),
        &finalize_input(),
        &StageOutcome::Replace(RawJson::parse(b"{}").expect("json")),
    )
    .expect_err("replace forbidden");
    assert_eq!(
        error.code(),
        finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED
    );
}
