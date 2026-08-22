use std::sync::Arc;

use finstack_ai_kernel::{
    Message, MessageRole, MessageTag, Metadata, ProviderIds, ReducerStageOutcome,
    SEMANTIC_ARRAY_MAX_ITEMS, Stage, StageCursor,
};

use crate::context::ContextItem;
use crate::ids::{Clock, RandomSource};
use crate::middleware_driver::{StageFold, StageTerminal};
use crate::model::ModelRequestDraft;
use crate::ports::middleware::{MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE};
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;

use super::codec::{canonical_draft, parse_draft, parse_messages};
use super::{MIDDLEWARE_STAGE_PAYLOAD_INVALID, stage_error};

/// Apply an aggregate fold to the facade's base outcome.
///
/// A [`StageTerminal`] wins outright: it replaces the base outcome whatever the
/// base was, which is what makes a middleware `Retry` at `BeforeFinalize`
/// supersede the facade's own structured-output `Retry` at the same cursor.
/// Otherwise exactly two non-terminal folds have a kernel landing at the stages
/// this choke point folds: `ContextPrepared` at `PrepareContext` and
/// `ModelRequestPrepared` at `BeforeModel`. A non-identity fold anywhere else
/// has nowhere to land and is rejected rather than silently dropped.
///
/// # Errors
///
/// Returns [`MIDDLEWARE_STAGE_UNLANDABLE`] for a non-terminal fold with no
/// landing at `cursor`, or the payload errors of [`apply_context_prepared`] and
/// [`apply_model_draft`].
pub(super) fn apply_fold<C: Clock, R: RandomSource>(
    fold: &StageFold,
    cursor: StageCursor,
    base: ReducerStageOutcome,
    sources: &SettlementSources<C, R>,
) -> Result<ReducerStageOutcome, RunHandleError> {
    if let Some(terminal) = fold.terminal.as_ref() {
        return Ok(match terminal {
            StageTerminal::Fail(descriptor) => {
                ReducerStageOutcome::Fail(descriptor.as_ref().clone())
            }
            StageTerminal::Retry(directive) => ReducerStageOutcome::Retry(directive.clone()),
        });
    }
    match base {
        ReducerStageOutcome::ContextPrepared { messages }
            if cursor.stage == Stage::PrepareContext =>
        {
            Ok(ReducerStageOutcome::ContextPrepared {
                messages: apply_context_prepared(fold, &messages, sources)?,
            })
        }
        ReducerStageOutcome::ModelRequestPrepared {
            request,
            component,
            output_contract,
            retry_safety,
            deadline,
        } if cursor.stage == Stage::BeforeModel => {
            let folded = apply_model_draft(fold, parse_draft(&request)?, sources)?;
            Ok(ReducerStageOutcome::ModelRequestPrepared {
                request: canonical_draft(&folded)?,
                component,
                output_contract,
                retry_safety,
                deadline,
            })
        }
        other if fold.is_identity() => Ok(other),
        _ => Err(stage_error(MIDDLEWARE_STAGE_UNLANDABLE)),
    }
}

