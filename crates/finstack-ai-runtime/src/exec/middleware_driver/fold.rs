use crate::ports::middleware::{MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE};
use std::collections::BTreeSet;

use finstack_ai_kernel::{ErrorDescriptor, RawJson, RetryDirective, Stage, ToolId};

use crate::context::ContextItem;
use crate::middleware::{CompactionResult, MiddlewareError, StageOutcome};
use crate::ports::model::ModelRequestDraft;

/// First terminal outcome in an ordered stage chain, short-circuiting every
/// component after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageTerminal {
    /// The stage fails with a safe descriptor. Landable at every stage.
    Fail(Box<ErrorDescriptor>),
    /// The stage requests a bounded semantic retry. Landable only at
    /// `Stage::BeforeFinalize` — see [`MIDDLEWARE_STAGE_UNLANDABLE`].
    Retry(RetryDirective),
}

/// Pure left-to-right fold of one stage's ordered [`StageOutcome`] chain.
///
/// Aggregates the ordered results of a stage's component chain into exactly
/// one result, matching the kernel's model of the middleware boundary as one
/// `ReducerStageOutcome` per `(cycle, stage)` cursor. Construct with
/// [`StageFold::accumulate`].
///
/// This fold assumes every outcome already passed
/// [`validate_stage_outcome`] for its own component descriptor (as
/// the stage-settlement driver guarantees); it does not re-check
/// per-component/role legality such as the single-compactor rule. What it
/// does check, independently, is whether the *aggregate* stage/outcome
/// combination has anywhere to land in the kernel, and whether the fold's own
/// accumulated content would exceed a kernel-enforced bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageFold {
    /// `AddInstructions` contributions, concatenated in chain order so a
    /// later component's contribution sits after an earlier one's.
    pub instructions: Vec<ContextItem>,
    /// `AddContext` contributions, concatenated in chain order.
    pub context: Vec<ContextItem>,
    /// `FilterTools` narrowing, intersected across every component that
    /// emitted one — narrowing is monotone, so the result does not depend on
    /// component order. `None` means no component narrowed the tool set.
    pub retained_tools: Option<BTreeSet<ToolId>>,
    /// The last `Replace` value in chain order (`Replace` substitutes).
    pub replacement: Option<RawJson>,
    /// The unique compactor's `CompactContext` result, if any. At most one
    /// component may hold the `ContextCompactor` role in a resolved chain, so
    /// at most one `CompactContext` outcome can appear in a stage's chain.
    pub compaction: Option<Box<CompactionResult>>,
    /// The first `Fail` or `Retry` in chain order. Once set, no later
    /// component in the chain is folded.
    pub terminal: Option<StageTerminal>,
}

