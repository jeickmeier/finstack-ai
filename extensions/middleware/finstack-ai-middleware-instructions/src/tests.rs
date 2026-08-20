use super::*;

fn entry(label: &str, text: &str) -> PolicyEntry {
    PolicyEntry {
        label: label.to_owned(),
        text: text.to_owned(),
    }
}

#[test]
fn valid_config_passes_validation() {
    let config = PolicyInstructionsConfig {
        entries: vec![
            entry(
                "compliance-footer",
                "All outputs are for tenant-a internal use only.",
            ),
            entry("as-of", "Treat 2026-08-20 as the current date."),
        ],
    };
    config.validate().expect("valid");
}

#[test]
fn empty_entries_are_rejected() {
    let config = PolicyInstructionsConfig { entries: vec![] };
    let error = config.validate().expect_err("empty");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_empty"
    );
}

#[test]
fn more_than_sixteen_entries_are_rejected() {
    let config = PolicyInstructionsConfig {
        entries: (0..17)
            .map(|i| entry(&format!("rule-{i}"), "text"))
            .collect(),
    };
    let error = config.validate().expect_err("too many");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_exceed_maximum"
    );
}

#[test]
fn blank_label_or_text_is_rejected() {
    let blank_label = PolicyInstructionsConfig {
        entries: vec![entry("", "text")],
    };
    assert_eq!(
        blank_label.validate().expect_err("label").to_string(),
        "instructions_configuration_invalid: entry_label_empty"
    );
    let blank_text = PolicyInstructionsConfig {
        entries: vec![entry("locale", "   ")],
    };
    assert_eq!(
        blank_text.validate().expect_err("text").to_string(),
        "instructions_configuration_invalid: entry_text_empty"
    );
}

#[test]
fn oversized_label_is_rejected() {
    let ok = PolicyInstructionsConfig {
        entries: vec![entry(&"a".repeat(MAX_POLICY_LABEL_BYTES), "text")],
    };
    ok.validate()
        .expect("a 249-byte label still fits the 256-byte source id");

    let too_long = PolicyInstructionsConfig {
        entries: vec![entry(&"a".repeat(MAX_POLICY_LABEL_BYTES + 1), "text")],
    };
    assert_eq!(
        too_long.validate().expect_err("too long").to_string(),
        "instructions_configuration_invalid: entry_label_too_long"
    );
}

#[test]
fn label_with_nul_is_rejected() {
    let config = PolicyInstructionsConfig {
        entries: vec![entry("bad\0label", "text")],
    };
    assert_eq!(
        config.validate().expect_err("nul").to_string(),
        "instructions_configuration_invalid: entry_label_invalid"
    );
}

use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RawJson, RunId, SessionId,
    Stage,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, ContextAuthority, ContextItemKind, Middleware,
    MiddlewareContext, MiddlewareRole, OrderTier, RunCallContext, StageInput, StageOutcome,
    validate_stage_outcome,
};
use finstack_ai_test::{MiddlewareConformanceCase, check_middleware_conformance};

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

fn prepare_input() -> StageInput {
    StageInput::PrepareContext {
        value: RawJson::parse(b"[]").expect("messages"),
    }
}

fn sample_config() -> PolicyInstructionsConfig {
    PolicyInstructionsConfig {
        entries: vec![
            PolicyEntry {
                label: "compliance-footer".to_owned(),
                text: "All outputs are for tenant-a internal use only.".to_owned(),
            },
            PolicyEntry {
                label: "as-of".to_owned(),
                text: "Treat 2026-08-20 as the current date.".to_owned(),
            },
        ],
    }
}

