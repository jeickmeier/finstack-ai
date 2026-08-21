//! One `before_model` context-compactor leaf with three configured strategies.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[cfg(test)]
use std::cell::Cell;

use finstack_ai_kernel::{
    BudgetScopeId, ComponentId, ComponentInvocation, ComponentRef, ContentBlock, Digest,
    ErrorCategory, InvocationRecovery, Message, Metadata, Sensitivity, Stage, TextBlock,
    ToolResultBlock, Version,
};
use finstack_ai_runtime::{
    BeforeModelInput, COMPACTION_BUDGET_EXCEEDED, COMPACTION_MODEL_NOT_AUTHORIZED,
    CompactedSummary, CompactionCheckpoint, CompactionEvidence, CompactionResult,
    CompactionSourceEntry, ContextAuthority, ContextItem, ContextItemKind, ContextProvenance,
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRole, OrderTier, PortFuture, PromptCacheImpact, StageInput, StageMask, StageOutcome,
    compaction_projection_digest, compaction_protected_set_digest, compaction_source_digest,
    compaction_summary_digest, validate_compaction_result,
};
use serde::Serialize;
use thiserror::Error;

/// Sliding-window strategy identity.
pub const SLIDING_WINDOW: &str = "finstack.compaction.sliding_window";
/// Large-tool-output strategy identity.
pub const LARGE_TOOL_OUTPUT: &str = "finstack.compaction.large_tool_output";
/// Model-assisted summarize strategy identity.
pub const SUMMARIZE: &str = "finstack.compaction.summarize";

/// Configured compaction strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStrategy {
    /// Drop oldest eligible mutable entries.
    SlidingWindow,
    /// Truncate large tool-result bodies while keeping pairs.
    LargeToolOutput,
    /// Request one runtime-owned compaction-summary model, then resume.
    Summarize,
}

impl CompactionStrategy {
    fn id(self) -> &'static str {
        match self {
            Self::SlidingWindow => SLIDING_WINDOW,
            Self::LargeToolOutput => LARGE_TOOL_OUTPUT,
            Self::Summarize => SUMMARIZE,
        }
    }
}

/// Locked compaction configuration hashed into the descriptor digest.
///
/// Fields are private. Build a strategy with [`Self::sliding_window`],
/// [`Self::large_tool_output`], or [`Self::summarize`]. Configuration
/// cannot authorize a secondary model; runtime authority is owned by
/// PR-112.
#[derive(Debug, Clone, Serialize)]
pub struct CompactionConfig {
    strategy: CompactionStrategy,
    threshold_tokens: u64,
    hysteresis_tokens: u64,
    large_tool_output_bytes: usize,
    summarize_model: Option<ComponentRef>,
    budget_scope_id: Option<BudgetScopeId>,
    residency_policy_digest: Digest,
    /// Always serialized as `false`. No constructor can set this true.
    secondary_model_authorized: bool,
}

impl CompactionConfig {
    fn locked(
        strategy: CompactionStrategy,
        threshold_tokens: u64,
        hysteresis_tokens: u64,
        large_tool_output_bytes: usize,
        summarize_model: Option<ComponentRef>,
        budget_scope_id: Option<BudgetScopeId>,
        residency_policy_digest: Digest,
    ) -> Self {
        Self {
            strategy,
            threshold_tokens,
            hysteresis_tokens,
            large_tool_output_bytes,
            summarize_model,
            budget_scope_id,
            residency_policy_digest,
            secondary_model_authorized: false,
        }
    }

    /// Sliding-window defaults with summarize off.
    ///
    /// Unused strategy fields keep the previous constructor defaults so
    /// serialized keys and configuration-digest bytes stay identical.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_middleware_compaction::CompactionConfig;
    ///
    /// let _config = CompactionConfig::sliding_window(1_024, 256);
    /// ```
    #[must_use]
    pub fn sliding_window(threshold_tokens: u64, hysteresis_tokens: u64) -> Self {
        Self::locked(
            CompactionStrategy::SlidingWindow,
            threshold_tokens,
            hysteresis_tokens,
            2_048,
            None,
            None,
            Digest::raw_json(b"residency-denied"),
        )
    }

