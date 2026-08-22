//! Aggregate middleware chain driver.
//!
//! [`StageFold`] is the pure aggregation step: it folds one stage's ordered
//! `Vec<StageOutcome>` into a single result, or a stable error when an outcome
//! has nowhere to land.
//! [`StageDriver`] is the handle a caller holds across a run's stage
//! boundaries: the locked chain plus the run's cancellation signal.
//! `crate::stage_settlement` is the only production caller — it applies the
//! fold at the worker's `KernelInput::StageSettled` choke point.
//!
//! # Durable invocation and aggregate settlement
//!
//! The kernel models the middleware boundary as exactly **one**
//! `ReducerStageOutcome` per `(cycle, stage)` cursor
//! ([`finstack_ai_kernel::StageOutcomeRecorded`]). A stage's chain may hold N
//! components producing N [`StageOutcome`]s. Each component invocation first
//! commits an `EffectKind::Middleware` request, executes only after that
//! commit, and commits its terminal effect settlement. The ordered outcomes
//! then fold into the one aggregate stage settlement.
//!
//! Recovery reuses a pending request's committed identity or replays a matching
//! completed outcome. A missing terminal settlement invokes only under the
//! descriptor's `RecomputeSafe` contract. An outcome that cannot be expressed
//! by the aggregate is refused with [`MIDDLEWARE_STAGE_UNLANDABLE`].
//!
//! # First terminal wins, so resolved order is load-bearing
//!
//! [`StageFold::accumulate`] short-circuits on the first `Fail` or `Retry`:
//! when component 2 of 4 fails, components 3 and 4 **never run**, and their
//! contributions never exist. Additive outcomes commute (`AddInstructions` and
//! `AddContext` concatenate, `FilterTools` intersects), but the terminal cut
//! does not.
//!
//! Resolved order is therefore semantics, not presentation: [`OrderTier`],
//! then `order.before` / `order.after`, then registration index
//! (`ResolvedMiddlewareChain::try_new`). Reordering a chain — including a
//! reordering that looks cosmetic, such as registering a new observational
//! component earlier — can change which components run at all. A component
//! that must observe every stage must be ordered before anything that can fail.
//!
//! [`OrderTier`]: crate::middleware::OrderTier
//!
//! # Known limitations of the aggregate design
//!
//! ## Outcomes with no landing path
//!
//! Four [`StageOutcome`] variants a component may legally return under the
//! stage matrix have no aggregate landing and are refused with
//! [`MIDDLEWARE_STAGE_UNLANDABLE`]:
//!
//! - `Suspend` and `Complete` — no `ReducerStageOutcome` peer exists at any
//!   stage. Deliberate: parking or completing a run from inside a stage fold
//!   would need a second kernel input the fold has no way to emit.
//! - `RequestInteraction` — an interaction does not consume a stage cursor, so
//!   there is no single settlement that both opens the interaction and settles
//!   the stage. No in-tree component returns it any more:
//!   `finstack-ai-middleware-verify` dropped its interaction mode when it was
//!   promoted to a battery, so only an out-of-tree component can reach this
//!   refusal.
//! - `RequestCompactionModel` — see below.
//!
//! ## Compaction landing
//!
//! `finstack-ai-middleware-compaction` is one of only two shipping middleware
//! leaves. Its two outcomes diverge:
//!
//! - Deterministic strategies return `CompactContext`, which this fold accepts
//!   at `BeforeModel` after `validate_compaction_result` sees a protected
//!   trailing user. That bit is authoritative-from-the-context-port plus the
//!   structural rule (system/developer and the current user). The production
//!   `ContextProvider` driver is the caller of `CommittedContextCall::try_new`
//!   and `assemble_context`.
//! - Its `summarize` strategy returns `RequestCompactionModel`. The
//!   runtime-owned compaction phase (ADR-042) intercepts that outcome
//!   before [`StageFold::accumulate`], commits a child model effect, and
//!   re-enters with `compaction_resume`. If the outcome still reaches
//!   this fold, it stays [`MIDDLEWARE_STAGE_UNLANDABLE`].
//!
//! One residual gap sits behind a landed `CompactContext` and is documented at
//! `stage_settlement::apply_model_draft`: a landed `CompactionResult` still
//! drops its `checkpoint` field at settlement (summarize `derived_summaries`
//! append as user messages). A `BeforeModel` aggregate that carries both a
//! `Replace` and a `CompactContext` is rejected by [`StageFold::accumulate`]
//! as [`MIDDLEWARE_STAGE_UNLANDABLE`] rather than silently overwriting one
//! projection with the other. Sliding-window compaction does not use
//! `derived_summaries` or `checkpoint`.
//!
//! # Crate-private surface
//!
//! This module is `pub(crate)`. Callers inside the runtime reach it as
//! `crate::middleware_driver`; it is not a crate-root public export.

mod driver;
mod fold;

#[cfg(test)]
mod tests;

pub use driver::StageDriver;
pub use fold::{StageFold, StageTerminal};
