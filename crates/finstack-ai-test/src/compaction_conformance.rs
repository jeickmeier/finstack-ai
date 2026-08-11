//! Adversarial public conformance checks for replay-safe context compaction.

use std::sync::Arc;

use finstack_ai_kernel::{ContentBlock, Digest, EntryId};
use finstack_ai_runtime::{
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

/// Prove the PR-023 compaction integrity matrix using public runtime APIs.
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
    let error = validate_compaction_result(&case.descriptor, &case.input, &invalid)
        .expect_err("removing protected content must fail");
    ensure(
        error.code() == finstack_ai_runtime::COMPACTION_RESULT_INVALID,
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
    let error = validate_compaction_result(&case.descriptor, &case.input, &invalid)
        .expect_err("retaining a tool call without its result must fail");
    ensure(
        error.code() == finstack_ai_runtime::COMPACTION_RESULT_INVALID,
        "compaction.tool_pair.atomic",
        "orphaning a retained tool call did not fail with compaction_result_invalid",
    )
}

fn prove_hard_budget_rejection(
    case: &CompactionConformanceCase,
) -> Result<(), PortConformanceFailure> {
    let mut invalid = case.result.clone();
    invalid.evidence.estimated_tokens_after = case.input.hard_input_tokens.saturating_add(1);
    let error = validate_compaction_result(&case.descriptor, &case.input, &invalid)
        .expect_err("hard-budget overrun must fail");
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
    PortConformanceFailure {
        port: "Middleware",
        contract,
        detail: detail.into(),
    }
}
