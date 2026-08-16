use std::sync::Arc;

use finstack_ai_kernel::{
    ContentBlock, EntryTag, Id, IdTag, Message, MessageRole, Metadata, ProviderIds, RawJson,
    Sensitivity, TextBlock, Timestamp, ToolCallBlock, ToolCallTag, ToolResultBlock,
};
use finstack_ai_runtime::{
    AuthorizationContext, BeforeModelInput, COMPACTION_MODEL_NOT_AUTHORIZED, CancellationSignal,
    CompactionModelResume, CompactionSourceEntry, ComponentRef, Digest, Middleware,
    MiddlewareContext, ModelName, ModelRequestDraft, ModelRequestLimits, ModelResponse,
    ModelSettings, OperationLocator, OutputSpec, PrincipalRef, RunCallContext, StageInput,
    StageOutcome, Usage, compaction_checkpoint_compatible, validate_stage_outcome,
};
use finstack_ai_test::{
    CompactionConformanceCase, MiddlewareConformanceCase, SharedCompactionProjection,
    check_compaction_conformance, check_middleware_conformance,
};

use super::*;

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
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

fn text(value: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text(TextBlock::try_new(value).expect("text"))]
}

fn history() -> BeforeModelInput {
    let call_id = id::<ToolCallTag>(50);
    let entries: Arc<[CompactionSourceEntry]> = Arc::from([
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(20),
            message: message(10, MessageRole::System, text("system policy")),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"sys"),
            protected: true,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(21),
            message: message(
                11,
                MessageRole::User,
                text(&"old question that can drop ".repeat(40)),
            ),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"old"),
            protected: false,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(22),
            message: message(
                12,
                MessageRole::Assistant,
                vec![ContentBlock::ToolCall(
                    ToolCallBlock::try_new(
                        call_id,
                        "lookup",
                        RawJson::parse(b"{\"q\":\"old\"}").expect("args"),
                    )
                    .expect("call"),
                )],
            ),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"call"),
            protected: false,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(23),
            message: message(
                13,
                MessageRole::Tool,
                vec![ContentBlock::ToolResult(
                    ToolResultBlock::try_new(call_id, text(&"tool-body-".repeat(80)), false)
                        .expect("result"),
                )],
            ),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"result"),
            protected: false,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(24),
            message: message(14, MessageRole::User, text("current question")),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"user"),
            protected: true,
        },
    ]);
    BeforeModelInput {
        request: ModelRequestDraft {
            model: ModelName::try_new("preview-model").expect("model"),
            messages: entries
                .iter()
                .map(|entry| entry.message.clone())
                .collect::<Vec<_>>()
                .into(),
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
        source_entries: entries,
        model_context_profile_digest: Digest::raw_json(b"profile"),
        hard_input_tokens: 10_000,
        checkpoint: None,
    }
}

fn middleware_ctx(resume: Option<CompactionModelResume>) -> MiddlewareContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    MiddlewareContext {
        run: RunCallContext {
            locator: OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator"),
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
        compaction_resume: resume,
    }
}

async fn invoke(
    middleware: &CompactionMiddleware,
    input: BeforeModelInput,
    resume: Option<CompactionModelResume>,
) -> Result<StageOutcome, MiddlewareError> {
    middleware
        .invoke(
            middleware_ctx(resume),
            StageInput::BeforeModel(Box::new(input)),
        )
        .await
}

#[tokio::test]
async fn below_threshold_continues_without_rewrite() {
    let middleware = CompactionMiddleware::try_new(CompactionConfig::sliding_window(50_000, 0))
        .expect("middleware");
    let outcome = invoke(&middleware, history(), None).await.expect("invoke");
    assert_eq!(outcome, StageOutcome::Continue);
}

#[tokio::test]
async fn middleware_satisfies_the_published_port_conformance_suite() {
    let middleware = CompactionMiddleware::try_new(CompactionConfig::sliding_window(50_000, 0))
        .expect("middleware");
    let outcome = check_middleware_conformance(
        &middleware,
        MiddlewareConformanceCase {
            context: middleware_ctx(None),
            input: StageInput::BeforeModel(Box::new(history())),
            expected: StageOutcome::Continue,
        },
    )
    .await
    .expect("published middleware conformance suite");
    assert_eq!(outcome, StageOutcome::Continue);
}

#[tokio::test]
async fn sliding_window_preserves_protected_bytes_and_passes_conformance() {
    let middleware = CompactionMiddleware::try_new(CompactionConfig::sliding_window(250, 0))
        .expect("middleware");
    let input = history();
    let before = serde_json::to_vec(&input.source_entries).expect("before");
    let outcome = invoke(&middleware, input.clone(), None)
        .await
        .expect("invoke");
    let after = serde_json::to_vec(&input.source_entries).expect("after");
    assert_eq!(before, after);
    let StageOutcome::CompactContext(result) = outcome else {
        panic!("expected compact context");
    };
    let protected = input
        .source_entries
        .iter()
        .filter(|entry| entry.protected)
        .collect::<Vec<_>>();
    for entry in protected {
        let replacement = result
            .replacement_messages
            .iter()
            .find(|message| message.id() == entry.message.id())
            .expect("protected retained");
        assert_eq!(
            serde_json::to_vec(replacement).expect("rep"),
            serde_json::to_vec(&entry.message).expect("src")
        );
    }
    validate_stage_outcome(
        &middleware.descriptor(),
        &StageInput::BeforeModel(Box::new(input.clone())),
        &StageOutcome::CompactContext(result.clone()),
    )
    .expect("stage");
    let projection: Arc<[u8]> = serde_json_canonicalizer::to_vec(&result.replacement_messages)
        .expect("canonical")
        .into();
    check_compaction_conformance(&CompactionConformanceCase {
        descriptor: middleware.descriptor(),
        input,
        result: *result,
        shared_projections: Arc::from([SharedCompactionProjection {
            target: Arc::from("rust"),
            bytes: projection,
        }]),
    })
    .expect("conformance");
}

