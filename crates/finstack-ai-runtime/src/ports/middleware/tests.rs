use std::sync::Arc;

use super::*;
use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ComponentRef, ContentBlock, Digest, EffectId, EffectInput,
    EffectKind, EffectOutputContract, EffectOutputKind, EffectPurpose, EffectRelation,
    EffectRequested, EffectTag, EntryTag, EventTag, Id, IdTag, InteractionKind, InteractionRequest,
    InteractionTag, InvocationRecovery, LaneTag, Message, MessageRole, Metadata, OutputSpec,
    PipelinePosition, ProviderIds, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RawJson, RecordBody,
    RecordEnvelope, RecordTag, RetrySafety, RunTag, Sensitivity, SessionTag, Stage, TextBlock,
    Timestamp, ToolCallBlock, ToolCallTag, ToolResultBlock, Version,
};

use crate::{
    ContextAuthority, ContextItemKind, ContextProvenance, ModelName, ModelRequestDraft,
    ModelRequestLimits, ModelSettings, PortFuture,
};

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

struct Stub {
    descriptor: MiddlewareDescriptor,
}

impl Middleware for Stub {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        Box::pin(async { Ok(StageOutcome::Continue) })
    }
}

fn descriptor(id: &str, before: &[&str], after: &[&str]) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: ComponentInvocation {
            component: ComponentId::parse(id).expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery: InvocationRecovery::RecomputeSafe,
        },
        stages: StageMask::from_stages([Stage::BeforeRun]),
        order: MiddlewareOrder {
            tier: OrderTier::Standard,
            priority: 0,
            before: before
                .iter()
                .map(|value| ComponentId::parse(*value).expect("before"))
                .collect::<Vec<_>>()
                .into(),
            after: after
                .iter()
                .map(|value| ComponentId::parse(*value).expect("after"))
                .collect::<Vec<_>>()
                .into(),
        },
        role: MiddlewareRole::Standard,
        metadata: Metadata::empty(),
    }
}

fn compactor_descriptor(id: &str) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: ComponentInvocation {
            component: ComponentId::parse(id).expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{\"window\":2}"),
            recovery: InvocationRecovery::Reconcile,
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

fn model_draft(messages: Arc<[Message]>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: ModelName::try_new("fixture-model").expect("model"),
        messages,
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
    }
}

fn compaction_input() -> BeforeModelInput {
    let call_id = id::<ToolCallTag>(50);
    let system = message(
        10,
        MessageRole::System,
        vec![ContentBlock::Text(
            TextBlock::try_new("system policy").expect("text"),
        )],
    );
    let assistant = message(
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
    );
    let tool = message(
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
    );
    let user = message(
        13,
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("current request").expect("text"),
        )],
    );
    let entries: Arc<[CompactionSourceEntry]> = Arc::from([
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(20),
            message: system,
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"system"),
            protected: true,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(21),
            message: assistant,
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"assistant"),
            protected: false,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(22),
            message: tool,
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"tool"),
            protected: false,
        },
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(23),
            message: user,
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"user"),
            protected: true,
        },
    ]);
    BeforeModelInput {
        request: model_draft(
            entries
                .iter()
                .map(|entry| entry.message.clone())
                .collect::<Vec<_>>()
                .into(),
        ),
        source_entries: entries,
        model_context_profile_digest: Digest::raw_json(b"profile"),
        hard_input_tokens: 1_000,
        checkpoint: None,
    }
}

fn valid_compaction(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
) -> CompactionResult {
    let replacement_messages: Arc<[Message]> = Arc::from([
        input.source_entries[0].message.clone(),
        input.source_entries[3].message.clone(),
    ]);
    let summary = crate::ContextItem::try_new(
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
    let derived_summaries: Arc<[crate::ContextItem]> = Arc::from([summary]);
    let checkpoint_summary = CompactedSummary::Inline(Arc::clone(&derived_summaries));
    let summary_digest = compaction_summary_digest(&checkpoint_summary).expect("summary digest");
    CompactionResult {
        replacement_messages: Arc::clone(&replacement_messages),
        derived_summaries,
        evidence: CompactionEvidence {
            strategy_id: Arc::from("fixture.window"),
            strategy_version: 1,
            configuration_digest: descriptor.invocation.configuration_digest,
            model_context_profile_digest: input.model_context_profile_digest,
            source_digest: compaction_source_digest(&input.source_entries).expect("source"),
            protected_item_set_digest: compaction_protected_set_digest(&[
                input.source_entries[0].entry_id,
                input.source_entries[3].entry_id,
            ])
            .expect("protected"),
            covered_entry_ids: input
                .source_entries
                .iter()
                .map(|entry| entry.entry_id)
                .collect::<Vec<_>>()
                .into(),
            retained_entry_ids: Arc::from([
                input.source_entries[0].entry_id,
                input.source_entries[3].entry_id,
            ]),
            projection_digest: compaction_projection_digest(&replacement_messages)
                .expect("projection"),
            estimated_tokens_before: 800,
            estimated_tokens_after: 200,
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
            source_digest: compaction_source_digest(&input.source_entries).expect("source"),
            summary: checkpoint_summary,
            summary_digest,
            sensitivity: Sensitivity::Internal,
        }),
    }
}