#[tokio::test]
async fn invoke_adds_protected_instruction_items_in_entry_order() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let outcome = middleware
        .invoke(ctx(), prepare_input())
        .await
        .expect("invoke");
    let StageOutcome::AddInstructions(items) = &outcome else {
        panic!("expected AddInstructions, got {outcome:?}");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(&*items[0].provenance.source_id, "policy:compliance-footer");
    assert_eq!(&*items[1].provenance.source_id, "policy:as-of");
    for item in items.iter() {
        assert_eq!(item.kind, ContextItemKind::Instruction);
        assert_eq!(item.authority, ContextAuthority::TrustedApplication);
        assert!(item.protected);
        assert!(!item.provenance.external);
    }
    validate_stage_outcome(&middleware.descriptor(), &prepare_input(), &outcome)
        .expect("allowed at prepare_context");
}

#[tokio::test]
async fn descriptor_declares_prepare_context_standard_role() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let descriptor = middleware.descriptor();
    assert!(descriptor.stages.contains(Stage::PrepareContext));
    assert!(!descriptor.stages.contains(Stage::BeforeModel));
    assert_eq!(descriptor.role, MiddlewareRole::Standard);
    assert_eq!(
        descriptor.invocation.component,
        finstack_ai_kernel::ComponentId::parse("finstack.middleware.instructions")
            .expect("component id")
    );
}

#[tokio::test]
async fn descriptor_and_item_fields_are_pinned() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let descriptor = middleware.descriptor();
    assert_eq!(descriptor.order.tier, OrderTier::Standard);
    assert_eq!(descriptor.order.priority, 0);
    assert_eq!(
        descriptor.invocation.version,
        finstack_ai_kernel::Version {
            major: 1,
            minor: 0,
            patch: 0,
        }
    );
    assert_eq!(
        descriptor.invocation.recovery,
        finstack_ai_kernel::InvocationRecovery::RecomputeSafe
    );
    let outcome = middleware
        .invoke(ctx(), prepare_input())
        .await
        .expect("invoke");
    let StageOutcome::AddInstructions(items) = &outcome else {
        panic!("expected AddInstructions, got {outcome:?}");
    };
    for item in items.iter() {
        assert_eq!(item.priority, 0);
        assert_eq!(item.sensitivity, finstack_ai_kernel::Sensitivity::Internal);
    }
}

#[tokio::test]
async fn distinct_configs_produce_distinct_digests() {
    let first = InstructionsMiddleware::try_new(sample_config()).expect("first");
    let mut other = sample_config();
    other.entries[1].text = "Treat 2026-08-21 as the current date.".to_owned();
    let second = InstructionsMiddleware::try_new(other).expect("second");
    assert_ne!(
        first.descriptor().invocation.configuration_digest,
        second.descriptor().invocation.configuration_digest
    );
    let same = InstructionsMiddleware::try_new(sample_config()).expect("same");
    assert_eq!(
        first.descriptor().invocation.configuration_digest,
        same.descriptor().invocation.configuration_digest
    );
}

#[tokio::test]
async fn wrong_stage_input_is_rejected() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let error = middleware
        .invoke(
            ctx(),
            StageInput::BeforeFinalize {
                candidate: RawJson::parse(br#""candidate""#).expect("candidate"),
            },
        )
        .await
        .expect_err("wrong stage");
    assert_eq!(
        error.code(),
        finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED
    );
}

#[tokio::test]
async fn invalid_config_is_rejected_at_construction() {
    let error = InstructionsMiddleware::try_new(PolicyInstructionsConfig { entries: vec![] })
        .expect_err("invalid");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_empty"
    );
}

#[tokio::test]
async fn middleware_satisfies_the_published_port_conformance_suite() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let expected = match middleware
        .invoke(ctx(), prepare_input())
        .await
        .expect("expected outcome")
    {
        outcome @ StageOutcome::AddInstructions(_) => outcome,
        other => panic!("expected AddInstructions, got {other:?}"),
    };
    let outcome = check_middleware_conformance(
        &middleware,
        MiddlewareConformanceCase {
            context: ctx(),
            input: prepare_input(),
            expected: expected.clone(),
        },
    )
    .await
    .expect("published middleware conformance suite");
    assert_eq!(outcome, expected);
}