    /// Truncate large tool-result bodies while keeping call/result pairs.
    ///
    /// Other fields keep the [`Self::sliding_window`] defaults so a
    /// configuration that previously mutated only `strategy` and
    /// `large_tool_output_bytes` produces the same serialized bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_middleware_compaction::CompactionConfig;
    ///
    /// let _config = CompactionConfig::large_tool_output(1_024, 256, 16);
    /// ```
    #[must_use]
    pub fn large_tool_output(
        threshold_tokens: u64,
        hysteresis_tokens: u64,
        byte_limit: usize,
    ) -> Self {
        Self::locked(
            CompactionStrategy::LargeToolOutput,
            threshold_tokens,
            hysteresis_tokens,
            byte_limit,
            None,
            None,
            Digest::raw_json(b"residency-denied"),
        )
    }

    /// Model-assisted summarize identity. Does not grant secondary-model
    /// authority; a first invoke without resume fails closed until the
    /// PR-112 runtime lock is present.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{BudgetScopeId, ComponentId, ComponentRef, Digest, Version};
    /// use finstack_ai_middleware_compaction::CompactionConfig;
    ///
    /// let model = ComponentRef::new(
    ///     ComponentId::parse("finstack.model.summarize").expect("id"),
    ///     Some(Version {
    ///         major: 0,
    ///         minor: 0,
    ///         patch: 4,
    ///     }),
    /// );
    /// let budget = BudgetScopeId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("budget");
    /// let _config = CompactionConfig::summarize(
    ///     1_024,
    ///     256,
    ///     model,
    ///     budget,
    ///     Digest::raw_json(b"residency-policy"),
    /// );
    /// ```
    #[must_use]
    pub fn summarize(
        threshold_tokens: u64,
        hysteresis_tokens: u64,
        model: ComponentRef,
        budget_scope: BudgetScopeId,
        residency_policy_digest: Digest,
    ) -> Self {
        Self::locked(
            CompactionStrategy::Summarize,
            threshold_tokens,
            hysteresis_tokens,
            2_048,
            Some(model),
            Some(budget_scope),
            residency_policy_digest,
        )
    }
}

/// Compaction leaf construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactionError {
    /// Configuration is malformed.
    #[error("compaction_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Unique `ContextCompactor` middleware.
#[derive(Debug, Clone)]
pub struct CompactionMiddleware {
    descriptor: MiddlewareDescriptor,
    config: CompactionConfig,
}

impl CompactionMiddleware {
    /// Construct one strategy owner.
    ///
    /// # Errors
    ///
    /// Rejects a zero threshold or an invalid checked-in identity.
    pub fn try_new(config: CompactionConfig) -> Result<Self, CompactionError> {
        if config.threshold_tokens == 0 || config.large_tool_output_bytes == 0 {
            return Err(CompactionError::Configuration {
                reason: "invalid_compaction_thresholds",
            });
        }
        let configuration_digest =
            Digest::raw_json(&serde_json::to_vec(&config).map_err(|_| {
                CompactionError::Configuration {
                    reason: "invalid_configuration_encoding",
                }
            })?);
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.middleware.compaction").map_err(
                        |_| CompactionError::Configuration {
                            reason: "invalid_component_id",
                        },
                    )?,
                    version: Version {
                        major: 0,
                        minor: 0,
                        patch: 4,
                    },
                    configuration_digest,
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
                    strategy_id: Arc::from(config.strategy.id()),
                    strategy_version: 1,
                },
                metadata: Metadata::empty(),
            },
            config,
        })
    }
}

impl Middleware for CompactionMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let descriptor = self.descriptor.clone();
        let config = self.config.clone();
        Box::pin(async move {
            if ctx.run.cancellation.is_cancelled() {
                return Err(middleware_error(
                    "compaction_cancelled",
                    ErrorCategory::Cancellation,
                    "compaction invocation was cancelled",
                ));
            }
            let StageInput::BeforeModel(before) = input else {
                return Err(MiddlewareError::try_new(
                    finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                    ErrorCategory::Middleware,
                    "compaction only runs at before_model",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into));
            };
            match config.strategy {
                CompactionStrategy::SlidingWindow => sliding_window(&descriptor, &config, &before),
                CompactionStrategy::LargeToolOutput => {
                    large_tool_output(&descriptor, &config, &before)
                }
                CompactionStrategy::Summarize => summarize(&descriptor, &config, &ctx, &before),
            }
        })
    }
}