#[test]
fn cycles_fail_during_resolution() {
    let left: Arc<dyn Middleware> = Arc::new(Stub {
        descriptor: descriptor("fixture.left", &["fixture.right"], &[]),
    });
    let right: Arc<dyn Middleware> = Arc::new(Stub {
        descriptor: descriptor("fixture.right", &["fixture.left"], &[]),
    });
    let error = ResolvedMiddlewareChain::try_new(vec![
        MiddlewareRegistration { middleware: left },
        MiddlewareRegistration { middleware: right },
    ])
    .err()
    .expect("cycle");
    assert_eq!(error.code(), MIDDLEWARE_ORDER_CYCLE);
}

#[test]
fn missing_requirements_duplicate_compactors_and_post_compactor_mutation_fail_resolution() {
    let missing: Arc<dyn Middleware> = Arc::new(Stub {
        descriptor: descriptor("fixture.missing", &["fixture.absent"], &[]),
    });
    assert_eq!(
        ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration {
            middleware: missing,
        }])
        .err()
        .expect("missing")
        .code(),
        MIDDLEWARE_RESOLUTION_INVALID
    );

    let first: Arc<dyn Middleware> = Arc::new(Stub {
        descriptor: compactor_descriptor("fixture.compactor-one"),
    });
    let second: Arc<dyn Middleware> = Arc::new(Stub {
        descriptor: compactor_descriptor("fixture.compactor-two"),
    });
    assert_eq!(
        ResolvedMiddlewareChain::try_new(vec![
            MiddlewareRegistration { middleware: first },
            MiddlewareRegistration { middleware: second },
        ])
        .err()
        .expect("duplicate compactor")
        .code(),
        MIDDLEWARE_RESOLUTION_INVALID
    );

    let compactor: Arc<dyn Middleware> = Arc::new(Stub {
        descriptor: compactor_descriptor("fixture.compactor"),
    });
    let mut mutator_descriptor = descriptor("fixture.mutator", &[], &["fixture.compactor"]);
    mutator_descriptor.stages = StageMask::from_stages([Stage::BeforeModel]);
    mutator_descriptor.order.tier = OrderTier::ContextMutation;
    let mutator: Arc<dyn Middleware> = Arc::new(Stub {
        descriptor: mutator_descriptor,
    });
    assert_eq!(
        ResolvedMiddlewareChain::try_new(vec![
            MiddlewareRegistration {
                middleware: compactor,
            },
            MiddlewareRegistration {
                middleware: mutator,
            },
        ])
        .err()
        .expect("post-compactor mutation")
        .code(),
        MIDDLEWARE_ORDER_CYCLE
    );
}

#[test]
fn before_finalize_accepts_interaction_but_rejects_replacement() {
    let descriptor = MiddlewareDescriptor {
        stages: StageMask::from_stages([Stage::BeforeFinalize]),
        ..descriptor("fixture.finalize", &[], &[])
    };
    let input = StageInput::BeforeFinalize {
        candidate: RawJson::parse(b"{\"answer\":42}").expect("candidate"),
        result_message: None,
    };
    assert_eq!(
        validate_stage_outcome(
            &descriptor,
            &input,
            &StageOutcome::Replace(RawJson::parse(b"{}").expect("replacement")),
        )
        .expect_err("replacement")
        .code(),
        MIDDLEWARE_OUTCOME_NOT_ALLOWED
    );
    let interaction = InteractionRequest::try_new(
        1,
        id::<InteractionTag>(70),
        id::<EffectTag>(71),
        InteractionKind::Approval,
        vec![ContentBlock::Text(
            TextBlock::try_new("approve completion").expect("prompt"),
        )],
        RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
        )
        .expect("schema"),
        ComponentRef::new(
            descriptor.invocation.component.clone(),
            Some(descriptor.invocation.version),
        ),
        descriptor.invocation.version,
        None,
        None,
        false,
        Metadata::empty(),
    )
    .expect("interaction");
    validate_stage_outcome(
        &descriptor,
        &input,
        &StageOutcome::RequestInteraction(Box::new(interaction)),
    )
    .expect("interaction allowed");
}

