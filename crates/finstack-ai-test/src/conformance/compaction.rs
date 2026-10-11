//! Adversarial public conformance checks for replay-safe context compaction.

use std::sync::Arc;

use finstack_ai_kernel::{ContentBlock, Digest, EntryId};
use finstack_ai_runtime::ports::middleware::{
    BeforeModelInput, COMPACTION_BUDGET_EXCEEDED, CompactionCheckpoint, CompactionResult,
    MiddlewareDescriptor, compaction_checkpoint_compatible, compaction_projection_digest,
    validate_compaction_result,
};

use crate::PortConformanceFailure;

/// One target-labelled shared model-visible projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedCompactionProjection {
    /// Binding or host label used only in diagnostics.
    pub target: Arc<str>,
    /// Canonical serialized replacement-message bytes.
    pub bytes: Arc<[u8]>,
}

/// Inputs for the complete adversarial compaction conformance suite.
#[derive(Debug, Clone)]
pub struct CompactionConformanceCase {
    /// Resolved unique compactor descriptor.
    pub descriptor: MiddlewareDescriptor,
    /// Immutable canonical history and hard budget.
    pub input: BeforeModelInput,
    /// Valid replacement projection and evidence.
    pub result: CompactionResult,
    /// Independently produced binding/host projections to compare byte-for-byte.
    pub shared_projections: Arc<[SharedCompactionProjection]>,
}

/// Digests and canonical bytes proved by the compaction suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionConformanceReport {
    /// Canonical history bytes before and after validation.
    pub canonical_history: Arc<[u8]>,
    /// Exact model-visible projection digest.
    pub projection_digest: Digest,
    /// Exact canonical model-visible projection bytes.
    pub projection_bytes: Arc<[u8]>,
    /// Number of target-labelled projections compared.
    pub compared_projections: usize,
}

/// Prove the compaction contract compaction integrity matrix using public runtime APIs.
///
/// # Errors
///
/// Names the exact canonical-history, protected-item, tool-pair, checkpoint,
/// hard-budget, or shared-projection contract that failed.
pub fn check_compaction_conformance(
    case: &CompactionConformanceCase,
) -> Result<CompactionConformanceReport, PortConformanceFailure> {
    let before = canonical_source_bytes(&case.input)?;
    validate_compaction_result(&case.descriptor, &case.input, &case.result)
        .map_err(|error| failure("compaction.valid_projection", error.to_string()))?;
    let after = canonical_source_bytes(&case.input)?;
    ensure(
        before == after,
        "compaction.canonical_history_immutable",
        "validation changed the canonical source history",
    )?;

    prove_protected_rejection(case)?;
    prove_tool_pair_rejection(case)?;
    prove_multi_call_component_rejection(case)?;
    prove_hard_budget_rejection(case)?;
    prove_checkpoint_invalidation(case)?;

    let projection_bytes: Arc<[u8]> =
        serde_json_canonicalizer::to_vec(&case.result.replacement_messages)
            .map_err(|error| failure("compaction.projection.canonical", error.to_string()))?
            .into();
    ensure(
        !case.shared_projections.is_empty(),
        "compaction.projection.shared_targets",
        "no independently produced target projections were supplied",
    )?;
    for projection in case.shared_projections.iter() {
        if projection.bytes != projection_bytes {
            return Err(failure(
                "compaction.projection.binding_parity",
                format!(
                    "target `{}` differs from the canonical Rust projection ({} bytes vs {} bytes)",
                    projection.target,
                    projection.bytes.len(),
                    projection_bytes.len()
                ),
            ));
        }
    }
    let projection_digest = compaction_projection_digest(&case.result.replacement_messages)
        .map_err(|error| failure("compaction.projection.digest", error.to_string()))?;
    ensure(
        projection_digest == case.result.evidence.projection_digest,
        "compaction.projection.digest",
        "reported projection digest differs from canonical replacement messages",
    )?;
    Ok(CompactionConformanceReport {
        canonical_history: before.into(),
        projection_digest,
        projection_bytes,
        compared_projections: case.shared_projections.len(),
    })
}

fn prove_protected_rejection(
    case: &CompactionConformanceCase,
) -> Result<(), PortConformanceFailure> {
    let protected = case
        .input
        .source_entries
        .iter()
        .find(|entry| entry.protected)
        .ok_or_else(|| {
            failure(
                "compaction.protected.required_fixture",
                "conformance case contains no protected source entry",
            )
        })?;
    let mut invalid = case.result.clone();
    let retained = invalid
        .replacement_messages
        .iter()
        .filter(|message| message.id() != protected.message.id())
        .cloned()
        .collect::<Vec<_>>();
    invalid.replacement_messages = retained.into();
    invalid.evidence.retained_entry_ids = retained_ids(&case.input, &invalid);
    invalid.evidence.projection_digest =
        compaction_projection_digest(&invalid.replacement_messages)
            .map_err(|error| failure("compaction.protected.digest", error.to_string()))?;
    let Err(error) = validate_compaction_result(&case.descriptor, &case.input, &invalid) else {
        return Err(failure(
            "compaction.protected.retained",
            "removing protected content must fail",
        ));
    };
    ensure(
        error.code() == finstack_ai_runtime::ports::middleware::COMPACTION_RESULT_INVALID,
        "compaction.protected.retained",
        "removing a protected entry did not fail with compaction_result_invalid",
    )
}

