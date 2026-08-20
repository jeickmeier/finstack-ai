use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use finstack_ai_kernel::{
    ContentBlock, Digest, EffectId, LaneId, Message, MessageRole, Metadata, OperationLocator,
    OutputSpec, PrincipalRef, ProviderIds, RawJson, RunId, SessionId, TextBlock, Timestamp,
};
use finstack_ai_runtime::{
    AuthorizationContext, BeforeModelInput, CancellationSignal, Middleware, MiddlewareContext,
    ModelName, ModelRequestDraft, ModelRequestLimits, ModelSettings, RunCallContext, StageInput,
    StageOutcome, validate_stage_outcome,
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

fn message_id(value: u64) -> finstack_ai_kernel::MessageId {
    id(value, |v| finstack_ai_kernel::MessageId::parse(v).expect("message id"))
}

fn text_message(ordinal: u64, role: MessageRole, text: &str) -> Message {
    Message::try_new(
        message_id(ordinal),
        role,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        Timestamp::from_unix_ms(i64::try_from(ordinal).expect("ts")).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn message_raw_json(text: &str) -> RawJson {
    RawJson::parse(format!(r#"{{"role":"assistant","text":"{text}"}}"#)).expect("raw message")
}

fn policy() -> VerifyPolicy {
    VerifyPolicy::try_new(finstack_ai_kernel::Duration::from_millis(250), "policy-v1")
        .expect("policy")
}

fn finalize_input(text: &str, has_result: bool) -> StageInput {
    StageInput::BeforeFinalize {
        candidate: RawJson::parse(br#""candidate""#).expect("candidate"),
        result_message: if has_result {
            Some(message_raw_json(text))
        } else {
            None
        },
    }
}

fn before_model_input(messages: Vec<Message>) -> StageInput {
    StageInput::BeforeModel(Box::new(BeforeModelInput {
        request: ModelRequestDraft {
            model: ModelName::try_new("preview-model").expect("model"),
            messages: messages.into(),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 10_000,
                max_output_tokens: 1_000,
            },
        },
        source_entries: Arc::from([]),
        model_context_profile_digest: Digest::raw_json(b"profile"),
        hard_input_tokens: 10_000,
        checkpoint: None,
    }))
}

/// Scripted verifier keyed on message text: text containing "bounce" or
/// "reject" drives the matching verdict with one finding each; anything
/// else is accepted. Every call is counted.
#[derive(Debug, Default)]
struct ScriptedVerifier {
    calls: AtomicUsize,
}

impl ScriptedVerifier {
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl EvidenceVerifier for ScriptedVerifier {
    fn verifier_id(&self) -> &'static str {
        "scripted-verifier"
    }

    fn verify(&self, message: &RawJson) -> Verdict {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = message.as_str();
        if text.contains("bounce") {
            Verdict::Bounce(vec![
                EvidenceFinding::try_new(EvidenceKind::Citation, "missing citation")
                    .expect("finding"),
            ])
        } else if text.contains("reject") {
            Verdict::Reject(vec![
                EvidenceFinding::try_new(EvidenceKind::Test, "failing test").expect("finding"),
            ])
        } else {
            Verdict::Accept
        }
    }
}

fn middleware(verifier: Arc<dyn EvidenceVerifier>) -> VerifyMiddleware {
    VerifyMiddleware::try_new(verifier, policy()).expect("middleware")
}

/// Verifier that records the exact bytes it was asked to judge, and always
/// accepts.
#[derive(Debug, Default)]
struct RecordingVerifier {
    observed: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
}

impl EvidenceVerifier for RecordingVerifier {
    fn verifier_id(&self) -> &'static str {
        "recording-verifier"
    }

    fn verify(&self, message: &RawJson) -> Verdict {
        *self.observed.lock().expect("lock") = Some(message.as_bytes().to_vec());
        Verdict::Accept
    }
}

#[tokio::test]
async fn before_finalize_accept_continues() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let input = finalize_input("looks fine", true);
    let outcome = mw.invoke(ctx(), input.clone()).await.expect("invoke");
    assert_eq!(outcome, StageOutcome::Continue);
    validate_stage_outcome(&mw.descriptor(), &input, &outcome).expect("allowed");
}

#[tokio::test]
async fn before_finalize_bounce_retries_with_verification_classification() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let input = finalize_input("please bounce this", true);
    let outcome = mw.invoke(ctx(), input.clone()).await.expect("invoke");
    match &outcome {
        StageOutcome::Retry(directive) => {
            assert_eq!(
                directive.classification,
                finstack_ai_kernel::RetryClassification::Verification
            );
            assert_eq!(directive.backoff.as_millis(), 250);
            assert_eq!(directive.policy_version.as_ref(), "policy-v1");
        }
        other => panic!("expected retry, got {other:?}"),
    }
    validate_stage_outcome(&mw.descriptor(), &input, &outcome).expect("retry allowed");
}

#[tokio::test]
async fn before_finalize_reject_fails_with_stable_code() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let input = finalize_input("please reject this", true);
    let outcome = mw.invoke(ctx(), input.clone()).await.expect("invoke");
    match &outcome {
        StageOutcome::Fail(error) => {
            assert_eq!(error.code.as_str(), "verify_rejected");
            assert!(!error.retryable);
            assert_eq!(error.category, finstack_ai_kernel::ErrorCategory::Validation);
        }
        other => panic!("expected fail, got {other:?}"),
    }
    validate_stage_outcome(&mw.descriptor(), &input, &outcome).expect("fail allowed");
}

#[tokio::test]
async fn before_finalize_none_result_message_short_circuits_without_calling_verifier() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(Arc::clone(&verifier) as Arc<dyn EvidenceVerifier>);
    let input = finalize_input("irrelevant", false);
    let outcome = mw.invoke(ctx(), input).await.expect("invoke");
    assert_eq!(outcome, StageOutcome::Continue);
    assert_eq!(verifier.call_count(), 0, "verifier must not run on a failed candidate");
}

