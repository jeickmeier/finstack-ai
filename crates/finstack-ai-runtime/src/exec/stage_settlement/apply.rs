use std::sync::Arc;

use finstack_ai_kernel::{
    Message, MessageRole, MessageTag, Metadata, ProviderIds, ReducerStageOutcome,
    SEMANTIC_ARRAY_MAX_ITEMS, Stage, StageCursor,
};

use crate::context::ContextItem;
use crate::middleware_driver::{
    MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE, StageFold, StageTerminal,
};
use crate::model::ModelRequestDraft;
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;
use crate::{Clock, RandomSource};

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
/// # Precedence: replacement re-bases, then compaction, then additions, then
/// narrowing
///
/// The same field-level rule [`apply_context_prepared`] documents, extended to
/// the draft's two independent axes. [`StageFold`] keeps no ordering *between*
/// outcome kinds, so a positional "last writer wins" is not representable; this
/// applier therefore fixes a stable, documented order:
///
/// 1. `Replace` substitutes the **whole draft** — at `BeforeModel` the stage's
///    payload is the request, so a `Replace` there re-bases model, tools,
///    output, settings and limits together, not just the messages.
/// 2. `CompactContext` replaces the **message projection** of whatever base
///    survived, and nothing else. At most one component can produce one (the
///    single-compactor rule, `middleware.rs:650-655`).
///
///    Two gaps live in this step. Both are unobservable today, because
///    `CompactContext` cannot reach here at all (see the module docs), and both
///    become live the moment the `ContextProvider` port is wired — whoever does
///    that work owns them:
///
///    - Its `derived_summaries` and `checkpoint` have no landing in a
///      `ModelRequestPrepared` and are **dropped**. That is a real gap, not a
///      design choice: a working compactor needs its summary in the projection.
///    - A fold carrying **both** a `Replace` and a `CompactContext` applies a
///      projection that was validated against the *base* draft's
///      `source_entries` — assembled by [`before_model_input`], checked at
///      `middleware.rs:1069-1083` — on top of the *replaced* draft, which step 1
///      has already substituted. The
///      compactor's guarantees — protected-entry preservation, tool-pair
///      atomicity, source-digest integrity — are all stated against the array it
///      was shown, and the array it lands on is a different one. Two components
///      are required for this (the single compactor cannot also `Replace`), so a
///      chain with a compactor plus any `BeforeModel` `Replace` reaches it.
///
///    Neither is fixed ahead of the `ContextProvider` work, because a landing
///    rule for either one is untestable through the port until a protected
///    entry can exist. They belong with the `protected` work, not ahead of it.
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
/// Returns [`MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`] when the rebuilt message array
/// would exceed [`ModelRequestDraft::MAX_MESSAGES`], and
/// `middleware_stage_payload_invalid` when a `Replace` payload is not a model
/// draft, an added item cannot become a valid message, or the rebuilt draft
/// fails its own `validate` (duplicate tool name, oversized collection).
pub(super) fn apply_model_draft<C: Clock, R: RandomSource>(
    fold: &StageFold,
    base: ModelRequestDraft,
    sources: &SettlementSources<C, R>,
) -> Result<ModelRequestDraft, RunHandleError> {
    let mut draft = match fold.replacement.as_ref() {
        Some(replacement) => parse_draft(replacement)?,
        None => base,
    };
    if let Some(compaction) = fold.compaction.as_ref() {
        draft.messages = compaction.replacement_messages.clone();
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
/// land, and `fold.instructions`/`fold.context` are appended to whatever base
/// survives — the replacement when there is one, the facade's messages when
/// there is not. Both orders are stable and documented: replacement first, then
/// instructions (as `MessageRole::System`), then context (as
/// `MessageRole::User`), each group in chain order.
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
    for item in &fold.instructions {
        messages.push(message_from_item(item, MessageRole::System, sources)?);
    }
    for item in &fold.context {
        messages.push(message_from_item(item, MessageRole::User, sources)?);
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