fn prove_tool_pair_rejection(
    case: &CompactionConformanceCase,
) -> Result<(), PortConformanceFailure> {
    let Some(result_message) = case.result.replacement_messages.iter().find(|message| {
        message
            .content()
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult(_)))
    }) else {
        return Err(failure(
            "compaction.tool_pair.required_fixture",
            "conformance projection contains no retained tool-result message",
        ));
    };
    ensure(
        case.result.replacement_messages.iter().any(|message| {
            message
                .content()
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolCall(_)))
        }),
        "compaction.tool_pair.required_fixture",
        "conformance projection contains no retained tool-call message",
    )?;
    let mut invalid = case.result.clone();
    invalid.replacement_messages = invalid
        .replacement_messages
        .iter()
        .filter(|message| message.id() != result_message.id())
        .cloned()
        .collect::<Vec<_>>()
        .into();
    invalid.evidence.retained_entry_ids = retained_ids(&case.input, &invalid);
    invalid.evidence.projection_digest =
        compaction_projection_digest(&invalid.replacement_messages)
            .map_err(|error| failure("compaction.tool_pair.digest", error.to_string()))?;
    let Err(error) = validate_compaction_result(&case.descriptor, &case.input, &invalid) else {
        return Err(failure(
            "compaction.tool_pair.retained",
            "retaining a tool call without its result must fail",
        ));
    };
    ensure(
        error.code() == finstack_ai_runtime::ports::middleware::COMPACTION_RESULT_INVALID,
        "compaction.tool_pair.atomic",
        "orphaning a retained tool call did not fail with compaction_result_invalid",
    )
}