#[test]
fn compaction_preserves_protected_bytes_tool_pair_atomicity_and_canonical_history() {
    let descriptor = compactor_descriptor("fixture.compactor");
    let input = compaction_input();
    let canonical_before = input.source_entries.clone();
    let valid = valid_compaction(&descriptor, &input);
    validate_compaction_result(&descriptor, &input, &valid).expect("valid compaction");
    assert!(
        compaction_checkpoint_compatible(
            &descriptor,
            &input,
            valid.checkpoint.as_ref().expect("checkpoint")
        )
        .expect("checkpoint compatibility")
    );
    assert_eq!(input.source_entries, canonical_before);

    let mut stale_checkpoint = valid.checkpoint.clone().expect("checkpoint");
    stale_checkpoint.configuration_digest = Digest::raw_json(b"stale-config");
    assert!(
        !compaction_checkpoint_compatible(&descriptor, &input, &stale_checkpoint)
            .expect("stale checkpoint is an ordinary cache miss")
    );

    let mut missing_user = valid.clone();
    missing_user.replacement_messages = Arc::from([input.source_entries[0].message.clone()]);
    missing_user.evidence.retained_entry_ids = Arc::from([input.source_entries[0].entry_id]);
    missing_user.evidence.projection_digest =
        compaction_projection_digest(&missing_user.replacement_messages).expect("digest");
    assert_eq!(
        validate_compaction_result(&descriptor, &input, &missing_user)
            .expect_err("protected user")
            .code(),
        COMPACTION_RESULT_INVALID
    );

    let mut reordered = valid.clone();
    reordered.replacement_messages = Arc::from([
        input.source_entries[3].message.clone(),
        input.source_entries[0].message.clone(),
    ]);
    reordered.evidence.retained_entry_ids = Arc::from([
        input.source_entries[3].entry_id,
        input.source_entries[0].entry_id,
    ]);
    reordered.evidence.projection_digest =
        compaction_projection_digest(&reordered.replacement_messages).expect("digest");
    assert_eq!(
        validate_compaction_result(&descriptor, &input, &reordered)
            .expect_err("source reorder")
            .code(),
        COMPACTION_RESULT_INVALID
    );

    let mut split_pair = valid;
    split_pair.replacement_messages = Arc::from([
        input.source_entries[0].message.clone(),
        input.source_entries[1].message.clone(),
        input.source_entries[3].message.clone(),
    ]);
    split_pair.evidence.retained_entry_ids = Arc::from([
        input.source_entries[0].entry_id,
        input.source_entries[1].entry_id,
        input.source_entries[3].entry_id,
    ]);
    split_pair.evidence.projection_digest =
        compaction_projection_digest(&split_pair.replacement_messages).expect("digest");
    assert_eq!(
        validate_compaction_result(&descriptor, &input, &split_pair)
            .expect_err("split pair")
            .code(),
        COMPACTION_RESULT_INVALID
    );
}

fn envelope(sequence: u64, body: RecordBody) -> RecordEnvelope {
    let events = (0..body
        .derived_event_count(RECORD_KIND_VERSION)
        .expect("events"))
        .map(|offset| id::<EventTag>(100 + sequence + u64::try_from(offset).expect("offset")))
        .collect();
    RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(10 + sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        Some(id::<RunTag>(3)),
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        Digest::raw_json(b"{}"),
        None,
        Digest::raw_json(b"{}"),
        events,
        body,
    )
    .expect("envelope")
}