/// Rebuild a `BeforeModel` model draft from an aggregate fold.
///
/// # Precedence: replacement **or** compaction re-bases messages, then
/// additions, then narrowing
///
/// The same field-level rule [`apply_context_prepared`] documents, extended to
/// the draft's two independent axes. [`StageFold`] keeps no ordering *between*
/// outcome kinds, so a positional "last writer wins" is not representable; this
/// applier therefore fixes a stable, documented order for *compatible*
/// aggregates:
///
/// 1. `Replace` substitutes the **whole draft** — at `BeforeModel` the stage's
///    payload is the request, so a `Replace` there re-bases model, tools,
///    output, settings and limits together, not just the messages.
/// 2. `CompactContext` replaces the **message projection** of the base draft
///    (and nothing else) when there is no `Replace`. At most one component can
///    produce one (the single-compactor rule). `derived_summaries` append as
///    user messages after the compacted projection so a landed summarize
///    result can carry the summary. `checkpoint` still has no landing and is
///    dropped. Sliding-window `CompactContext` does not use those fields.
///
///    A fold carrying **both** a `Replace` and a `CompactContext` is rejected
///    by [`StageFold::accumulate`] as [`MIDDLEWARE_STAGE_UNLANDABLE`] before
///    settlement. This applier also refuses that combination so compaction
///    cannot silently overwrite a replacement if a caller bypasses the fold.
///    The runtime does not merge leaves, wrap compactors, or rebase one
///    projection onto the other.
/// 3. `AddInstructions` (as [`MessageRole::System`]) then `AddContext` (as
///    [`MessageRole::User`]) append, each group in chain order.
/// 4. `FilterTools` intersects the surviving tool set.
///
/// Narrowing last is not arbitrary: intersection is monotone, so applying it
/// after a `Replace` that widened the tool list still honours every component's
/// restriction, whereas the reverse order would let a later `Replace`
/// resurrect a tool a `FilterTools` had already removed.
///
/// # Errors
///
/// Returns [`MIDDLEWARE_STAGE_UNLANDABLE`] when both a replacement and a
/// compaction are present. Returns [`MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`] when
/// the rebuilt message array would exceed [`ModelRequestDraft::MAX_MESSAGES`],
/// and `middleware_stage_payload_invalid` when a `Replace` payload is not a
/// model draft, an added item cannot become a valid message, or the rebuilt
/// draft fails its own `validate` (duplicate tool name, oversized collection).
pub(super) fn apply_model_draft<C: Clock, R: RandomSource>(
    fold: &StageFold,
    base: ModelRequestDraft,
    sources: &SettlementSources<C, R>,
) -> Result<ModelRequestDraft, RunHandleError> {
    if fold.replacement.is_some() && fold.compaction.is_some() {
        return Err(stage_error(MIDDLEWARE_STAGE_UNLANDABLE));
    }
    let mut draft = match fold.replacement.as_ref() {
        Some(replacement) => parse_draft(replacement)?,
        None => base,
    };
    if let Some(compaction) = fold.compaction.as_ref() {
        let mut messages = compaction.replacement_messages.to_vec();
        for item in compaction.derived_summaries.iter() {
            messages.push(message_from_item(item, MessageRole::User, sources)?);
        }
        draft.messages = messages.into();
    }
    if !fold.instructions.is_empty() || !fold.context.is_empty() {
        let mut messages = draft.messages.to_vec();
        for item in &fold.instructions {
            messages.push(message_from_item(item, MessageRole::System, sources)?);
        }
        for item in &fold.context {
            messages.push(message_from_item(item, MessageRole::User, sources)?);
        }
        draft.messages = messages.into();
    }
    // Unconditional, not folded into the branch above: `StageFold::accumulate`
    // bounds-checks its own additions and its compaction projection
    // (`middleware_driver.rs`'s `check_bounds`), but it never inspects a
    // `Replace` payload, so a replacement draft is the one way the array can
    // arrive here oversized with nothing appended. Reaching `validate` in that
    // case would report it as `middleware_stage_payload_invalid` — a different
    // stable code for the identical condition.
    if draft.messages.len() > ModelRequestDraft::MAX_MESSAGES {
        return Err(stage_error(MIDDLEWARE_STAGE_BOUNDS_EXCEEDED));
    }
    if let Some(retained) = fold.retained_tools.as_ref() {
        draft.tools = draft
            .tools
            .iter()
            .filter(|tool| retained.contains(&tool.id))
            .cloned()
            .collect::<Vec<_>>()
            .into();
    }
    draft
        .validate()
        .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))?;
    Ok(draft)
}

