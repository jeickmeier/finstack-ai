use std::sync::Arc;

use finstack_ai_kernel::{Digest, EntryId, Message, Sensitivity};
use serde::Serialize;

use super::COMPACTION_RESULT_INVALID;
use super::error::MiddlewareError;
use super::types::{
    BeforeModelInput, CompactedSummary, CompactionCheckpoint, CompactionSourceEntry,
    MiddlewareDescriptor, MiddlewareRole, canonical_bytes,
};

/// Digest canonical compaction source entries under the frozen v1 domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_source_digest(
    entries: &Arc<[CompactionSourceEntry]>,
) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-source", entries)
}

/// Digest the ordered protected-entry set under the frozen v1 domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_protected_set_digest(entries: &[EntryId]) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-protected-set", &entries)
}

/// Digest replacement messages under the frozen v1 projection domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_projection_digest(messages: &Arc<[Message]>) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-projection", messages)
}

/// Digest a reusable summary under the frozen v1 summary domain.
///
/// # Errors
///
/// Returns a stable compaction error when canonicalization fails.
pub fn compaction_summary_digest(summary: &CompactedSummary) -> Result<Digest, MiddlewareError> {
    digest_value("compaction-summary", summary)
}

/// Check whether a reusable checkpoint exactly matches the current compactor and history prefix.
///
/// A mismatch is an ordinary cache miss: callers must ignore it and rebuild from canonical
/// history. Malformed canonical data returns an integrity error instead of being trusted.
///
/// # Errors
///
/// Returns a stable compaction error when source or summary digests cannot be computed.
pub fn compaction_checkpoint_compatible(
    descriptor: &MiddlewareDescriptor,
    input: &BeforeModelInput,
    checkpoint: &CompactionCheckpoint,
) -> Result<bool, MiddlewareError> {
    let MiddlewareRole::ContextCompactor {
        strategy_id,
        strategy_version,
    } = &descriptor.role
    else {
        return Ok(false);
    };
    let Some(covered_index) = input
        .source_entries
        .iter()
        .position(|entry| entry.entry_id == checkpoint.covered_through_entry_id)
    else {
        return Ok(false);
    };
    let covered: Arc<[CompactionSourceEntry]> =
        input.source_entries[..=covered_index].to_vec().into();
    let source_digest = compaction_source_digest(&covered)?;
    let summary_digest = compaction_summary_digest(&checkpoint.summary)?;
    let source_sensitivity = covered
        .iter()
        .map(|entry| sensitivity_rank(entry.sensitivity))
        .max()
        .unwrap_or(0);
    let summary_is_safe = match &checkpoint.summary {
        CompactedSummary::Inline(items) => items.iter().all(|item| {
            item.kind == crate::ports::context::ContextItemKind::DerivedSummary
                && item.authority == crate::ports::context::ContextAuthority::Untrusted
                && sensitivity_rank(item.sensitivity) >= source_sensitivity
        }),
        CompactedSummary::Artifact(_) => true,
    };
    Ok(checkpoint.component_id == descriptor.invocation.component
        && checkpoint.strategy_id.as_ref() == strategy_id.as_ref()
        && checkpoint.strategy_version == *strategy_version
        && checkpoint.configuration_digest == descriptor.invocation.configuration_digest
        && checkpoint.model_context_profile_digest == input.model_context_profile_digest
        && checkpoint.source_digest == source_digest
        && checkpoint.summary_digest == summary_digest
        && sensitivity_rank(checkpoint.sensitivity) >= source_sensitivity
        && summary_is_safe)
}

/// Monotone rank used to compare sensitivity classes across ports and the event hub.
pub(crate) const fn sensitivity_rank(value: Sensitivity) -> u8 {
    match value {
        Sensitivity::Public => 0,
        Sensitivity::Internal => 1,
        Sensitivity::Confidential => 2,
        Sensitivity::Secret => 3,
        Sensitivity::Credential => 4,
    }
}

fn digest_value<T: Serialize>(domain: &'static str, value: &T) -> Result<Digest, MiddlewareError> {
    Digest::domain_separated(domain, 1, &canonical_bytes(value)?).map_err(|_| {
        MiddlewareError::stable(
            COMPACTION_RESULT_INVALID,
            "compaction digest could not be constructed",
        )
    })
}
