use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{
    CapabilityId, EntryId, Message, MessageRole, MessageTag, ProviderIds, SEMANTIC_ARRAY_MAX_ITEMS,
    Sensitivity,
};

use crate::context::{
    ContextAuthority, ContextBudget, ContextItem, ContextItemKind, ContextOverflowPolicy,
    ContextRequest, RecordedContextContribution, assemble_context,
};
use crate::coordinator::CommitCoordinator;
use crate::model::LockedModelContextProfile;
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;
use crate::{Clock, RandomSource, RunCallContext};

use super::commit::{
    chain_digest, committed_context_call, context_error, derived_context_effect_id,
};
use super::{ContextDriver, ProtectedProjection, empty_assembled};

/// Assembled provider context plus the message array and `protected` map.
#[derive(Clone)]
pub(crate) struct ContextStagePlan {
    pub(crate) messages: Arc<[Message]>,
    pub(crate) projection: ProtectedProjection,
}

/// Invoke locked providers, assemble, and rebuild `PrepareContext` messages.
///
/// Provider items are inserted before the current user so the trailing source
/// entry stays the protected user request. System and developer messages stay
/// protected regardless of provider output.
///
/// # Errors
///
/// Returns a stable context or identity error when a provider or assembly fails.
pub(crate) async fn collect_context_stage<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    driver: &ContextDriver,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    cycle: u64,
    base_messages: &[Message],
) -> Result<ContextStagePlan, RunHandleError> {
    if driver.cancellation().is_cancelled() {
        return Ok(plan_from_messages(base_messages, &empty_assembled()));
    }
    let seed = coordinator
        .stage_dispatch_seed()
        .ok_or_else(|| RunHandleError::Middleware {
            code: Arc::from("middleware_stage_identity_missing"),
        })?;
    let request = context_request(&seed.locator, coordinator, profile, base_messages);
    let digest = chain_digest(driver.providers());
    let mut recorded = Vec::with_capacity(driver.providers().len());
    for (index, provider) in driver.providers().iter().enumerate() {
        let provider_index = u32::try_from(index).map_err(|_| RunHandleError::Middleware {
            code: Arc::from(crate::CONTEXT_CONFIGURATION_INVALID),
        })?;
        let run = RunCallContext {
            effect_id: derived_context_effect_id(&seed.locator, cycle, provider_index),
            locator: seed.locator.clone(),
            authorization: seed.authorization.clone(),
            attempt: seed.attempt,
            deadline: seed.deadline,
            budget_scope_id: seed.budget_scope_id,
            cancellation: driver.cancellation().child(),
        };
        let contribution = committed_context_call(
            driver,
            provider.as_ref(),
            provider_index,
            run,
            request.clone(),
            digest,
            sources,
        )
        .await?;
        recorded.push(RecordedContextContribution {
            component: provider.descriptor().invocation.component,
            provider_index,
            contribution,
        });
    }
    let assembled = if recorded.is_empty() {
        empty_assembled()
    } else {
        assemble_context(recorded, request.budget).map_err(|error| context_error(&error))?
    };
    let messages = rebuild_messages(base_messages, &assembled.items, sources)?;
    let projection = project_protected(&messages, &assembled.items);
    Ok(ContextStagePlan {
        messages,
        projection,
    })
}

fn context_request(
    locator: &finstack_ai_kernel::OperationLocator,
    coordinator: &CommitCoordinator,
    profile: &LockedModelContextProfile,
    messages: &[Message],
) -> ContextRequest {
    let (current_user, history) = split_current_user(messages);
    let user_input = current_user.map_or_else(
        || Arc::from([]),
        |message| Arc::from(message.content().to_vec()),
    );
    let recent_history: Arc<[Message]> = history.into();
    let active_capabilities: Arc<[CapabilityId]> = coordinator
        .state()
        .active_capabilities
        .iter()
        .map(|capability| capability.capability_id.clone())
        .collect::<Vec<_>>()
        .into();
    let hard_tokens = profile
        .profile
        .context_window_tokens
        .saturating_sub(
            profile
                .profile
                .reserved_output_tokens
                .saturating_add(profile.profile.provider_overhead_tokens),
        )
        .max(1);
    ContextRequest {
        session_id: locator.session_id,
        lane_id: locator.lane_id,
        run_id: locator.run_id,
        user_input,
        recent_history,
        budget: ContextBudget {
            max_items: SEMANTIC_ARRAY_MAX_ITEMS,
            max_tokens: hard_tokens,
            max_bytes: profile.profile.hard_input_bytes,
            overflow: ContextOverflowPolicy::TruncateWithDiagnostic,
        },
        active_capabilities,
    }
}

