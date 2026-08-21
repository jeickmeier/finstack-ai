//! compaction contract public adversarial compaction conformance proof.

use std::sync::Arc;

use finstack_ai_kernel::Stage;
use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, EntryTag, Id, IdTag,
    InvocationRecovery, Message, MessageRole, Metadata, OutputSpec, ProviderIds, RawJson,
    Sensitivity, TextBlock, Timestamp, ToolCallBlock, ToolCallTag, ToolResultBlock, Version,
};
use finstack_ai_runtime::{
    BeforeModelInput, CompactedSummary, CompactionCheckpoint, CompactionEvidence, CompactionResult,
    CompactionSourceEntry, ContextAuthority, ContextItem, ContextItemKind, ContextProvenance,
    MiddlewareDescriptor, MiddlewareOrder, MiddlewareRole, ModelName, ModelRequestDraft,
    ModelRequestLimits, ModelSettings, OrderTier, PromptCacheImpact, StageMask,
    compaction_projection_digest, compaction_protected_set_digest, compaction_source_digest,
    compaction_summary_digest,
};
use finstack_ai_test::{
    CompactionConformanceCase, SharedCompactionProjection, check_compaction_conformance,
};

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
        Timestamp::from_unix_ms(i64::try_from(ordinal).expect("timestamp")).expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn descriptor() -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: ComponentInvocation {
            component: ComponentId::parse("fixture.compactor").expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(br#"{"window":2}"#),
            recovery: InvocationRecovery::RecomputeSafe,
        },
        stages: StageMask::from_stages([Stage::BeforeModel]),
        order: MiddlewareOrder {
            tier: OrderTier::ContextCompaction,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        role: MiddlewareRole::ContextCompactor {
            strategy_id: Arc::from("fixture.window"),
            strategy_version: 1,
        },
        metadata: Metadata::empty(),
    }
}

fn input() -> BeforeModelInput {
    let call_id = id::<ToolCallTag>(50);
    let entries: Arc<[CompactionSourceEntry]> = Arc::from([
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(20),
            message: message(
                10,
                MessageRole::System,
                vec![ContentBlock::Text(
                    TextBlock::try_new("system policy").expect("text"),
                )],
            ),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"system"),
            protected: true,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(21),
            message: message(
                11,
                MessageRole::Assistant,
                vec![ContentBlock::ToolCall(
                    ToolCallBlock::try_new(
                        call_id,
                        "fixture.lookup",
                        RawJson::parse(b"{}").expect("arguments"),
                    )
                    .expect("call"),
                )],
            ),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"assistant"),
            protected: false,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(22),
            message: message(
                12,
                MessageRole::Tool,
                vec![ContentBlock::ToolResult(
                    ToolResultBlock::try_new(
                        call_id,
                        vec![ContentBlock::Text(
                            TextBlock::try_new("large output").expect("text"),
                        )],
                        false,
                    )
                    .expect("result"),
                )],
            ),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"tool"),
            protected: false,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(23),
            message: message(
                13,
                MessageRole::User,
                vec![ContentBlock::Text(
                    TextBlock::try_new("current request").expect("text"),
                )],
            ),
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"user"),
            protected: true,
        },
    ]);
    BeforeModelInput {
        request: ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
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
        hard_input_tokens: 1_000,
        checkpoint: None,
    }
}

fn result(descriptor: &MiddlewareDescriptor, input: &BeforeModelInput) -> CompactionResult {
    let replacement_messages: Arc<[Message]> = input
        .source_entries
        .iter()
        .map(|entry| entry.message.clone())
        .collect::<Vec<_>>()
        .into();
    let summary = ContextItem::try_new(
        ContextItemKind::DerivedSummary,
        vec![ContentBlock::Text(
            TextBlock::try_new("lookup completed").expect("text"),
        )],
        ContextProvenance {
            source_id: Arc::from("fixture.window"),
            source_ref: None,
            external: false,
        },
        ContextAuthority::Untrusted,
        0,
        4,
        Sensitivity::Internal,
        false,
    )
    .expect("summary");
    let summaries: Arc<[ContextItem]> = Arc::from([summary]);
    let checkpoint_summary = CompactedSummary::Inline(Arc::clone(&summaries));
    let summary_digest = compaction_summary_digest(&checkpoint_summary).expect("summary digest");
    let source_digest = compaction_source_digest(&input.source_entries).expect("source digest");
    CompactionResult {
        replacement_messages: Arc::clone(&replacement_messages),
        derived_summaries: summaries,
        evidence: CompactionEvidence {
            strategy_id: Arc::from("fixture.window"),
            strategy_version: 1,
            configuration_digest: descriptor.invocation.configuration_digest,
            model_context_profile_digest: input.model_context_profile_digest,
            source_digest,
            protected_item_set_digest: compaction_protected_set_digest(&[
                input.source_entries[0].entry_id,
                input.source_entries[3].entry_id,
            ])
            .expect("protected digest"),
            covered_entry_ids: input
                .source_entries
                .iter()
                .map(|entry| entry.entry_id)
                .collect::<Vec<_>>()
                .into(),
            retained_entry_ids: input
                .source_entries
                .iter()
                .map(|entry| entry.entry_id)
                .collect::<Vec<_>>()
                .into(),
            projection_digest: compaction_projection_digest(&replacement_messages)
                .expect("projection digest"),
            estimated_tokens_before: 900,
            estimated_tokens_after: 800,
            summary_digest: Some(summary_digest),
            cache_impact: PromptCacheImpact::StablePrefixPreserved,
        },
        checkpoint: Some(CompactionCheckpoint {
            component_id: descriptor.invocation.component.clone(),
            strategy_id: Arc::from("fixture.window"),
            strategy_version: 1,
            configuration_digest: descriptor.invocation.configuration_digest,
            model_context_profile_digest: input.model_context_profile_digest,
            covered_through_entry_id: input.source_entries[3].entry_id,
            source_digest,
            summary: checkpoint_summary,
            summary_digest,
            sensitivity: Sensitivity::Internal,
        }),
    }
}

#[test]
fn public_compaction_suite_proves_the_full_integrity_matrix() {
    let descriptor = descriptor();
    let input = input();
    let result = result(&descriptor, &input);
    let projection: Arc<[u8]> = serde_json_canonicalizer::to_vec(&result.replacement_messages)
        .expect("canonical projection")
        .into();
    let report = check_compaction_conformance(&CompactionConformanceCase {
        descriptor,
        input,
        result,
        shared_projections: Arc::from([
            SharedCompactionProjection {
                target: Arc::from("rust"),
                bytes: Arc::clone(&projection),
            },
            SharedCompactionProjection {
                target: Arc::from("python-fixture"),
                bytes: Arc::clone(&projection),
            },
            SharedCompactionProjection {
                target: Arc::from("wasm-fixture"),
                bytes: projection,
            },
        ]),
    })
    .expect("compaction conformance");
    assert_eq!(report.compared_projections, 3);
    assert!(!report.canonical_history.is_empty());
    assert!(!report.projection_bytes.is_empty());
}