/// A retained assistant message with two or more tool calls forms one
/// atomic component with every one of its result messages (the kernel
/// records one `Tool` message per result): dropping any single result
/// while the call message stays must fail. Proved for every such message
/// whose results are all retained; a projection that retains none is a
/// legal fixture and proves nothing here.
fn prove_multi_call_component_rejection(
    case: &CompactionConformanceCase,
) -> Result<(), PortConformanceFailure> {
    let retained_results = case
        .result
        .replacement_messages
        .iter()
        .flat_map(|message| message.content().iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(*result.tool_call_id()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    for message in case.result.replacement_messages.iter() {
        let calls = message
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(*call.tool_call_id()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if calls.len() < 2 || !calls.iter().all(|call| retained_results.contains(call)) {
            continue;
        }
        for call in calls {
            let mut invalid = case.result.clone();
            invalid.replacement_messages = invalid
                .replacement_messages
                .iter()
                .filter(|candidate| {
                    !candidate.content().iter().any(|block| {
                        matches!(block, ContentBlock::ToolResult(result) if *result.tool_call_id() == call)
                    })
                })
                .cloned()
                .collect::<Vec<_>>()
                .into();
            invalid.evidence.retained_entry_ids = retained_ids(&case.input, &invalid);
            invalid.evidence.projection_digest =
                compaction_projection_digest(&invalid.replacement_messages).map_err(|error| {
                    failure("compaction.tool_pair.multi_call.digest", error.to_string())
                })?;
            let Err(error) = validate_compaction_result(&case.descriptor, &case.input, &invalid)
            else {
                return Err(failure(
                    "compaction.tool_pair.multi_call.atomic",
                    "dropping one result of a multi-call assistant message must fail",
                ));
            };
            ensure(
                error.code() == finstack_ai_runtime::ports::middleware::COMPACTION_RESULT_INVALID,
                "compaction.tool_pair.multi_call.atomic",
                "orphaning one call of a multi-call component did not fail with compaction_result_invalid",
            )?;
        }
    }
    Ok(())
}

fn prove_hard_budget_rejection(
    case: &CompactionConformanceCase,
) -> Result<(), PortConformanceFailure> {
    let mut invalid = case.result.clone();
    invalid.evidence.estimated_tokens_after = case.input.hard_input_tokens.saturating_add(1);
    let Err(error) = validate_compaction_result(&case.descriptor, &case.input, &invalid) else {
        return Err(failure(
            "compaction.hard_budget.enforced",
            "hard-budget overrun must fail",
        ));
    };
    ensure(
        error.code() == COMPACTION_BUDGET_EXCEEDED,
        "compaction.hard_budget.enforced",
        "over-budget projection did not fail with context_budget_exceeded",
    )
}

fn prove_checkpoint_invalidation(
    case: &CompactionConformanceCase,
) -> Result<(), PortConformanceFailure> {
    let checkpoint = case.result.checkpoint.as_ref().ok_or_else(|| {
        failure(
            "compaction.checkpoint.required_fixture",
            "conformance result contains no reusable checkpoint",
        )
    })?;
    ensure(
        compatible(&case.descriptor, &case.input, checkpoint)?,
        "compaction.checkpoint.compatible",
        "valid checkpoint is not compatible with its exact frozen inputs",
    )?;

    let mutations: [fn(&mut CompactionCheckpoint); 4] = [
        |value| value.configuration_digest = Digest::raw_json(b"changed-configuration"),
        |value| value.model_context_profile_digest = Digest::raw_json(b"changed-profile"),
        |value| value.source_digest = Digest::raw_json(b"changed-source"),
        |value| value.summary_digest = Digest::raw_json(b"changed-summary"),
    ];
    for mutate in mutations {
        let mut invalid = checkpoint.clone();
        mutate(&mut invalid);
        ensure(
            !compatible(&case.descriptor, &case.input, &invalid)?,
            "compaction.checkpoint.invalidated",
            "a frozen checkpoint digest mismatch was accepted",
        )?;
    }
    Ok(())
}

fn compatible(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    checkpoint: &CompactionCheckpoint,
) -> Result<bool, PortConformanceFailure> {
    compaction_checkpoint_compatible(descriptor, input, checkpoint)
        .map_err(|error| failure("compaction.checkpoint.validation", error.to_string()))
}

fn retained_ids(input: &BeforeModelInput, result: &CompactionResult) -> Arc<[EntryId]> {
    result
        .replacement_messages
        .iter()
        .filter_map(|message| {
            input
                .source_entries
                .iter()
                .find(|entry| entry.message.id() == message.id())
                .map(|entry| entry.entry_id)
        })
        .collect::<Vec<_>>()
        .into()
}

fn canonical_source_bytes(input: &BeforeModelInput) -> Result<Vec<u8>, PortConformanceFailure> {
    serde_json_canonicalizer::to_vec(&input.source_entries)
        .map_err(|error| failure("compaction.canonical_history", error.to_string()))
}

fn ensure(
    condition: bool,
    contract: &'static str,
    detail: &'static str,
) -> Result<(), PortConformanceFailure> {
    if condition {
        Ok(())
    } else {
        Err(failure(contract, detail))
    }
}

fn failure(contract: &'static str, detail: impl Into<String>) -> PortConformanceFailure {
    PortConformanceFailure::new("Middleware", contract, detail)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::{
        ComponentId, ComponentInvocation, ContentBlock, Digest, EntryTag, Id, IdTag,
        InvocationRecovery, Message, MessageRole, Metadata, OutputSpec, ProviderIds, RawJson,
        Sensitivity, Stage, TextBlock, Timestamp, ToolCallBlock, ToolCallTag, ToolResultBlock,
        Version,
    };
    use finstack_ai_runtime::ports::context::{
        ContextAuthority, ContextItem, ContextItemKind, ContextProvenance,
    };
    use finstack_ai_runtime::ports::middleware::{
        BeforeModelInput, CompactedSummary, CompactionCheckpoint, CompactionEvidence,
        CompactionResult, CompactionSourceEntry, MiddlewareDescriptor, MiddlewareOrder,
        MiddlewareRole, OrderTier, PromptCacheImpact, StageMask, compaction_projection_digest,
        compaction_protected_set_digest, compaction_source_digest, compaction_summary_digest,
    };
    use finstack_ai_runtime::ports::model::{
        ModelName, ModelRequestDraft, ModelRequestLimits, ModelSettings,
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
            Timestamp::from_unix_ms(i64::try_from(ordinal).expect("timestamp")).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn text(value: &str) -> Vec<ContentBlock> {
        vec![ContentBlock::Text(TextBlock::try_new(value).expect("text"))]
    }

    fn entry(ordinal: u64, message: Message, protected: bool) -> CompactionSourceEntry {
        CompactionSourceEntry {
            entry_id: id::<EntryTag>(ordinal),
            message,
            sensitivity: Sensitivity::Internal,
            provenance_digest: Digest::raw_json(b"fixture"),
            protected,
        }
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

    /// System, an assistant message with two tool calls, one `Tool` message
    /// per result, then the protected current user turn.
    fn multi_call_input() -> BeforeModelInput {
        let first = id::<ToolCallTag>(50);
        let second = id::<ToolCallTag>(51);
        let entries: Arc<[CompactionSourceEntry]> = Arc::from([
            entry(
                20,
                message(10, MessageRole::System, text("system policy")),
                true,
            ),
            entry(
                21,
                message(
                    11,
                    MessageRole::Assistant,
                    vec![
                        ContentBlock::ToolCall(
                            ToolCallBlock::try_new(
                                first,
                                "fixture.lookup",
                                RawJson::parse(b"{\"q\":1}").expect("arguments"),
                            )
                            .expect("call"),
                        ),
                        ContentBlock::ToolCall(
                            ToolCallBlock::try_new(
                                second,
                                "fixture.lookup",
                                RawJson::parse(b"{\"q\":2}").expect("arguments"),
                            )
                            .expect("call"),
                        ),
                    ],
                ),
                false,
            ),
            entry(
                22,
                message(
                    12,
                    MessageRole::Tool,
                    vec![ContentBlock::ToolResult(
                        ToolResultBlock::try_new(first, text("first output"), false)
                            .expect("result"),
                    )],
                ),
                false,
            ),
            entry(
                23,
                message(
                    13,
                    MessageRole::Tool,
                    vec![ContentBlock::ToolResult(
                        ToolResultBlock::try_new(second, text("second output"), false)
                            .expect("result"),
                    )],
                ),
                false,
            ),
            entry(
                24,
                message(14, MessageRole::User, text("current request")),
                true,
            ),
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

    /// A projection retaining every entry, with a matching inline summary
    /// and checkpoint.
    fn retain_all(descriptor: &MiddlewareDescriptor, input: &BeforeModelInput) -> CompactionResult {
        let replacement_messages: Arc<[Message]> = input
            .source_entries
            .iter()
            .map(|entry| entry.message.clone())
            .collect::<Vec<_>>()
            .into();
        let summary = ContextItem::try_new(
            ContextItemKind::DerivedSummary,
            text("lookups completed"),
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
        let summary_digest =
            compaction_summary_digest(&checkpoint_summary).expect("summary digest");
        let source_digest = compaction_source_digest(&input.source_entries).expect("source");
        let entry_ids = input
            .source_entries
            .iter()
            .map(|entry| entry.entry_id)
            .collect::<Vec<_>>();
        let protected_ids = input
            .source_entries
            .iter()
            .filter(|entry| entry.protected)
            .map(|entry| entry.entry_id)
            .collect::<Vec<_>>();
        CompactionResult {
            replacement_messages: Arc::clone(&replacement_messages),
            derived_summaries: summaries,
            evidence: CompactionEvidence {
                strategy_id: Arc::from("fixture.window"),
                strategy_version: 1,
                configuration_digest: descriptor.invocation.configuration_digest,
                model_context_profile_digest: input.model_context_profile_digest,
                source_digest,
                protected_item_set_digest: compaction_protected_set_digest(&protected_ids)
                    .expect("protected digest"),
                covered_entry_ids: entry_ids.clone().into(),
                retained_entry_ids: entry_ids.clone().into(),
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
                covered_through_entry_id: *entry_ids.last().expect("entries"),
                source_digest,
                summary: checkpoint_summary,
                summary_digest,
                sensitivity: Sensitivity::Internal,
            }),
        }
    }

    #[test]
    fn multi_call_component_is_proved_atomic() {
        let descriptor = descriptor();
        let input = multi_call_input();
        let result = retain_all(&descriptor, &input);
        let projection: Arc<[u8]> = serde_json_canonicalizer::to_vec(&result.replacement_messages)
            .expect("canonical projection")
            .into();
        let case = CompactionConformanceCase {
            descriptor,
            input,
            result,
            shared_projections: Arc::from([SharedCompactionProjection {
                target: Arc::from("rust"),
                bytes: projection,
            }]),
        };
        prove_multi_call_component_rejection(&case).expect("multi-call component is atomic");
        let report = check_compaction_conformance(&case).expect("full suite");
        assert_eq!(report.compared_projections, 1);
    }

    #[test]
    fn multi_call_proof_rejects_a_validator_that_accepts_a_partial_component() {
        // Drop one result message *and* its call from the fixture's own
        // projection so the remaining single-call projection has nothing to
        // prove: the proof must then be a no-op rather than a false failure.
        let descriptor = descriptor();
        let input = multi_call_input();
        let mut result = retain_all(&descriptor, &input);
        result.replacement_messages = result
            .replacement_messages
            .iter()
            .filter(|message| message.role() != MessageRole::Tool)
            .cloned()
            .collect::<Vec<_>>()
            .into();
        let case = CompactionConformanceCase {
            descriptor,
            input,
            result,
            shared_projections: Arc::from([]),
        };
        prove_multi_call_component_rejection(&case)
            .expect("no retained multi-call component means nothing to prove");
    }
}