/// Rebuild a `ContextPrepared` message array from an aggregate fold.
///
/// # Precedence: replacement re-bases, additive contributions apply on top
///
/// `StageFold` aggregates each outcome kind separately and keeps no ordering
/// *between* kinds, so a positional "last writer across kinds wins" rule is not
/// representable. This applier therefore defines precedence at the field level:
/// `fold.replacement` substitutes the **base payload** the stage was going to
/// land, and `fold.instructions`/`fold.context` apply on top of whatever base
/// survives — the replacement when there is one, the facade's messages when
/// there is not. Both orders are stable and documented: replacement first, then
/// instructions (as `MessageRole::System`), then context (as
/// `MessageRole::User`), each group in chain order.
///
/// # Placement: additions land before the trailing current user
///
/// When the surviving array ends with a `MessageRole::User` message — the
/// current user message in every facade-prepared context — the fold's
/// instruction and context messages are **inserted immediately before it**,
/// preserving the same trailing-user-last invariant as
/// `exec/context_driver/collect.rs::rebuild_messages` (whose insertion point
/// differs: provider items land after the leading system/developer prefix,
/// before conversation history). This keeps the current
/// user message *last*, which is load-bearing downstream: `before_model_input`
/// (`input.rs`) marks the trailing user structurally protected, and
/// `validate_compaction_result` (`ports/middleware/validate.rs`) requires the
/// last source entry to be a protected user before any `CompactContext` can
/// land. Appending after the user message would make every compaction fail
/// whenever a `prepare_context` addition ran. When the array is empty or its
/// last message is not a user message (e.g. a `Replace` reshaped it), the
/// additions append at the tail — the fold does not invent structure. A
/// `Replace` author is therefore responsible for ending the payload with the
/// current user message: a payload that buries the user turn mid-array leaves
/// no protected trailing user, so `validate_compaction_result` rejects every
/// subsequent `CompactContext` for that turn.
///
/// The alternative — letting a `Replace` from one component discard an
/// `AddContext` from another — was rejected: silently dropping a component's
/// contribution is exactly the failure mode
/// [`MIDDLEWARE_STAGE_UNLANDABLE`] exists to prevent, and a chain whose
/// components disagree that badly is a configuration error the driver cannot
/// resolve by picking a winner.
///
/// Ids for the appended messages come from `sources`, not from `AllocatedIds`:
/// the kernel requires **zero** message ids for `ContextPrepared`
/// (`decide.rs:1131-1133`, tuple `(2, 0, 0, 1, 0, 0)`), exactly as the facade's
/// own `Agent::context_messages` mints them outside the id bag.
///
/// # Errors
///
/// Returns [`MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`] when the rebuilt array would
/// exceed the kernel's `SEMANTIC_ARRAY_MAX_ITEMS` bound (`decide.rs:1195-1202`
/// rejects it), or `middleware_stage_payload_invalid` when the replacement
/// payload is not a message array or an item cannot become a valid message.
pub(crate) fn apply_context_prepared<C: Clock, R: RandomSource>(
    fold: &StageFold,
    base: &[Message],
    sources: &SettlementSources<C, R>,
) -> Result<Arc<[Message]>, RunHandleError> {
    let mut messages = match fold.replacement.as_ref() {
        Some(replacement) => parse_messages(replacement)?,
        None => base.to_vec(),
    };
    if !fold.instructions.is_empty() || !fold.context.is_empty() {
        let insert_at = match messages.last() {
            Some(last) if last.role() == MessageRole::User => messages.len().saturating_sub(1),
            _ => messages.len(),
        };
        let mut added = Vec::with_capacity(fold.instructions.len() + fold.context.len());
        for item in &fold.instructions {
            added.push(message_from_item(item, MessageRole::System, sources)?);
        }
        for item in &fold.context {
            added.push(message_from_item(item, MessageRole::User, sources)?);
        }
        messages.splice(insert_at..insert_at, added);
    }
    if messages.len() > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(stage_error(MIDDLEWARE_STAGE_BOUNDS_EXCEEDED));
    }
    Ok(messages.into())
}

fn message_from_item<C: Clock, R: RandomSource>(
    item: &ContextItem,
    role: MessageRole,
    sources: &SettlementSources<C, R>,
) -> Result<Message, RunHandleError> {
    Message::try_new(
        sources.generate::<MessageTag>()?,
        role,
        item.content.to_vec(),
        sources.now()?,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .map_err(|_| stage_error(MIDDLEWARE_STAGE_PAYLOAD_INVALID))
}