#[tokio::test]
async fn large_tool_output_keeps_pairs_and_shortens_bodies() {
    let mut config = CompactionConfig::sliding_window(8, 0);
    config.strategy = CompactionStrategy::LargeToolOutput;
    config.large_tool_output_bytes = 16;
    let middleware = CompactionMiddleware::try_new(config).expect("middleware");
    let input = history();
    let outcome = invoke(&middleware, input.clone(), None)
        .await
        .expect("invoke");
    let StageOutcome::CompactContext(result) = outcome else {
        panic!("expected compact context");
    };
    assert_eq!(
        result.replacement_messages.len(),
        input.source_entries.len()
    );
    let tool = result
        .replacement_messages
        .iter()
        .find(|message| message.role() == MessageRole::Tool)
        .expect("tool");
    let original = input.source_entries[3].message.content()[0].clone();
    let ContentBlock::ToolResult(original) = original else {
        panic!("tool result");
    };
    let ContentBlock::ToolResult(truncated) = &tool.content()[0] else {
        panic!("truncated");
    };
    assert_eq!(truncated.tool_call_id(), original.tool_call_id());
    assert!(
        block_len(&tool.content()[0]) < block_len(&input.source_entries[3].message.content()[0])
    );
}

#[tokio::test]
async fn summarize_requests_child_model_and_resume_does_not_recurse() {
    let mut config = CompactionConfig::sliding_window(8, 0);
    config.strategy = CompactionStrategy::Summarize;
    config.secondary_model_authorized = true;
    config.summarize_model = Some(ComponentRef::new(
        ComponentId::parse("finstack.model.summarize").expect("id"),
        Some(Version {
            major: 0,
            minor: 0,
            patch: 4,
        }),
    ));
    config.budget_scope_id = Some(id(9));
    let middleware = CompactionMiddleware::try_new(config).expect("middleware");
    let input = history();
    let first = invoke(&middleware, input.clone(), None)
        .await
        .expect("first");
    assert!(matches!(first, StageOutcome::RequestCompactionModel(_)));

    let resume = CompactionModelResume {
        request_id: id(70),
        effect_id: id(71),
        result: ModelResponse {
            assistant_content: text("derived summary").into(),
            tool_calls: Arc::from([]),
            usage: Usage::empty(),
            provider_ids: ProviderIds::empty(),
            completion_id: Arc::from("summary-1"),
            continuation_state: None,
        },
        resume_state: RawJson::parse(b"{\"depth\":1}").expect("resume"),
    };
    let second = invoke(&middleware, input, Some(resume))
        .await
        .expect("resume");
    assert!(matches!(second, StageOutcome::CompactContext(_)));
}

#[tokio::test]
async fn unauthorized_secondary_model_fails_before_dispatch() {
    let mut config = CompactionConfig::sliding_window(8, 0);
    config.strategy = CompactionStrategy::Summarize;
    config.secondary_model_authorized = false;
    let middleware = CompactionMiddleware::try_new(config).expect("middleware");
    let error = invoke(&middleware, history(), None)
        .await
        .expect_err("denied");
    assert_eq!(error.code(), COMPACTION_MODEL_NOT_AUTHORIZED);
}

#[tokio::test]
async fn cancelled_invoke_fails_closed() {
    let middleware =
        CompactionMiddleware::try_new(CompactionConfig::sliding_window(8, 0)).expect("middleware");
    let ctx = middleware_ctx(None);
    ctx.run.cancellation.cancel();
    let error = middleware
        .invoke(ctx, StageInput::BeforeModel(Box::new(history())))
        .await
        .expect_err("cancelled");
    assert_eq!(error.code(), "compaction_cancelled");
}

#[test]
fn incompatible_checkpoint_is_a_cache_miss() {
    let input = history();
    let mut checkpoint = finstack_ai_runtime::CompactionCheckpoint {
        component_id: ComponentId::parse("finstack.middleware.compaction").expect("id"),
        strategy_id: Arc::from(SLIDING_WINDOW),
        strategy_version: 1,
        configuration_digest: Digest::raw_json(b"other"),
        model_context_profile_digest: input.model_context_profile_digest,
        covered_through_entry_id: input.source_entries.last().expect("last").entry_id,
        source_digest: Digest::raw_json(b"other-source"),
        summary: CompactedSummary::Inline(Arc::from([])),
        summary_digest: Digest::raw_json(b"summary"),
        sensitivity: Sensitivity::Internal,
    };
    assert!(
        !compaction_checkpoint_compatible(
            &CompactionMiddleware::try_new(CompactionConfig::sliding_window(8, 0))
                .expect("mw")
                .descriptor(),
            &input,
            &checkpoint,
        )
        .expect("check")
    );
    checkpoint.configuration_digest = Digest::raw_json(b"still-other");
    assert!(
        !compaction_checkpoint_compatible(
            &CompactionMiddleware::try_new(CompactionConfig::sliding_window(8, 0))
                .expect("mw")
                .descriptor(),
            &input,
            &checkpoint,
        )
        .expect("check")
    );
}