fn sliding_window(
    descriptor: &MiddlewareDescriptor,
    config: &CompactionConfig,
    input: &BeforeModelInput,
) -> Result<StageOutcome, MiddlewareError> {
    let before = estimate_entries(&input.source_entries);
    if before <= config.threshold_tokens {
        return Ok(StageOutcome::Continue);
    }
    let target = config
        .threshold_tokens
        .saturating_sub(config.hysteresis_tokens)
        .max(1);
    let pairs = collect_pairs(&input.source_entries);
    let required = required_indices(&input.source_entries, &pairs);
    let mut retained: BTreeSet<usize> = (0..input.source_entries.len()).collect();
    let mut current = before;
    for index in 0..input.source_entries.len() {
        if current <= target {
            break;
        }
        if required.contains(&index) || !retained.contains(&index) {
            continue;
        }
        current = current.saturating_sub(drop_with_pair(
            index,
            &input.source_entries,
            &pairs,
            &mut retained,
        ));
    }
    let after = current;
    if after > input.hard_input_tokens {
        return Err(budget_error());
    }
    let messages = retained_messages(&input.source_entries, &retained);
    finish(
        descriptor,
        config,
        input,
        messages,
        Vec::new(),
        before,
        after,
        PromptCacheImpact::CacheInvalidated,
    )
}

fn large_tool_output(
    descriptor: &MiddlewareDescriptor,
    config: &CompactionConfig,
    input: &BeforeModelInput,
) -> Result<StageOutcome, MiddlewareError> {
    let before = estimate_entries(&input.source_entries);
    if before <= config.threshold_tokens {
        return Ok(StageOutcome::Continue);
    }
    let mut messages = Vec::new();
    for entry in input.source_entries.iter() {
        if entry.protected {
            messages.push(entry.message.clone());
            continue;
        }
        messages.push(truncate_tool_message(
            &entry.message,
            config.large_tool_output_bytes,
        )?);
    }
    let after = messages.iter().map(estimate_message).sum::<u64>();
    if after > input.hard_input_tokens {
        return Err(budget_error());
    }
    finish(
        descriptor,
        config,
        input,
        messages,
        Vec::new(),
        before,
        after,
        PromptCacheImpact::StablePrefixPreserved,
    )
}

fn summarize(
    descriptor: &MiddlewareDescriptor,
    config: &CompactionConfig,
    ctx: &MiddlewareContext,
    input: &BeforeModelInput,
) -> Result<StageOutcome, MiddlewareError> {
    if let Some(resume) = &ctx.compaction_resume {
        return summarize_resume(descriptor, config, input, resume);
    }
    Err(middleware_error(
        COMPACTION_MODEL_NOT_AUTHORIZED,
        ErrorCategory::Middleware,
        "compaction secondary model is not authorized",
    ))
}