impl StageFold {
    /// Fold `outcomes` for `stage` left to right into one aggregate result.
    ///
    /// `Continue` is a no-op. `AddInstructions`/`AddContext` concatenate in
    /// chain order. `FilterTools` intersects. `Replace` substitutes (last
    /// writer wins). The first `Fail` or `Retry` becomes the [`StageTerminal`]
    /// and short-circuits every later component — the rest of the chain is
    /// never inspected, so a later outcome's own legality is irrelevant once
    /// an earlier terminal has won.
    ///
    /// # Errors
    ///
    /// Returns [`MIDDLEWARE_STAGE_UNLANDABLE`] the moment it encounters an
    /// outcome with no kernel landing path at `stage`: `Suspend`, `Complete`,
    /// `RequestInteraction`, and `RequestCompactionModel` unconditionally;
    /// `Retry` and `Replace` at any stage other than the ones the kernel
    /// admits them at; `CompactContext` outside `Stage::BeforeModel`.
    /// Also returns that code when a `BeforeModel` aggregate carries both a
    /// `Replace` and a `CompactContext`: each outcome is individually
    /// landable, but landing both would silently discard one projection.
    /// The fold does not merge leaves, wrap compactors, or rebase one
    /// projection onto the other.
    ///
    /// Returns [`MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`] when the accumulated
    /// `instructions` and `context` (plus, at `Stage::BeforeModel`, any
    /// `CompactContext` replacement messages) would exceed the kernel's
    /// array bound for the landed outcome at `Stage::PrepareContext` or
    /// `Stage::BeforeModel`. Skipped entirely once a terminal has won, since
    /// a terminal outcome never lands as `ContextPrepared`/`ModelRequestPrepared`.
    pub fn accumulate(stage: Stage, outcomes: &[StageOutcome]) -> Result<Self, MiddlewareError> {
        let mut fold = Self::default();
        for outcome in outcomes {
            match outcome {
                StageOutcome::Continue => {}
                StageOutcome::AddInstructions(items) => {
                    fold.instructions.extend(items.iter().cloned());
                }
                StageOutcome::AddContext(items) => {
                    fold.context.extend(items.iter().cloned());
                }
                StageOutcome::FilterTools(ids) => {
                    let incoming: BTreeSet<ToolId> = ids.iter().cloned().collect();
                    fold.retained_tools = Some(match fold.retained_tools.take() {
                        Some(existing) => existing.intersection(&incoming).cloned().collect(),
                        None => incoming,
                    });
                }
                StageOutcome::Replace(raw) => {
                    if matches!(stage, Stage::PrepareContext | Stage::BeforeModel) {
                        fold.replacement = Some(raw.clone());
                    } else {
                        return Err(unlandable(
                            "middleware Replace has no kernel landing shape at this stage",
                        ));
                    }
                }
                StageOutcome::CompactContext(result) => {
                    if stage == Stage::BeforeModel {
                        fold.compaction = Some(result.clone());
                    } else {
                        return Err(unlandable(
                            "middleware CompactContext has no kernel landing shape at this stage",
                        ));
                    }
                }
                StageOutcome::RequestCompactionModel(_) => {
                    return Err(unlandable(
                        "middleware RequestCompactionModel has no StageSettled landing path",
                    ));
                }
                StageOutcome::RequestInteraction(_) => {
                    return Err(unlandable(
                        "middleware RequestInteraction does not consume a stage cursor",
                    ));
                }
                StageOutcome::Suspend(_) => {
                    return Err(unlandable(
                        "middleware Suspend has no StageSettled landing path",
                    ));
                }
                StageOutcome::Complete(_) => {
                    return Err(unlandable(
                        "middleware Complete has no ReducerStageOutcome peer",
                    ));
                }
                StageOutcome::Retry(directive) => {
                    if stage == Stage::BeforeFinalize {
                        fold.terminal = Some(StageTerminal::Retry(directive.clone()));
                    } else {
                        return Err(unlandable(
                            "the kernel admits ReducerStageOutcome::Retry only at BeforeFinalize",
                        ));
                    }
                    break;
                }
                StageOutcome::Fail(descriptor) => {
                    fold.terminal = Some(StageTerminal::Fail(descriptor.clone()));
                    break;
                }
            }
        }
        if fold.replacement.is_some() && fold.compaction.is_some() {
            return Err(unlandable(
                "BeforeModel Replace and CompactContext cannot both claim the model projection",
            ));
        }
        fold.check_bounds(stage)?;
        Ok(fold)
    }

    /// Whether this fold leaves the stage's base outcome unperturbed.
    ///
    /// True exactly when no component contributed anything: no added
    /// instructions or context, no tool narrowing, no replacement or
    /// compaction, and no terminal. A caller can use this to skip landing an
    /// aggregate outcome entirely and proceed as if no middleware ran.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.instructions.is_empty()
            && self.context.is_empty()
            && self.retained_tools.is_none()
            && self.replacement.is_none()
            && self.compaction.is_none()
            && self.terminal.is_none()
    }

    /// Bounds-check the accumulated additions once folding is complete.
    ///
    /// A no-op once a terminal has won: a `Fail`/`Retry` outcome never lands
    /// as `ContextPrepared`/`ModelRequestPrepared`, so the partial
    /// instructions/context accumulated before the short-circuit are moot.
    fn check_bounds(&self, stage: Stage) -> Result<(), MiddlewareError> {
        if self.terminal.is_some() {
            return Ok(());
        }
        let added = self.instructions.len() + self.context.len();
        match stage {
            Stage::PrepareContext if added > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS => {
                Err(bounds_exceeded(
                    "aggregate PrepareContext instructions/context exceed the semantic array bound",
                ))
            }
            Stage::BeforeModel => {
                let compacted = self.compaction.as_ref().map_or(0, |result| {
                    result.replacement_messages.len() + result.derived_summaries.len()
                });
                if added + compacted > ModelRequestDraft::MAX_MESSAGES {
                    Err(bounds_exceeded(
                        "aggregate BeforeModel additions exceed the model request message bound",
                    ))
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }
}

fn unlandable(reason: &'static str) -> MiddlewareError {
    MiddlewareError::stable(MIDDLEWARE_STAGE_UNLANDABLE, reason)
}

fn bounds_exceeded(reason: &'static str) -> MiddlewareError {
    MiddlewareError::stable(MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, reason)
}
