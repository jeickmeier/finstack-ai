//! Aggregate middleware chain driver.
//!
//! [`StageFold`] is the pure aggregation step: it folds one stage's ordered
//! `Vec<StageOutcome>` (as produced by [`invoke_middleware_stage`]) into a
//! single result, or a stable error when an outcome has nowhere to land.
//! [`StageDriver`] is the handle a caller holds across a run's stage
//! boundaries: the locked chain plus the run's cancellation signal.
//! `crate::stage_settlement` is the only production caller — it applies the
//! fold at the worker's `KernelInput::StageSettled` choke point.
//!
//! The five sections below are the module's contract. They are not background:
//! each one describes a property a caller, a middleware author, or a future
//! maintainer can violate by accident.
//!
//! # 1. The aggregate-fold invariant
//!
//! The kernel models the middleware boundary as exactly **one**
//! `ReducerStageOutcome` per `(cycle, stage)` cursor
//! ([`finstack_ai_kernel::StageOutcomeRecorded`]). A stage's chain may hold N
//! components producing N [`StageOutcome`]s; those fold into that single
//! aggregate, and the aggregate reaches the kernel **only** through
//! `KernelInput::StageSettled`. Nothing else crosses the boundary: no
//! `KernelInput` commits an `EffectKind::Middleware` `EffectRequested`, so an
//! individual invocation is never a committed effect and never appears in the
//! journal on its own.
//!
//! That is why the kernel needed no change to gain a middleware chain, and it
//! is the constraint every other limitation here follows from. An outcome that
//! cannot be expressed as part of one aggregate `ReducerStageOutcome` has
//! nowhere to go, and the fold refuses it with
//! [`MIDDLEWARE_STAGE_UNLANDABLE`] rather than dropping it.
//!
//! # 2. Replay safety is a contract, not a type
//!
//! Because invocations are never journaled, there is nothing to replay them
//! *from*. On recovery, a cursor whose `StageOutcomeRecorded` is absent re-runs
//! its **entire** chain from component zero — including components that already
//! ran and returned before the crash.
//!
//! Nothing in the type system prevents a middleware from sending an email,
//! charging a card, or appending to an external log inside `invoke`. Such a
//! component **will repeat that action** after a crash. The
//! `InvocationRecovery::NonRepeatable` marker on a descriptor does not save it:
//! nothing in this design reads that marker at a stage boundary. Middleware
//! must be pure with respect to external state. This is a documented
//! obligation on the implementer, enforced by nothing.
//!
//! # 3. The derived effect id is not a real effect
//!
//! [`crate::RunCallContext`] carries a non-`Option` `effect_id`, but at a stage
//! boundary there is no committed effect to name (section 1). The driver
//! therefore fabricates one with [`derived_stage_effect_id`]: deterministic in
//! `(locator, cycle, stage)`, domain-separated so it cannot collide with a real
//! `UuidV7` effect id. It is a **correlation id**, not a committed identity.
//!
//! This escapes the crate. `sanitize_call_context`
//! (`plugins/finstack-ai-wit/src/mapping.rs`) copies `RunCallContext.effect_id`
//! verbatim into the guest-visible WIT `call-context`, so a plugin author or
//! host that treats the value a middleware observes as a journal key — looks it
//! up, correlates it against `EffectRequested`, keys idempotency on it — is
//! wrong, and will find nothing. The same applies to any future in-tree code
//! tempted to resolve it against the journal.
//!
//! # 4. First terminal wins, so resolved order is load-bearing
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
//! # 5. Known limitations of the aggregate design
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

mod context;
mod driver;
mod fold;

#[cfg(test)]
mod tests;

pub use context::{MiddlewareStageContext, derived_stage_effect_id, invoke_middleware_stage};
pub use driver::StageDriver;
pub use fold::{
    MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE, StageFold, StageTerminal,
};