fn middleware_effect(
    effect_id: EffectId,
    descriptor: &MiddlewareDescriptor,
    input: &StageInput,
) -> EffectRequested {
    EffectRequested::try_new(
        effect_id,
        EffectKind::Middleware,
        None,
        Some(descriptor.invocation.clone()),
        Some(
            PipelinePosition::try_new(
                Digest::raw_json(b"middleware-chain"),
                stage_name(input.stage()),
                0,
            )
            .expect("pipeline"),
        ),
        EffectOutputContract {
            kind: EffectOutputKind::MiddlewareOutcome,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"middleware-outcome-v1"),
        },
        EffectInput::Middleware {
            stage: Arc::from(stage_name(input.stage())),
            input: input.to_raw_json().expect("input"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("effect")
}

#[test]
fn compaction_child_model_requires_exact_committed_relation_and_input() {
    let descriptor = compactor_descriptor("fixture.compactor");
    let input = StageInput::BeforeModel(Box::new(compaction_input()));
    let parent_id = id::<EffectTag>(80);
    let parent = middleware_effect(parent_id, &descriptor, &input);
    let model_component = ComponentRef::new(
        ComponentId::parse("fixture.summary-model").expect("model component"),
        Some(Version {
            major: 1,
            minor: 0,
            patch: 0,
        }),
    );
    let request = CompactionModelRequest {
        model: model_component.clone(),
        request: model_draft(Arc::from([])),
        budget_scope_id: id(81),
        source_sensitivity: Sensitivity::Internal,
        residency_policy_digest: Digest::raw_json(b"residency"),
        resume_state: RawJson::parse(b"{}").expect("resume"),
    };
    let child = EffectRequested::try_new(
        id::<EffectTag>(82),
        EffectKind::Model,
        Some(EffectRelation {
            parent_effect_id: parent_id,
            purpose: EffectPurpose::CompactionSummary {
                middleware_component_id: descriptor.invocation.component.clone(),
            },
        }),
        Some(ComponentInvocation {
            component: model_component.id().clone(),
            version: model_component.version().expect("version"),
            configuration_digest: Digest::raw_json(b"model-config"),
            recovery: InvocationRecovery::Reconcile,
        }),
        None,
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"model-response-v1"),
        },
        EffectInput::Model {
            request: RawJson::parse(request.request.canonical_bytes().expect("request"))
                .expect("raw"),
        },
        RetrySafety::IdempotentWithKey,
        None,
    )
    .expect("child");
    let child_envelope = envelope(2, RecordBody::EffectRequested(child.clone()));
    validate_compaction_model_effect(&parent, &child_envelope, &request).expect("related child");

    let wrong_relation = EffectRequested::try_new(
        id::<EffectTag>(83),
        child.kind(),
        Some(EffectRelation {
            parent_effect_id: id::<EffectTag>(84),
            purpose: EffectPurpose::CompactionSummary {
                middleware_component_id: descriptor.invocation.component.clone(),
            },
        }),
        child.component().cloned(),
        child.pipeline().cloned(),
        child.output_contract().clone(),
        child.input().clone(),
        child.retry_safety(),
        child.deadline(),
    )
    .expect("wrong relation fixture");
    assert_eq!(
        validate_compaction_model_effect(
            &parent,
            &envelope(3, RecordBody::EffectRequested(wrong_relation)),
            &request,
        )
        .expect_err("wrong parent relation")
        .code(),
        COMPACTION_MODEL_NOT_AUTHORIZED
    );

    let mut wrong = request;
    wrong.resume_state = RawJson::parse(b"{\"different\":true}").expect("state");
    // Resume state belongs to the parent cursor and does not change child provider input.
    validate_compaction_model_effect(&parent, &child_envelope, &wrong)
        .expect("opaque parent resume state");
}

#[test]
fn before_tool_batch_input_serializes_with_stage_tag_calls_and_tools() {
    let input = StageInput::BeforeToolBatch(Box::new(BeforeToolBatchInput {
        calls: Arc::from([]),
        tools: Arc::from([]),
    }));
    let json = serde_json::to_value(&input).expect("serialize");
    assert_eq!(json["stage"], "before_tool_batch");
    assert!(json["calls"].is_array());
    assert!(json["tools"].is_array());
    assert_eq!(input.stage(), Stage::BeforeToolBatch);

    let round_tripped: StageInput =
        serde_json::from_value(json).expect("deserialize before_tool_batch");
    assert_eq!(
        round_tripped, input,
        "the internally-tagged BeforeToolBatch variant must round-trip through serde_json"
    );
}
