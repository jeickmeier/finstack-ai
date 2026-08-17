use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{CapabilityId, Message, SEMANTIC_ARRAY_MAX_ITEMS};

use super::assembly::AssembledContext;
use super::error::{CONTEXT_CONFIGURATION_INVALID, ContextError};
use super::types::{ContextAuthority, ContextItem, ContextItemKind};

/// Locked capability instructions used during final context projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityContext {
    /// Capability identity in locked activation order.
    pub capability_id: CapabilityId,
    /// Trusted capability instruction items in declared order.
    pub instructions: Arc<[ContextItem]>,
}

/// Source group for one final model-visible projection item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextProjectionSource {
    /// Resolved system policy.
    System,
    /// Resolved developer/application instruction.
    Developer,
    /// Active capability instruction.
    Capability,
    /// Authorized provider application reminder.
    ProviderReminder,
    /// Delimited external provider context.
    ExternalContext,
    /// Canonical conversation history.
    History,
    /// Current user request, exactly once and last.
    CurrentUser,
}

/// Message or context item in the fixed-authority model projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextProjectionItem {
    /// Canonical message group.
    Message {
        /// Fixed source group.
        source: ContextProjectionSource,
        /// Immutable message.
        message: Message,
    },
    /// Provider/capability context item.
    Context {
        /// Fixed source group.
        source: ContextProjectionSource,
        /// Immutable normalized item.
        item: ContextItem,
    },
}

/// Inputs for fixed authority and ordering assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextProjectionInput {
    /// Resolved system messages in declared order.
    pub system: Arc<[Message]>,
    /// Resolved developer/application messages in declared order.
    pub developer: Arc<[Message]>,
    /// Active capabilities in locked activation order.
    pub capabilities: Arc<[CapabilityContext]>,
    /// Already budgeted and deterministically ordered provider context.
    pub providers: AssembledContext,
    /// Canonical conversation history in parent-chain order.
    pub history: Arc<[Message]>,
    /// Current user request.
    pub current_user: Message,
}

/// Assemble the fixed authority groups ending with exactly one current user request.
///
/// # Errors
///
/// Returns a stable configuration error for role drift, duplicate capability identity, invalid
/// capability instructions, or a duplicated current-user message.
pub fn assemble_context_projection(
    input: ContextProjectionInput,
) -> Result<Arc<[ContextProjectionItem]>, ContextError> {
    if input
        .system
        .iter()
        .any(|message| message.role() != finstack_ai_kernel::MessageRole::System)
        || input
            .developer
            .iter()
            .any(|message| message.role() != finstack_ai_kernel::MessageRole::Developer)
        || input.current_user.role() != finstack_ai_kernel::MessageRole::User
        || input
            .history
            .iter()
            .any(|message| message.id() == input.current_user.id())
    {
        return Err(ContextError::stable(
            CONTEXT_CONFIGURATION_INVALID,
            "context authority group contains an invalid role or duplicate current user",
        ));
    }
    let mut capability_ids = BTreeSet::new();
    for capability in input.capabilities.iter() {
        if !capability_ids.insert(capability.capability_id.clone())
            || capability.instructions.iter().any(|item| {
                item.kind != ContextItemKind::Instruction
                    || item.authority != ContextAuthority::TrustedApplication
                    || item.provenance.external
            })
        {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "capability context is duplicate or not trusted resolved instruction data",
            ));
        }
    }
    let capacity = input
        .system
        .len()
        .saturating_add(input.developer.len())
        .saturating_add(
            input
                .capabilities
                .iter()
                .map(|capability| capability.instructions.len())
                .sum::<usize>(),
        )
        .saturating_add(input.providers.items.len())
        .saturating_add(input.history.len())
        .saturating_add(1);
    if capacity > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(ContextError::stable(
            CONTEXT_CONFIGURATION_INVALID,
            "assembled context projection exceeds the semantic item bound",
        ));
    }
    let mut projection = Vec::with_capacity(capacity);
    projection.extend(
        input
            .system
            .iter()
            .cloned()
            .map(|message| ContextProjectionItem::Message {
                source: ContextProjectionSource::System,
                message,
            }),
    );
    projection.extend(input.developer.iter().cloned().map(|message| {
        ContextProjectionItem::Message {
            source: ContextProjectionSource::Developer,
            message,
        }
    }));
    for capability in input.capabilities.iter() {
        projection.extend(capability.instructions.iter().cloned().map(|item| {
            ContextProjectionItem::Context {
                source: ContextProjectionSource::Capability,
                item,
            }
        }));
    }
    projection.extend(input.providers.items.iter().cloned().map(|item| {
        let source = if item.kind == ContextItemKind::Instruction
            && item.authority == ContextAuthority::TrustedApplication
        {
            ContextProjectionSource::ProviderReminder
        } else {
            ContextProjectionSource::ExternalContext
        };
        ContextProjectionItem::Context { source, item }
    }));
    projection.extend(input.history.iter().cloned().map(|message| {
        ContextProjectionItem::Message {
            source: ContextProjectionSource::History,
            message,
        }
    }));
    projection.push(ContextProjectionItem::Message {
        source: ContextProjectionSource::CurrentUser,
        message: input.current_user,
    });
    Ok(projection.into())
}
