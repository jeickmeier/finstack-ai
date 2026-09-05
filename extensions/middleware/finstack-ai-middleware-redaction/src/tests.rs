use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    ContentBlock, Digest, Id, IdTag, LaneId, Message, MessageRole, Metadata, OperationLocator,
    OutputSpec, PrincipalRef, ProviderIds, RawJson, RunId, SessionId, Stage, TextBlock, Timestamp,
    ToolResultBlock,
};
use finstack_ai_runtime::ports::middleware::{
    BeforeModelInput, MiddlewareError, OrderTier, StageInput, StageOutcome,
};
use finstack_ai_runtime::ports::model::{
    AuthorizationContext, CancellationSignal, ModelName, ModelRequestDraft, ModelRequestLimits,
    ModelSettings, RunCallContext,
};

use crate::detect::Detectors;
use crate::{OutputPolicy, RedactionConfig, RedactionError, RedactionMiddleware};

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn middleware_context() -> finstack_ai_runtime::ports::middleware::MiddlewareContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    finstack_ai_runtime::ports::middleware::MiddlewareContext {
        run: RunCallContext {
            relation_depth: 0,
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
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
            effect_id: id::<finstack_ai_kernel::EffectTag>(4),
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

fn message(ordinal: u64, role: MessageRole, content: Vec<ContentBlock>) -> Message {
    Message::try_new(
        id(ordinal),
        role,
        content,
        Timestamp::from_unix_ms(i64::try_from(ordinal).expect("ts")).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn text(value: &str) -> ContentBlock {
    ContentBlock::Text(TextBlock::try_new(value).expect("text"))
}

fn draft_with_messages(messages: Vec<Message>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: ModelName::try_new("preview-model").expect("model"),
        messages: messages.into(),
        tools: Arc::from([]),
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 100_000_000,
            max_input_tokens: 10_000,
            max_output_tokens: 1_000,
        },
    }
}

fn before_model_input(messages: Vec<Message>) -> StageInput {
    StageInput::BeforeModel(Box::new(BeforeModelInput {
        request: draft_with_messages(messages),
        source_entries: Arc::from([]),
        model_context_profile_digest: Digest::raw_json(b"profile"),
        hard_input_tokens: 10_000,
        checkpoint: None,
    }))
}

fn invoke(middleware: &RedactionMiddleware, input: StageInput) -> StageOutcome {
    invoke_result(middleware, input).expect("invoke")
}

fn invoke_result(
    middleware: &RedactionMiddleware,
    input: StageInput,
) -> Result<StageOutcome, MiddlewareError> {
    block_on(middleware.invoke(middleware_context(), input))
}

/// H2: a Fail-policy construction path may `Fail` or `Err`, never `Continue`.
fn assert_never_continues(result: &Result<StageOutcome, MiddlewareError>) {
    match result {
        Ok(StageOutcome::Continue) => {
            panic!("fail-closed redaction must not Continue")
        }
        Ok(StageOutcome::Fail(_)) | Err(_) => {}
        Ok(other) => panic!("unexpected fail-closed outcome {other:?}"),
    }
}

fn replaced_draft(outcome: &StageOutcome) -> ModelRequestDraft {
    let StageOutcome::Replace(raw) = outcome else {
        panic!("expected Replace, got {outcome:?}");
    };
    serde_json::from_slice(raw.as_bytes()).expect("draft json")
}

fn detectors() -> Detectors {
    Detectors::try_new(RedactionConfig::default()).expect("detectors")
}

fn redacted(text: &str) -> String {
    detectors().redact(text).expect("expected a redaction")
}

fn untouched(text: &str) {
    assert_eq!(detectors().redact(text), None, "should not redact: {text}");
}

// Descriptor

#[test]
fn descriptor_identity_and_order() {
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let descriptor = middleware.descriptor();
    assert_eq!(
        descriptor.invocation.component.to_string(),
        "finstack.middleware.redaction"
    );
    assert_eq!(descriptor.order.tier, OrderTier::RequestShaping);
    assert_eq!(descriptor.order.priority, 0);
    assert!(descriptor.stages.contains(Stage::BeforeModel));
    assert!(!descriptor.stages.contains(Stage::AfterModel));
}

#[test]
fn fail_output_policy_declares_after_model() {
    let middleware = RedactionMiddleware::try_with_config(RedactionConfig {
        output_policy: OutputPolicy::Fail,
        ..RedactionConfig::default()
    })
    .expect("construct");
    let descriptor = middleware.descriptor();
    assert!(descriptor.stages.contains(Stage::BeforeModel));
    assert!(descriptor.stages.contains(Stage::AfterModel));
}

#[test]
fn all_detectors_disabled_is_a_configuration_error() {
    let result = RedactionMiddleware::try_with_config(RedactionConfig {
        detect_emails: false,
        detect_api_keys: false,
        detect_account_numbers: false,
        output_policy: OutputPolicy::Off,
    });
    assert_eq!(
        result.err(),
        Some(RedactionError::Configuration {
            reason: "all_detectors_disabled",
        })
    );
}

#[test]
fn configuration_digest_tracks_config() {
    let defaults = RedactionMiddleware::try_new().expect("construct");
    let no_emails = RedactionMiddleware::try_with_config(RedactionConfig {
        detect_emails: false,
        ..RedactionConfig::default()
    })
    .expect("construct");
    assert_ne!(
        defaults.descriptor().invocation.configuration_digest,
        no_emails.descriptor().invocation.configuration_digest,
    );
}

// Detection engine

#[test]
fn emails_are_redacted() {
    assert_eq!(
        redacted("reach me at jane.doe+x@example.co.uk today"),
        "reach me at [REDACTED:email] today"
    );
    untouched("user at host dot com");
}

#[test]
fn vendor_api_keys_are_redacted() {
    for secret in [
        "sk-proj-abcdefghij0123456789",
        "sk-ant-api03-abcdefghij0123456789",
        "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        "github_pat_11ABCDEFG0123456789_abcdefghij",
        "xoxb-1234567890-abcdefghij",
        "AKIAIOSFODNN7EXAMPLE",
        "AIzaSyA-1234567890abcdefghijklmnopqrstuv",
        "sk_live_abcdefghij0123456789",
    ] {
        let input = format!("token: {secret} end");
        assert_eq!(
            detectors().redact(&input).as_deref(),
            Some("token: [REDACTED:api-key] end"),
            "should redact {secret}"
        );
    }
    untouched("short sk-abc is not a key");
}

#[test]
fn jwts_are_redacted() {
    assert_eq!(
        redacted(
            "bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        ),
        "bearer [REDACTED:jwt]"
    );
    untouched("two segments eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0 only");
}

#[test]
fn pem_private_keys_are_redacted() {
    let block = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
    assert_eq!(
        detectors()
            .redact(&format!("cfg:\n{block}\ndone"))
            .as_deref(),
        Some("cfg:\n[REDACTED:private-key]\ndone")
    );
    assert_eq!(
        redacted("-----BEGIN PRIVATE KEY----- truncated paste"),
        "[REDACTED:private-key] truncated paste"
    );
}

#[test]
fn luhn_valid_cards_are_redacted() {
    for card in [
        "4111111111111111",
        "4111 1111 1111 1111",
        "4111-1111-1111-1111",
    ] {
        let input = format!("card {card} on file");
        assert_eq!(
            detectors().redact(&input).as_deref(),
            Some("card [REDACTED:card] on file"),
            "should redact {card}"
        );
    }
}

#[test]
fn invalid_card_candidates_are_untouched() {
    untouched("card 4111111111111112 on file"); // Luhn fails
    untouched("run 411111111111 short"); // 12 digits
    untouched("mixed 4111 1111-1111 1111 separators"); // non-uniform
}

#[test]
fn valid_ibans_are_redacted() {
    assert_eq!(
        redacted("pay GB82WEST12345698765432 now"),
        "pay [REDACTED:iban] now"
    );
    untouched("pay GB82WEST12345698765433 now"); // mod-97 fails
}

#[test]
fn overlapping_matches_redact_once() {
    // The JWT's middle segment contains an api-key-shaped substring; only
    // one marker must be produced, for the longer, earlier match.
    let text = "eyJhbGciOiJIUzI1NiJ9.eyJsk-abcdefghij0123456789In0.dBjftJeZ4CVPmB92K27uhb";
    assert_eq!(redacted(text), "[REDACTED:jwt]");
}

#[test]
fn redaction_is_idempotent() {
    let once = redacted("mail jane@example.com card 4111111111111111");
    assert_eq!(detectors().redact(&once), None);
}

#[test]
fn redaction_is_idempotent_for_marker_adjacent_candidates() {
    // Pass 1 redacts the card and rejects the IBAN (digit-adjacent). The
    // marker must keep blocking the IBAN on later passes, or the Replace
    // payload would differ between BeforeModel cycles.
    let once = redacted("4111111111111111GB82WEST12345698765432");
    assert_eq!(once, "[REDACTED:card]GB82WEST12345698765432");
    assert_eq!(detectors().redact(&once), None);
}

#[test]
fn card_embedded_in_rejected_candidate_is_still_redacted() {
    assert_eq!(
        redacted("2026-08-20 4111-1111-1111-1111"),
        "2026-08-20 [REDACTED:card]"
    );
    assert_eq!(
        redacted("Order 12345 4111111111111111"),
        "Order 12345 [REDACTED:card]"
    );
}

#[test]
fn truncated_pem_block_consumes_key_body() {
    assert_eq!(
        redacted("cfg:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEAvq\nQEFAAOCAQ8A=="),
        "cfg:\n[REDACTED:private-key]"
    );
}

#[test]
fn spaced_iban_grouping_is_redacted() {
    assert_eq!(
        redacted("pay GB82 WEST 1234 5698 7654 32 now"),
        "pay [REDACTED:iban] now"
    );
}

#[test]
fn email_does_not_swallow_following_sentence() {
    assert_eq!(
        redacted("Email a@b.com.See the runbook"),
        "Email [REDACTED:email].See the runbook"
    );
    // Uppercase emails whose TLD is the only remaining label stay intact.
    assert_eq!(
        redacted("mail USER@EXAMPLE.COM now"),
        "mail [REDACTED:email] now"
    );
}

#[test]
fn disabled_detector_groups_do_not_fire() {
    let config = RedactionConfig {
        detect_emails: false,
        ..RedactionConfig::default()
    };
    let detectors = Detectors::try_new(config).expect("detectors");
    assert_eq!(detectors.redact("mail jane@example.com"), None);
    assert!(
        detectors
            .redact("key sk-proj-abcdefghij0123456789")
            .is_some()
    );
}

#[test]
fn findings_report_detector_kinds_and_count() {
    let (kinds, count) =
        detectors().findings("mail jane@example.com key sk-proj-abcdefghij0123456789");
    assert!(kinds.contains("email"));
    assert!(kinds.contains("api-key"));
    assert!(!kinds.contains("card"));
    assert_eq!(count, 2);
}

// BeforeModel draft rewrite

#[test]
fn user_secret_is_redacted_in_replace_draft() {
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let original = message(
        1,
        MessageRole::User,
        vec![text("my key is sk-proj-abcdefghij0123456789 ok")],
    );
    let outcome = invoke(&middleware, before_model_input(vec![original.clone()]));
    let draft = replaced_draft(&outcome);
    assert_eq!(draft.messages.len(), 1);
    let rewritten = &draft.messages[0];
    assert_eq!(rewritten.id(), original.id());
    assert_eq!(rewritten.role(), MessageRole::User);
    assert_eq!(rewritten.created_at(), original.created_at());
    let ContentBlock::Text(block) = &rewritten.content()[0] else {
        panic!("expected text block");
    };
    assert_eq!(block.text(), "my key is [REDACTED:api-key] ok");
}

#[test]
fn assistant_history_is_redacted_next_turn() {
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let outcome = invoke(
        &middleware,
        before_model_input(vec![
            message(
                1,
                MessageRole::Assistant,
                vec![text("generated card 4111111111111111 for testing")],
            ),
            message(2, MessageRole::User, vec![text("thanks")]),
        ]),
    );
    let draft = replaced_draft(&outcome);
    let ContentBlock::Text(block) = &draft.messages[0].content()[0] else {
        panic!("expected text block");
    };
    assert_eq!(block.text(), "generated card [REDACTED:card] for testing");
    let ContentBlock::Text(clean) = &draft.messages[1].content()[0] else {
        panic!("expected text block");
    };
    assert_eq!(clean.text(), "thanks");
}

#[test]
fn tool_result_nested_text_is_redacted() {
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let result = ToolResultBlock::try_new(
        id(9),
        vec![text("env dump: AKIAIOSFODNN7EXAMPLE and more")],
        false,
    )
    .expect("tool result");
    let outcome = invoke(
        &middleware,
        before_model_input(vec![message(
            1,
            MessageRole::Tool,
            vec![ContentBlock::ToolResult(result)],
        )]),
    );
    let draft = replaced_draft(&outcome);
    let ContentBlock::ToolResult(rewritten) = &draft.messages[0].content()[0] else {
        panic!("expected tool result block");
    };
    assert_eq!(*rewritten.tool_call_id(), id(9));
    let ContentBlock::Text(block) = &rewritten.content()[0] else {
        panic!("expected nested text block");
    };
    assert_eq!(block.text(), "env dump: [REDACTED:api-key] and more");
}

#[test]
fn clean_draft_continues() {
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let outcome = invoke(
        &middleware,
        before_model_input(vec![message(
            1,
            MessageRole::User,
            vec![text("nothing sensitive here")],
        )]),
    );
    assert_eq!(outcome, StageOutcome::Continue);
}

#[test]
fn non_before_model_input_continues() {
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let outcome = invoke(
        &middleware,
        StageInput::BeforeRun {
            value: RawJson::parse(b"[]").expect("raw"),
        },
    );
    assert_eq!(outcome, StageOutcome::Continue);
}

// AfterModel output policy

fn after_model_input(message: &Message) -> StageInput {
    let bytes = serde_json_canonicalizer::to_vec(message).expect("canonical message");
    StageInput::AfterModel {
        value: RawJson::parse(bytes).expect("raw"),
    }
}

fn fail_policy_middleware() -> RedactionMiddleware {
    RedactionMiddleware::try_with_config(RedactionConfig {
        output_policy: OutputPolicy::Fail,
        ..RedactionConfig::default()
    })
    .expect("construct")
}

#[test]
fn fail_policy_fails_on_output_secret_without_leaking_it() {
    let secret = "sk-proj-abcdefghij0123456789";
    let assistant = message(
        1,
        MessageRole::Assistant,
        vec![text(&format!("here is your key {secret}"))],
    );
    let outcome = invoke(&fail_policy_middleware(), after_model_input(&assistant));
    let StageOutcome::Fail(descriptor) = outcome else {
        panic!("expected Fail, got {outcome:?}");
    };
    assert_eq!(descriptor.code.as_str(), "redaction_output_detected");
    assert!(descriptor.message.contains("api-key"));
    assert!(!descriptor.message.contains(secret));
}

#[test]
fn fail_policy_continues_on_clean_output() {
    let assistant = message(1, MessageRole::Assistant, vec![text("all clear")]);
    let outcome = invoke(&fail_policy_middleware(), after_model_input(&assistant));
    assert_eq!(outcome, StageOutcome::Continue);
}

#[test]
fn fail_policy_fails_closed_on_undecodable_payload() {
    let result = invoke_result(
        &fail_policy_middleware(),
        StageInput::AfterModel {
            value: RawJson::parse(b"{\"not\":\"a message\"}").expect("raw"),
        },
    );
    match result {
        Ok(StageOutcome::Fail(descriptor)) => {
            assert_eq!(descriptor.code.as_str(), "redaction_output_undecodable");
        }
        Ok(StageOutcome::Continue) => {
            panic!("undecodable Fail-policy output must not Continue")
        }
        Ok(other) => panic!("expected Fail, got {other:?}"),
        Err(error) => {
            assert!(!error.to_string().contains("not"));
        }
    }
}

#[test]
fn fail_outcome_fails_closed_on_oversized_message() {
    let oversized = "x".repeat(finstack_ai_kernel::TEXT_MAX_BYTES + 1);
    let result = super::fail_outcome("redaction_output_detected", oversized);
    assert_never_continues(&result);
    match result {
        Ok(StageOutcome::Fail(descriptor)) => {
            assert_eq!(descriptor.code.as_str(), "redaction_output_detected");
            assert_eq!(
                descriptor.message.as_ref(),
                super::FAIL_OUTCOME_FALLBACK_MESSAGE
            );
        }
        Err(error) => {
            assert!(!error.to_string().contains("xxxx"));
        }
        Ok(other) => panic!("expected Fail or Err, got {other:?}"),
    }
}

#[test]
fn fail_outcome_returns_err_when_descriptor_is_unconstructable() {
    let secret = "sk-proj-abcdefghij0123456789";
    let result = super::fail_outcome("Not_A_Valid_Code", format!("leak {secret}"));
    let error = match result {
        Err(error) => error,
        Ok(StageOutcome::Continue) => {
            panic!("unconstructable descriptor must not Continue")
        }
        Ok(other) => panic!("expected Err, got {other:?}"),
    };
    assert!(!error.to_string().contains(secret));
    assert!(!error.code().as_str().contains(secret));
    assert_eq!(error.code(), super::FAIL_CONSTRUCTION_CODE);
}

#[test]
fn off_policy_ignores_after_model() {
    let secret_message = message(
        1,
        MessageRole::Assistant,
        vec![text("key sk-proj-abcdefghij0123456789")],
    );
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let outcome = invoke(&middleware, after_model_input(&secret_message));
    assert_eq!(outcome, StageOutcome::Continue);
}

// Wrapping composition

use finstack_ai_kernel::{ComponentId, ComponentInvocation, InvocationRecovery, Version};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::middleware::{
    Middleware, MiddlewareDescriptor, MiddlewareOrder, MiddlewareRole, StageMask,
};

/// Stub Replace-emitting inner middleware standing in for document-ingest.
#[derive(Clone)]
struct StubInner {
    descriptor: MiddlewareDescriptor,
    outcome: StageOutcome,
}

impl StubInner {
    fn new(stages: StageMask, role: MiddlewareRole, outcome: StageOutcome) -> Self {
        Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.middleware.stub-ingest")
                        .expect("component"),
                    version: Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    configuration_digest: Digest::raw_json(b"stub"),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages,
                order: MiddlewareOrder {
                    tier: OrderTier::ContextMutation,
                    priority: 7,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role,
                metadata: Metadata::empty(),
            },
            outcome,
        }
    }

    fn before_model(outcome: StageOutcome) -> Arc<dyn Middleware> {
        Arc::new(Self::new(
            StageMask::from_stages([Stage::BeforeModel]),
            MiddlewareRole::Standard,
            outcome,
        ))
    }
}

impl Middleware for StubInner {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: finstack_ai_runtime::ports::middleware::MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let outcome = self.outcome.clone();
        Box::pin(async move { Ok(outcome) })
    }
}

fn replace_outcome(messages: Vec<Message>) -> StageOutcome {
    let bytes =
        serde_json_canonicalizer::to_vec(&draft_with_messages(messages)).expect("canonical");
    StageOutcome::Replace(RawJson::parse(bytes).expect("raw"))
}

#[test]
fn wrapper_redacts_inner_replace_payload() {
    let inner = StubInner::before_model(replace_outcome(vec![message(
        1,
        MessageRole::User,
        vec![text("ingested doc says key sk-proj-abcdefghij0123456789")],
    )]));
    let wrapper =
        RedactionMiddleware::try_wrapping(inner, RedactionConfig::default()).expect("wrap");
    let outcome = invoke(
        &wrapper,
        before_model_input(vec![message(2, MessageRole::User, vec![text("clean")])]),
    );
    let draft = replaced_draft(&outcome);
    let ContentBlock::Text(block) = &draft.messages[0].content()[0] else {
        panic!("expected text block");
    };
    assert_eq!(block.text(), "ingested doc says key [REDACTED:api-key]");
}

#[test]
fn wrapper_redacts_base_draft_when_inner_continues() {
    let inner = StubInner::before_model(StageOutcome::Continue);
    let wrapper =
        RedactionMiddleware::try_wrapping(inner, RedactionConfig::default()).expect("wrap");
    let outcome = invoke(
        &wrapper,
        before_model_input(vec![message(
            1,
            MessageRole::User,
            vec![text("mail jane@example.com")],
        )]),
    );
    let draft = replaced_draft(&outcome);
    let ContentBlock::Text(block) = &draft.messages[0].content()[0] else {
        panic!("expected text block");
    };
    assert_eq!(block.text(), "mail [REDACTED:email]");
}

#[test]
fn wrapper_passes_through_inner_terminal_outcomes() {
    let descriptor = finstack_ai_kernel::ErrorDescriptor::new(
        "stub_failed",
        "stub failure",
        finstack_ai_kernel::ErrorCategory::Middleware,
        false,
    )
    .expect("descriptor");
    let inner = StubInner::before_model(StageOutcome::Fail(Box::new(descriptor)));
    let wrapper =
        RedactionMiddleware::try_wrapping(inner, RedactionConfig::default()).expect("wrap");
    let outcome = invoke(
        &wrapper,
        before_model_input(vec![message(
            1,
            MessageRole::User,
            vec![text("mail jane@example.com")],
        )]),
    );
    let StageOutcome::Fail(failed) = outcome else {
        panic!("expected passthrough Fail, got {outcome:?}");
    };
    assert_eq!(failed.code.as_str(), "stub_failed");
}

#[test]
fn wrapper_rejects_unparsable_inner_replace() {
    let inner = StubInner::before_model(StageOutcome::Replace(
        RawJson::parse(b"{\"not\":\"a draft\"}").expect("raw"),
    ));
    let wrapper =
        RedactionMiddleware::try_wrapping(inner, RedactionConfig::default()).expect("wrap");
    let error = invoke_result(
        &wrapper,
        before_model_input(vec![message(1, MessageRole::User, vec![text("clean")])]),
    )
    .expect_err("malformed replacement must fail closed");
    assert_eq!(error.code(), super::REDACTION_REWRITE_CODE);
}

#[test]
fn wrapper_rejects_non_before_model_inner() {
    let inner: Arc<dyn Middleware> = Arc::new(StubInner::new(
        StageMask::from_stages([Stage::BeforeModel, Stage::AfterModel]),
        MiddlewareRole::Standard,
        StageOutcome::Continue,
    ));
    let result = RedactionMiddleware::try_wrapping(inner, RedactionConfig::default());
    assert_eq!(
        result.err(),
        Some(RedactionError::Configuration {
            reason: "wrapped_middleware_not_before_model_only",
        })
    );
}

#[test]
fn wrapper_rejects_non_standard_inner_role() {
    let inner: Arc<dyn Middleware> = Arc::new(StubInner::new(
        StageMask::from_stages([Stage::BeforeModel]),
        MiddlewareRole::PostCompactionValidator,
        StageOutcome::Continue,
    ));
    let result = RedactionMiddleware::try_wrapping(inner, RedactionConfig::default());
    assert_eq!(
        result.err(),
        Some(RedactionError::Configuration {
            reason: "wrapped_middleware_not_standard_role",
        })
    );
}

#[test]
fn wrapper_rejects_non_recompute_safe_inner() {
    let mut inner = StubInner::new(
        StageMask::from_stages([Stage::BeforeModel]),
        MiddlewareRole::Standard,
        StageOutcome::Continue,
    );
    inner.descriptor.invocation.recovery = InvocationRecovery::Reconcile;
    let result = RedactionMiddleware::try_wrapping(Arc::new(inner), RedactionConfig::default());
    assert_eq!(
        result.err(),
        Some(RedactionError::Configuration {
            reason: "wrapped_middleware_not_recompute_safe",
        })
    );
}

#[test]
fn wrapper_descriptor_adopts_inner_order() {
    let inner = StubInner::before_model(StageOutcome::Continue);
    let wrapper =
        RedactionMiddleware::try_wrapping(inner, RedactionConfig::default()).expect("wrap");
    let descriptor = wrapper.descriptor();
    assert_eq!(
        descriptor.invocation.component.to_string(),
        "finstack.middleware.redaction"
    );
    assert_eq!(descriptor.order.tier, OrderTier::ContextMutation);
    assert_eq!(descriptor.order.priority, 7);
    let standalone = RedactionMiddleware::try_new().expect("construct");
    assert_ne!(
        descriptor.invocation.configuration_digest,
        standalone.descriptor().invocation.configuration_digest,
    );
}

// Constructor assembly identity

const STANDALONE_DEFAULT_DIGEST_HEX: &str =
    "8399ab7e4cf363b4663fc1a5450c3c5db1031fe3d9feeadc5fee4cf371de0522";
const STANDALONE_FAIL_DIGEST_HEX: &str =
    "302f9002bc942153f2ce5182eac2ae46058a632002ac95509939cf3b21097e99";
const WRAPPING_DEFAULT_STUB_DIGEST_HEX: &str =
    "01289ce99a47a939279f326ab90c3d4f98815e7a5fe7b6557d8cf6a60dfd43be";

fn expected_configuration_digest(
    config: RedactionConfig,
    inner: Option<&ComponentInvocation>,
) -> Digest {
    let value = serde_json::json!({
        "config": config,
        "wraps": inner.map(|invocation| serde_json::json!({
            "component": invocation.component,
            "version": invocation.version,
            "configuration_digest": invocation.configuration_digest,
            "recovery": invocation.recovery,
        })),
    });
    let bytes = serde_json_canonicalizer::to_vec(&value).expect("canonical");
    Digest::raw_json(&bytes)
}

#[test]
fn try_new_matches_try_with_config_default_identity() {
    let via_new = RedactionMiddleware::try_new().expect("try_new");
    let via_config =
        RedactionMiddleware::try_with_config(RedactionConfig::default()).expect("try_with_config");
    let new_descriptor = via_new.descriptor();
    let config_descriptor = via_config.descriptor();
    assert_eq!(new_descriptor.order, config_descriptor.order);
    assert_eq!(new_descriptor.stages, config_descriptor.stages);
    assert_eq!(
        new_descriptor.invocation.configuration_digest.as_bytes(),
        config_descriptor.invocation.configuration_digest.as_bytes(),
    );
    assert_eq!(
        new_descriptor.invocation.configuration_digest.to_hex(),
        STANDALONE_DEFAULT_DIGEST_HEX,
    );
    assert_eq!(
        new_descriptor.invocation.configuration_digest.as_bytes(),
        expected_configuration_digest(RedactionConfig::default(), None).as_bytes(),
    );
}

#[test]
fn standalone_constructors_pin_order_and_stage_masks() {
    let off = RedactionMiddleware::try_new().expect("try_new");
    assert_eq!(off.descriptor().order.tier, OrderTier::RequestShaping);
    assert_eq!(off.descriptor().order.priority, 0);
    assert!(off.descriptor().order.before.is_empty());
    assert!(off.descriptor().order.after.is_empty());
    assert_eq!(
        off.descriptor().stages,
        StageMask::from_stages([Stage::BeforeModel]),
    );

    let fail = RedactionMiddleware::try_with_config(RedactionConfig {
        output_policy: OutputPolicy::Fail,
        ..RedactionConfig::default()
    })
    .expect("fail config");
    assert_eq!(fail.descriptor().order.tier, OrderTier::RequestShaping);
    assert_eq!(
        fail.descriptor().stages,
        StageMask::from_stages([Stage::BeforeModel, Stage::AfterModel]),
    );
    assert_eq!(
        fail.descriptor().invocation.configuration_digest.to_hex(),
        STANDALONE_FAIL_DIGEST_HEX,
    );
}

#[test]
fn wrapping_constructor_pins_order_mask_validation_and_digest_bytes() {
    let inner = StubInner::before_model(StageOutcome::Continue);
    let default_config = RedactionConfig::default();
    let wrapper = RedactionMiddleware::try_wrapping(inner.clone(), default_config).expect("wrap");
    let descriptor = wrapper.descriptor();
    let inner_invocation = inner.descriptor().invocation;
    assert_eq!(descriptor.order.tier, OrderTier::ContextMutation);
    assert_eq!(descriptor.order.priority, 7);
    assert_eq!(
        descriptor.stages,
        StageMask::from_stages([Stage::BeforeModel]),
    );
    assert_eq!(descriptor.role, MiddlewareRole::Standard);
    assert_eq!(
        descriptor.invocation.configuration_digest.to_hex(),
        WRAPPING_DEFAULT_STUB_DIGEST_HEX,
    );
    assert_eq!(
        descriptor.invocation.configuration_digest.as_bytes(),
        expected_configuration_digest(default_config, Some(&inner_invocation)).as_bytes(),
    );

    let fail_wrapper = RedactionMiddleware::try_wrapping(
        StubInner::before_model(StageOutcome::Continue),
        RedactionConfig {
            output_policy: OutputPolicy::Fail,
            ..RedactionConfig::default()
        },
    )
    .expect("wrap fail");
    assert_eq!(
        fail_wrapper.descriptor().order.tier,
        OrderTier::ContextMutation
    );
    assert_eq!(
        fail_wrapper.descriptor().stages,
        StageMask::from_stages([Stage::BeforeModel, Stage::AfterModel]),
    );

    let invalid_stages: Arc<dyn Middleware> = Arc::new(StubInner::new(
        StageMask::from_stages([Stage::BeforeModel, Stage::AfterModel]),
        MiddlewareRole::Standard,
        StageOutcome::Continue,
    ));
    assert_eq!(
        RedactionMiddleware::try_wrapping(invalid_stages, RedactionConfig::default()).err(),
        Some(RedactionError::Configuration {
            reason: "wrapped_middleware_not_before_model_only",
        })
    );

    let invalid_role: Arc<dyn Middleware> = Arc::new(StubInner::new(
        StageMask::from_stages([Stage::BeforeModel]),
        MiddlewareRole::PostCompactionValidator,
        StageOutcome::Continue,
    ));
    assert_eq!(
        RedactionMiddleware::try_wrapping(invalid_role, RedactionConfig::default()).err(),
        Some(RedactionError::Configuration {
            reason: "wrapped_middleware_not_standard_role",
        })
    );
}

#[test]
fn oversized_rewrite_uses_bounded_marker() {
    // The marker for this email is longer than the email itself, so the
    // rewritten text would exceed TextBlock's byte ceiling; the detected
    // content must be replaced by the bounded oversize marker.
    let secret = "a@b.co";
    let filler = "!".repeat(finstack_ai_kernel::TEXT_MAX_BYTES - secret.len() - 1);
    let oversized = format!("{filler} {secret}");
    let middleware = RedactionMiddleware::try_new().expect("construct");
    let outcome = invoke(
        &middleware,
        before_model_input(vec![message(1, MessageRole::User, vec![text(&oversized)])]),
    );
    let draft = replaced_draft(&outcome);
    let ContentBlock::Text(block) = &draft.messages[0].content()[0] else {
        panic!("expected text block");
    };
    assert_eq!(block.text(), super::OVERSIZED_MARKER);
}