fn summarize_resume(
    descriptor: &MiddlewareDescriptor,
    config: &CompactionConfig,
    input: &BeforeModelInput,
    resume: &finstack_ai_runtime::CompactionModelResume,
) -> Result<StageOutcome, MiddlewareError> {
    let before = estimate_entries(&input.source_entries);
    let pairs = collect_pairs(&input.source_entries);
    let required = required_indices(&input.source_entries, &pairs);
    let messages = retained_messages(&input.source_entries, &required);
    let after = messages.iter().map(estimate_message).sum::<u64>();
    if after > input.hard_input_tokens {
        return Err(budget_error());
    }
    let summary_text = resume
        .result
        .assistant_content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let summary = ContextItem::try_new(
        ContextItemKind::DerivedSummary,
        vec![ContentBlock::Text(
            TextBlock::try_new(if summary_text.is_empty() {
                "compacted history"
            } else {
                &summary_text
            })
            .map_err(|_| {
                middleware_error(
                    finstack_ai_runtime::COMPACTION_RESULT_INVALID,
                    ErrorCategory::Middleware,
                    "compaction summary text is invalid",
                )
            })?,
        )],
        ContextProvenance {
            source_id: Arc::from(SUMMARIZE),
            source_ref: None,
            external: false,
        },
        ContextAuthority::Untrusted,
        0,
        estimate_tokens(summary_text.len()),
        max_sensitivity(&input.source_entries),
        false,
    )
    .map_err(|_| {
        middleware_error(
            finstack_ai_runtime::COMPACTION_RESULT_INVALID,
            ErrorCategory::Middleware,
            "compaction derived summary is invalid",
        )
    })?;
    finish(
        descriptor,
        config,
        input,
        messages,
        vec![summary],
        before,
        after,
        PromptCacheImpact::CacheInvalidated,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish(
    descriptor: &MiddlewareDescriptor,
    config: &CompactionConfig,
    input: &BeforeModelInput,
    messages: Vec<Message>,
    summaries: Vec<ContextItem>,
    before: u64,
    after: u64,
    cache_impact: PromptCacheImpact,
) -> Result<StageOutcome, MiddlewareError> {
    let replacement_messages: Arc<[Message]> = messages.into();
    let derived_summaries: Arc<[ContextItem]> = summaries.into();
    let source_digest = compaction_source_digest(&input.source_entries)?;
    let protected_ids = input
        .source_entries
        .iter()
        .filter(|entry| entry.protected)
        .map(|entry| entry.entry_id)
        .collect::<Vec<_>>();
    let retained_entry_ids = input
        .source_entries
        .iter()
        .filter(|entry| {
            replacement_messages
                .iter()
                .any(|message| message.id() == entry.message.id())
        })
        .map(|entry| entry.entry_id)
        .collect::<Vec<_>>();
    let covered_entry_ids: Arc<[finstack_ai_kernel::EntryId]> = input
        .source_entries
        .iter()
        .map(|entry| entry.entry_id)
        .collect::<Vec<_>>()
        .into();
    let checkpoint_summary = CompactedSummary::Inline(Arc::clone(&derived_summaries));
    let summary_digest = compaction_summary_digest(&checkpoint_summary)?;
    let covered_through_entry_id = *covered_entry_ids.last().ok_or_else(|| {
        middleware_error(
            finstack_ai_runtime::COMPACTION_RESULT_INVALID,
            ErrorCategory::Middleware,
            "compaction source is empty",
        )
    })?;
    let result = CompactionResult {
        replacement_messages: Arc::clone(&replacement_messages),
        derived_summaries,
        evidence: CompactionEvidence {
            strategy_id: Arc::from(config.strategy.id()),
            strategy_version: 1,
            configuration_digest: descriptor.invocation.configuration_digest,
            model_context_profile_digest: input.model_context_profile_digest,
            source_digest,
            protected_item_set_digest: compaction_protected_set_digest(&protected_ids)?,
            covered_entry_ids: Arc::clone(&covered_entry_ids),
            retained_entry_ids: retained_entry_ids.into(),
            projection_digest: compaction_projection_digest(&replacement_messages)?,
            estimated_tokens_before: before,
            estimated_tokens_after: after.min(before),
            summary_digest: Some(summary_digest),
            cache_impact,
        },
        checkpoint: Some(CompactionCheckpoint {
            component_id: descriptor.invocation.component.clone(),
            strategy_id: Arc::from(config.strategy.id()),
            strategy_version: 1,
            configuration_digest: descriptor.invocation.configuration_digest,
            model_context_profile_digest: input.model_context_profile_digest,
            covered_through_entry_id,
            source_digest,
            summary: checkpoint_summary,
            summary_digest,
            sensitivity: max_sensitivity(&input.source_entries),
        }),
    };
    validate_compaction_result(descriptor, input, &result)?;
    Ok(StageOutcome::CompactContext(Box::new(result)))
}

fn required_indices(
    entries: &[CompactionSourceEntry],
    pairs: &BTreeMap<usize, Option<usize>>,
) -> BTreeSet<usize> {
    let mut required = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.protected {
            required.insert(index);
        }
    }
    for (call, result) in pairs {
        if result.is_none() {
            required.insert(*call);
        }
    }
    required
}

fn collect_pairs(entries: &[CompactionSourceEntry]) -> BTreeMap<usize, Option<usize>> {
    #[cfg(test)]
    PAIR_ANALYSIS_COUNT.with(|count| count.set(count.get().saturating_add(1)));
    let mut calls = BTreeMap::new();
    let mut results = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        for block in entry.message.content() {
            match block {
                ContentBlock::ToolCall(call) => {
                    calls.insert(*call.tool_call_id(), index);
                }
                ContentBlock::ToolResult(result) => {
                    results.insert(*result.tool_call_id(), index);
                }
                _ => {}
            }
        }
    }
    calls
        .into_iter()
        .map(|(id, call_index)| (call_index, results.get(&id).copied()))
        .collect()
}

fn drop_with_pair(
    index: usize,
    entries: &[CompactionSourceEntry],
    pairs: &BTreeMap<usize, Option<usize>>,
    retained: &mut BTreeSet<usize>,
) -> u64 {
    let mut subtracted = 0_u64;
    if retained.remove(&index) {
        subtracted = subtracted.saturating_add(estimate_message(&entries[index].message));
    }
    for (call, result) in pairs {
        if *call == index
            && let Some(result) = *result
            && retained.remove(&result)
        {
            subtracted = subtracted.saturating_add(estimate_message(&entries[result].message));
        }
        if *result == Some(index) && retained.remove(call) {
            subtracted = subtracted.saturating_add(estimate_message(&entries[*call].message));
        }
    }
    subtracted
}

