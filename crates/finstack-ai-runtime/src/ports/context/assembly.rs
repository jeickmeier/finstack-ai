use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, SEMANTIC_ARRAY_MAX_ITEMS};
use serde::{Deserialize, Serialize};

use super::committed::RecordedContextContribution;
use super::error::{CONTEXT_BUDGET_EXCEEDED, CONTEXT_CONFIGURATION_INVALID, ContextError};
use super::types::{
    ContextAuthority, ContextBudget, ContextItem, ContextItemKind, ContextOverflowPolicy,
};

/// One diagnostic describing explicit deterministic context truncation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextTruncationDiagnostic {
    /// Provider component.
    pub component: ComponentId,
    /// Number of whole items omitted.
    pub dropped_items: u32,
    /// Omitted estimated tokens.
    pub dropped_tokens: u64,
    /// Omitted canonical bytes.
    pub dropped_bytes: u64,
}

/// Fully ordered provider-context projection and explicit truncation diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledContext {
    /// Trusted provider reminders before untrusted external items.
    pub items: Arc<[ContextItem]>,
    /// Explicit overrun diagnostics; never silently omitted.
    pub diagnostics: Arc<[ContextTruncationDiagnostic]>,
    /// Accepted estimated tokens.
    pub estimated_tokens: u64,
    /// Accepted canonical bytes.
    pub bytes: u64,
}

/// Deterministically assemble only committed provider outputs.
///
/// Provider index, authority group, descending item priority, and original item order are the
/// complete ordering keys. Completion order and hash-map iteration never participate.
///
/// # Errors
///
/// Returns a stable budget or contribution error on duplicates, gaps, invalid items, or overrun.
pub fn assemble_context(
    mut recorded: Vec<RecordedContextContribution>,
    budget: ContextBudget,
) -> Result<AssembledContext, ContextError> {
    budget.validate()?;
    if recorded.len() > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(ContextError::stable(
            CONTEXT_CONFIGURATION_INVALID,
            "recorded context provider chain exceeds the semantic item bound",
        ));
    }
    recorded.sort_by_key(|item| item.provider_index);
    let mut components = BTreeSet::new();
    for (expected_index, item) in recorded.iter().enumerate() {
        if usize::try_from(item.provider_index).ok() != Some(expected_index)
            || !components.insert(item.component.clone())
        {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "recorded context provider order has a duplicate, gap, or unstable identity",
            ));
        }
    }

    let mut indexed = Vec::new();
    for provider in recorded {
        for (source_index, item) in provider.contribution.items.iter().enumerate() {
            indexed.push((
                provider.provider_index,
                source_index,
                provider.component.clone(),
                item.clone(),
            ));
        }
    }
    indexed.sort_by_key(|(provider_index, source_index, _, item)| {
        let authority_group = u8::from(
            !(item.kind == ContextItemKind::Instruction
                && item.authority == ContextAuthority::TrustedApplication),
        );
        (
            authority_group,
            *provider_index,
            core::cmp::Reverse(item.priority),
            *source_index,
        )
    });

    let mut accepted = Vec::new();
    let mut diagnostics = Vec::new();
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for (_, _, component, item) in indexed {
        item.validate()?;
        let next_items = accepted.len().saturating_add(1);
        let next_tokens = tokens.checked_add(item.estimated_tokens);
        let next_bytes = bytes.checked_add(item.bytes);
        let fits = next_items <= budget.max_items
            && next_tokens.is_some_and(|value| value <= budget.max_tokens)
            && next_bytes.is_some_and(|value| value <= budget.max_bytes);
        if fits {
            tokens += item.estimated_tokens;
            bytes += item.bytes;
            accepted.push(item);
            continue;
        }
        if budget.overflow == ContextOverflowPolicy::Reject {
            return Err(ContextError::stable(
                CONTEXT_BUDGET_EXCEEDED,
                "assembled context exceeds its explicit budget",
            ));
        }
        if let Some(existing) = diagnostics
            .iter_mut()
            .find(|diagnostic: &&mut ContextTruncationDiagnostic| diagnostic.component == component)
        {
            existing.dropped_items = existing.dropped_items.saturating_add(1);
            existing.dropped_tokens = existing
                .dropped_tokens
                .saturating_add(item.estimated_tokens);
            existing.dropped_bytes = existing.dropped_bytes.saturating_add(item.bytes);
        } else {
            diagnostics.push(ContextTruncationDiagnostic {
                component,
                dropped_items: 1,
                dropped_tokens: item.estimated_tokens,
                dropped_bytes: item.bytes,
            });
        }
    }
    Ok(AssembledContext {
        items: accepted.into(),
        diagnostics: diagnostics.into(),
        estimated_tokens: tokens,
        bytes,
    })
}