#[tokio::test]
async fn before_model_bounce_adds_feedback_context() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let assistant = text_message(1, MessageRole::Assistant, "please bounce this");
    let input = before_model_input(vec![text_message(0, MessageRole::User, "question"), assistant]);
    let outcome = mw.invoke(ctx(), input).await.expect("invoke");
    match outcome {
        StageOutcome::AddContext(items) => {
            assert_eq!(items.len(), 1);
            let ContentBlock::Text(block) = &items[0].content[0] else {
                panic!("expected text block");
            };
            let text = block.text();
            assert!(text.contains("Evidence verification rejected the previous answer"));
            assert!(text.contains("[citation]"));
            assert!(text.contains("missing citation"));
        }
        other => panic!("expected add_context, got {other:?}"),
    }
}

#[tokio::test]
async fn before_model_reject_adds_feedback_context() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let assistant = text_message(1, MessageRole::Assistant, "please reject this");
    let input = before_model_input(vec![assistant]);
    let outcome = mw.invoke(ctx(), input).await.expect("invoke");
    match outcome {
        StageOutcome::AddContext(items) => {
            let ContentBlock::Text(block) = &items[0].content[0] else {
                panic!("expected text block");
            };
            let text = block.text();
            assert!(text.contains("[test]"));
            assert!(text.contains("failing test"));
        }
        other => panic!("expected add_context, got {other:?}"),
    }
}

#[tokio::test]
async fn before_model_no_assistant_message_continues() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let input = before_model_input(vec![text_message(0, MessageRole::User, "please bounce this")]);
    let outcome = mw.invoke(ctx(), input).await.expect("invoke");
    assert_eq!(outcome, StageOutcome::Continue);
}

#[tokio::test]
async fn before_model_empty_draft_continues() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let input = before_model_input(vec![]);
    let outcome = mw.invoke(ctx(), input).await.expect("invoke");
    assert_eq!(outcome, StageOutcome::Continue);
}

#[tokio::test]
async fn before_model_accept_continues() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let assistant = text_message(1, MessageRole::Assistant, "all good here");
    let input = before_model_input(vec![assistant]);
    let outcome = mw.invoke(ctx(), input).await.expect("invoke");
    assert_eq!(outcome, StageOutcome::Continue);
}

#[tokio::test]
async fn before_model_verifier_receives_canonical_message_json_identical_to_before_finalize() {
    let observed: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(None));
    let verifier = Arc::new(RecordingVerifier {
        observed: Arc::clone(&observed),
    });
    let mw = middleware(verifier);
    let assistant = text_message(7, MessageRole::Assistant, "capturing this message");
    let input = before_model_input(vec![assistant.clone()]);
    mw.invoke(ctx(), input).await.expect("invoke");

    let expected =
        serde_json_canonicalizer::to_vec(&assistant).expect("canonical message bytes");
    let observed_bytes = observed.lock().expect("lock").clone().expect("verifier called");
    assert_eq!(
        observed_bytes, expected,
        "before_model must hand the verifier byte-identical JCS-canonical Message JSON"
    );
}

#[tokio::test]
async fn oversized_finding_note_is_truncated_not_rejected() {
    let oversized = "x".repeat(TEXT_MAX_BYTES + 16);
    let finding =
        EvidenceFinding::try_new(EvidenceKind::Artifact, &oversized).expect("truncated finding");
    assert!(finding.note.len() <= TEXT_MAX_BYTES);
    assert!(finding.note.len() < oversized.len());
}

#[tokio::test]
async fn wrong_stage_still_errors() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let error = mw
        .invoke(
            ctx(),
            StageInput::AfterModel {
                value: RawJson::parse(b"{}").expect("value"),
            },
        )
        .await
        .expect_err("wrong stage must error");
    assert_eq!(error.code(), MIDDLEWARE_OUTCOME_NOT_ALLOWED);
}

#[tokio::test]
async fn middleware_satisfies_the_published_port_conformance_suite() {
    let verifier = Arc::new(ScriptedVerifier::default());
    let mw = middleware(verifier);
    let outcome = check_middleware_conformance(
        &mw,
        MiddlewareConformanceCase {
            context: ctx(),
            input: finalize_input("looks fine", true),
            expected: StageOutcome::Continue,
        },
    )
    .await
    .expect("published middleware conformance suite");
    assert_eq!(outcome, StageOutcome::Continue);
}