#[cfg(test)]
thread_local! {
    static PAIR_ANALYSIS_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn reset_pair_analysis_count() {
    PAIR_ANALYSIS_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
#[must_use]
fn pair_analysis_count() -> usize {
    PAIR_ANALYSIS_COUNT.with(Cell::get)
}

fn retained_messages(
    entries: &[CompactionSourceEntry],
    retained: &BTreeSet<usize>,
) -> Vec<Message> {
    entries
        .iter()
        .enumerate()
        .filter(|(index, _)| retained.contains(index))
        .map(|(_, entry)| entry.message.clone())
        .collect()
}

fn truncate_tool_message(message: &Message, limit: usize) -> Result<Message, MiddlewareError> {
    let mut content = Vec::new();
    for block in message.content() {
        match block {
            ContentBlock::ToolResult(result) => {
                let truncated = truncate_blocks(result.content(), limit);
                content.push(ContentBlock::ToolResult(
                    ToolResultBlock::try_new(*result.tool_call_id(), truncated, result.is_error())
                        .map_err(|_| {
                            middleware_error(
                                finstack_ai_runtime::COMPACTION_RESULT_INVALID,
                                ErrorCategory::Middleware,
                                "truncated tool result is invalid",
                            )
                        })?,
                ));
            }
            other => content.push(other.clone()),
        }
    }
    Message::try_new(
        *message.id(),
        message.role(),
        content,
        message.created_at(),
        message.model().cloned(),
        message.provider_ids().clone(),
        message.metadata().clone(),
    )
    .map_err(|_| {
        middleware_error(
            finstack_ai_runtime::COMPACTION_RESULT_INVALID,
            ErrorCategory::Middleware,
            "truncated tool message is invalid",
        )
    })
}

fn truncate_blocks(blocks: &[ContentBlock], limit: usize) -> Vec<ContentBlock> {
    let mut remaining = limit;
    let mut out = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) if text.text().len() > remaining => {
                let preview = text
                    .text()
                    .chars()
                    .take(remaining.max(1))
                    .collect::<String>();
                if let Ok(block) = TextBlock::try_new(preview) {
                    out.push(ContentBlock::Text(block));
                }
                break;
            }
            ContentBlock::Text(text) => {
                remaining = remaining.saturating_sub(text.text().len());
                out.push(ContentBlock::Text(text.clone()));
            }
            other => out.push(other.clone()),
        }
    }
    if out.is_empty()
        && let Ok(block) = TextBlock::try_new("[truncated]")
    {
        out.push(ContentBlock::Text(block));
    }
    out
}

fn estimate_entries(entries: &[CompactionSourceEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| estimate_message(&entry.message))
        .sum()
}

fn estimate_message(message: &Message) -> u64 {
    estimate_tokens(message.content().iter().map(block_len).sum::<usize>())
}

fn block_len(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text(text) => text.text().len(),
        ContentBlock::ToolCall(call) => call.arguments().as_bytes().len(),
        ContentBlock::ToolResult(result) => result.content().iter().map(block_len).sum(),
        _ => 8,
    }
}

fn estimate_tokens(bytes: usize) -> u64 {
    u64::try_from(bytes.div_ceil(4)).unwrap_or(1).max(1)
}

fn max_sensitivity(entries: &[CompactionSourceEntry]) -> Sensitivity {
    entries
        .iter()
        .map(|entry| entry.sensitivity)
        .max_by_key(|value| match value {
            Sensitivity::Public => 0,
            Sensitivity::Internal => 1,
            Sensitivity::Confidential => 2,
            Sensitivity::Secret => 3,
            Sensitivity::Credential => 4,
        })
        .unwrap_or(Sensitivity::Internal)
}

fn budget_error() -> MiddlewareError {
    middleware_error(
        COMPACTION_BUDGET_EXCEEDED,
        ErrorCategory::Limit,
        "compacted projection exceeds the hard model-input budget",
    )
}

fn middleware_error(
    code: &'static str,
    category: ErrorCategory,
    message: &'static str,
) -> MiddlewareError {
    MiddlewareError::try_new(code, category, message, Metadata::empty()).unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