fn split_current_user(messages: &[Message]) -> (Option<&Message>, Vec<Message>) {
    let current_user = messages
        .iter()
        .rposition(|message| message.role() == MessageRole::User);
    match current_user {
        Some(index) => {
            let current = &messages[index];
            let mut history = Vec::with_capacity(messages.len().saturating_sub(1));
            history.extend(messages[..index].iter().cloned());
            history.extend(messages[index + 1..].iter().cloned());
            (Some(current), history)
        }
        None => (None, messages.to_vec()),
    }
}

fn rebuild_messages<C: Clock, R: RandomSource>(
    base: &[Message],
    items: &[ContextItem],
    sources: &SettlementSources<C, R>,
) -> Result<Arc<[Message]>, RunHandleError> {
    let (current_user, prefix_and_history) = split_current_user(base);
    let leading = prefix_and_history
        .iter()
        .take_while(|message| {
            matches!(message.role(), MessageRole::System | MessageRole::Developer)
        })
        .cloned()
        .collect::<Vec<_>>();
    let history = prefix_and_history
        .iter()
        .skip(leading.len())
        .cloned()
        .collect::<Vec<_>>();
    let mut messages = leading;
    for item in items {
        messages.push(message_from_item(item, sources)?);
    }
    messages.extend(history);
    if let Some(current) = current_user {
        messages.push(current.clone());
    }
    if messages.len() > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(RunHandleError::Middleware {
            code: Arc::from(crate::middleware_driver::MIDDLEWARE_STAGE_BOUNDS_EXCEEDED),
        });
    }
    Ok(messages.into())
}

fn message_from_item<C: Clock, R: RandomSource>(
    item: &ContextItem,
    sources: &SettlementSources<C, R>,
) -> Result<Message, RunHandleError> {
    let role = if item.kind == ContextItemKind::Instruction
        && item.authority == ContextAuthority::TrustedApplication
    {
        MessageRole::System
    } else {
        MessageRole::User
    };
    Message::try_new(
        sources.generate::<MessageTag>()?,
        role,
        item.content.to_vec(),
        sources.now()?,
        None,
        ProviderIds::empty(),
        finstack_ai_kernel::Metadata::empty(),
    )
    .map_err(|_| RunHandleError::Middleware {
        code: Arc::from("middleware_stage_payload_invalid"),
    })
}

fn project_protected(messages: &[Message], items: &[ContextItem]) -> ProtectedProjection {
    let mut by_item_content: BTreeMap<Vec<u8>, bool> = BTreeMap::new();
    for item in items {
        if let Ok(bytes) = serde_json_canonicalizer::to_vec(&item.content) {
            by_item_content.insert(bytes, item.protected);
        }
    }
    let last = messages.len().saturating_sub(1);
    let mut by_entry = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        let entry_id = EntryId::from_bytes(message.id().to_bytes());
        let from_item = serde_json_canonicalizer::to_vec(&message.content())
            .ok()
            .and_then(|bytes| by_item_content.get(&bytes).copied());
        let protected = structural_protected(message, index == last) || from_item.unwrap_or(false);
        by_entry.insert(entry_id, (protected, Sensitivity::Internal));
    }
    ProtectedProjection::new(by_entry)
}

/// System/developer messages and the trailing current user are always protected.
pub(crate) fn structural_protected(message: &Message, is_last: bool) -> bool {
    matches!(message.role(), MessageRole::System | MessageRole::Developer)
        || (is_last && message.role() == MessageRole::User)
}

fn plan_from_messages(
    messages: &[Message],
    assembled: &crate::context::AssembledContext,
) -> ContextStagePlan {
    let projection = project_protected(messages, &assembled.items);
    ContextStagePlan {
        messages: Arc::from(messages.to_vec()),
        projection,
    }
}
