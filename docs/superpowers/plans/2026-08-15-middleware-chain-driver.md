# Middleware Chain Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the resolved `ResolvedMiddlewareChain` actually run at all seven
stages, folding each stage's ordered component outcomes into the single
aggregate `ReducerStageOutcome` the kernel already accepts, and ship a first
real producer for every dead `StageOutcome` variant that has a landing path.

**Architecture:** Aggregate fold, no kernel change. `KernelInput::StageSettled
{ cursor, outcome }` and `StageOutcomeRecorded { cursor, disposition,
settlement_digest }` already model the middleware boundary as exactly one
outcome per `(cycle, stage)`. The driver runs the chain in-process immediately
before each existing `submit_stage` call, applies the ordered outcomes to that
stage's locally-computed payload, and submits the folded result through the
existing path. Per-invocation effects are **not** committed.

**Tech Stack:** Rust 1.97.1, tokio, `finstack-ai-kernel`, `finstack-ai-runtime`,
`finstack-ai` facade, `finstack-ai-test` conformance suite.

## Global Constraints

- **No kernel change.** `crates/finstack-ai-kernel/` is not modified by this
  plan. If a task appears to require a `KernelInput` variant, a `KernelState`
  field, or an `apply_effect_*` branch, stop and escalate — the design decision
  was explicitly aggregate-fold.
- **No new public re-exports.** `tools/compat/public_items.py` scrapes `pub use`
  blocks from exactly `crates/finstack-ai-kernel/src/lib.rs`,
  `crates/finstack-ai-runtime/src/lib.rs`, and `crates/finstack-ai/src/lib.rs`.
  Removals and renames fail `public_item_lists_reject_renames`
  (`crates/finstack-ai-test/tests/breaking_change.rs:113`), which runs inside
  `mise run test`. Additions are invisible to the check but still require an
  ADR under `docs/implementation/compatibility-governance.md`; this plan adds
  none.
- **No new `StageOutcome` variant.** All twelve already exist. Two of them
  (`Suspend`, `Complete`) are deliberately left without a producer — see Task 11.
- **At most one `MiddlewareRole::ContextCompactor`** per resolved agent, already
  enforced at `crates/finstack-ai-runtime/src/middleware.rs:612`. Never register
  a second compactor; new strategies are configuration inside the existing one.
- **Middleware must be replay-safe.** Under aggregate fold the chain is re-run
  on recovery when no `StageOutcomeRecorded` exists for the cursor. Middleware
  implementations must be pure with respect to external state. This is a new
  documented invariant (Task 12).
- `AddInstructions` / `AddContext` are legal **only** at `PrepareContext` and
  `BeforeModel` (`crates/finstack-ai-runtime/src/middleware.rs:927`).
- `CompactContext` / `RequestCompactionModel` are legal **only** at
  `BeforeModel` and **only** for the unique compactor role (`middleware.rs:933`).
- `FilterTools` is legal **only** at `BeforeModel` and `BeforeToolBatch`
  (`middleware.rs:930`).
- Every outcome must pass `validate_stage_outcome(descriptor, input, outcome)`
  (`middleware.rs:912`) before the fold consumes it. The driver never coerces a
  rejected outcome; it returns the stable code `middleware_outcome_not_allowed`.
- Run `mise run ci` before opening a pull request (`CONTRIBUTING.md:18`). It is
  `mise run check` + `mise run test` + `mise run check-wasm` (`mise.toml:311`).
- Do not commit unless the user explicitly requests a commit.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/finstack-ai-runtime/src/middleware_driver.rs` (create) | The chain driver. One entry point per stage payload shape. Owns invocation ordering, matrix validation, and the fold. |
| `crates/finstack-ai-runtime/src/middleware.rs` (modify) | Make `stage_name` / `parse_stage` `pub(crate)`; add `StageOutcome::is_terminal_for_fold`. |
| `crates/finstack-ai-runtime/src/lib.rs` (modify) | `mod middleware_driver;` — **module declaration only, no `pub use`**. |
| `crates/finstack-ai-runtime/src/settlement.rs` (modify) | `BeforeToolBatch` stage — the one stage the facade does not own. |
| `crates/finstack-ai-runtime/src/task.rs` (modify) | Thread `Arc<ResolvedMiddlewareChain>` into `RunTaskOwner` so `settlement.rs` can reach it. |
| `crates/finstack-ai/src/agent.rs` (modify) | Six facade-owned stages; `StageIds::for_outcome`. |
| `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs` (modify) | `VerifyDecision::Retry` arm — first `Retry` producer. |
| `crates/finstack-ai/tests/middleware_chain.rs` (create) | End-to-end integration proof that a non-empty chain changes run behavior. |

---

### Task 1: Expose stage naming and add the driver module skeleton

`stage_name` (`middleware.rs:1509`) and `parse_stage` (`middleware.rs:1521`) are
module-private with no visibility modifier, so no other module can build the
`PipelinePosition.stage` string. The driver needs them.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/middleware.rs:1509`, `:1521`
- Create: `crates/finstack-ai-runtime/src/middleware_driver.rs`
- Modify: `crates/finstack-ai-runtime/src/lib.rs`
- Test: `crates/finstack-ai-runtime/src/middleware_driver.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Stage`, `ResolvedMiddlewareChain`, `ResolvedMiddleware`,
  `MiddlewareError`, `StageInput`, `StageOutcome`, `validate_stage_outcome`
- Produces: `pub(crate) fn stage_name(Stage) -> &'static str`,
  `pub(crate) fn parse_stage(&str) -> Option<Stage>`, and
  `pub(crate) struct StageRunContext` used by every later task

- [ ] **Step 1: Write the failing test**

Append to `crates/finstack-ai-runtime/src/middleware_driver.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::Stage;

    #[test]
    fn stage_names_round_trip() {
        for stage in [
            Stage::BeforeRun,
            Stage::PrepareContext,
            Stage::BeforeModel,
            Stage::AfterModel,
            Stage::BeforeToolBatch,
            Stage::AfterToolBatch,
            Stage::BeforeFinalize,
        ] {
            let name = crate::middleware::stage_name(stage);
            assert_eq!(crate::middleware::parse_stage(name), Some(stage));
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai-runtime --locked middleware_driver`
Expected: FAIL — `middleware_driver` module not found, and `stage_name` is
private.

- [ ] **Step 3: Create the module and widen visibility**

Create `crates/finstack-ai-runtime/src/middleware_driver.rs`:

```rust
//! Aggregate middleware chain driver.
//!
//! The kernel models the middleware boundary as exactly one
//! [`ReducerStageOutcome`] per `(cycle, stage)` cursor
//! ([`finstack_ai_kernel::StageOutcomeRecorded`]). This module runs the ordered
//! component chain for a stage in-process, validates each outcome against the
//! stage matrix, and folds the ordered results into that single aggregate
//! outcome. Individual invocations are not committed as effects.
//!
//! Middleware implementations must therefore be pure with respect to external
//! state: on recovery, a stage whose `StageOutcomeRecorded` is absent re-runs
//! its whole chain.

use alloc::sync::Arc;

use finstack_ai_kernel::{Digest, Stage};

use crate::middleware::{
    validate_stage_outcome, MiddlewareError, ResolvedMiddleware, ResolvedMiddlewareChain,
    StageInput, StageOutcome,
};
use crate::model::RunCallContext;

/// Shared identity for one stage's chain invocation.
pub(crate) struct StageRunContext {
    /// Run-scoped call context reused for every component in the stage.
    pub(crate) run: RunCallContext,
    /// Locked chain digest from the resolved agent.
    pub(crate) chain_digest: Digest,
    /// Committed parent effect this stage runs under, used to validate any
    /// child compaction-model effect (Task 9).
    pub(crate) parent: EffectRequested,
}

impl StageRunContext {
    /// Per-component invocation context.
    pub(crate) fn middleware_context(
        &self,
        index: usize,
    ) -> Result<crate::middleware::MiddlewareContext, MiddlewareError> {
        Ok(crate::middleware::MiddlewareContext {
            run: self.run.clone(),
            chain_digest: self.chain_digest,
            chain_index: u32::try_from(index).map_err(|_| {
                MiddlewareError::stable(
                    crate::middleware::MIDDLEWARE_RESOLUTION_INVALID,
                    "middleware chain index exceeds u32",
                )
            })?,
            compaction_resume: None,
        })
    }
}

/// Invoke every component registered for `stage`, in resolved order.
///
/// Each outcome is validated against the stage matrix before it is returned.
/// The caller folds the ordered results into one `ReducerStageOutcome`.
///
/// # Errors
///
/// Returns the component's own `MiddlewareError`, or a stable
/// `middleware_outcome_not_allowed` when an outcome fails the matrix.
pub(crate) async fn invoke_stage(
    chain: &ResolvedMiddlewareChain,
    ctx: &StageRunContext,
    input: StageInput,
) -> Result<Vec<StageOutcome>, MiddlewareError> {
    let stage = input.stage();
    let components: &[ResolvedMiddleware] = chain.stage(stage);
    let mut outcomes = Vec::with_capacity(components.len());
    for (index, resolved) in components.iter().enumerate() {
        let mw_ctx = ctx.middleware_context(index)?;
        let outcome = resolved.middleware.invoke(mw_ctx, input.clone()).await?;
        validate_stage_outcome(&resolved.descriptor, &input, &outcome)?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}
```

In `crates/finstack-ai-runtime/src/middleware.rs`, change line 1509 from
`fn stage_name(` to `pub(crate) fn stage_name(` and line 1521 from
`fn parse_stage(` to `pub(crate) fn parse_stage(`.

In `crates/finstack-ai-runtime/src/lib.rs`, add `mod middleware_driver;`
immediately after the existing `mod middleware;` line. Do **not** add a
`pub use` — that would extend the frozen public surface.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p finstack-ai-runtime --locked middleware_driver`
Expected: PASS

- [ ] **Step 5: Verify no public surface moved**

Run: `uv run --no-project python tools/compat/public_items.py --check`
Expected: exit 0, no missing items reported.

---

### Task 2: Shared facade helpers and `StageIds` selection

Every stage task from Task 3 onward calls the same small set of facade helpers.
Define them all here, in one reviewable place, so no later task invents a
name its neighbor spells differently.

`submit_stage` (`crates/finstack-ai/src/agent.rs:2315`) takes a `StageIds` count
block that pre-allocates the ids the reducer will mint. Every call site today
hardcodes it (`StageIds::continued()`, `::context()`, `::finalize()`). Once the
fold can change *which* `ReducerStageOutcome` is produced, a hardcoded count is
wrong — folding `FinalizeAccepted` into `Retry` needs `StageIds::retry()`
(3 records, 1 event, 1 effect), not `StageIds::finalize()` (2 records, 1 event).

**Files:**
- Modify: `crates/finstack-ai/src/agent.rs:2266-2313`
- Test: `crates/finstack-ai/src/agent.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `StageRunContext` (Task 1), `StageIds` unchanged
- Produces — every later task uses these exact names and signatures:
  - `const fn StageIds::for_outcome(&ReducerStageOutcome) -> Self`
  - `fn Agent::stage_run_context(&self, cycle: u64, stage: Stage) -> Result<StageRunContext, AgentRunError>`
  - `fn AgentRunError::from_middleware(MiddlewareError) -> Self`
  - `fn canonical_messages(&[Message]) -> Result<Vec<u8>, AgentRunError>`
  - `fn canonical_message(&Message) -> Result<Vec<u8>, AgentRunError>`
  - `fn parse_messages(&RawJson) -> Result<Vec<Message>, AgentRunError>`
  - `fn canonical_terminal_candidate(&KernelState) -> Result<Vec<u8>, AgentRunError>`
  - `fn Agent::compactor_descriptor(&self) -> Result<&MiddlewareDescriptor, AgentRunError>`
  - `fn Agent::model_context_profile(&self) -> Result<ModelContextProfileRef, AgentRunError>`
  - `async fn Agent::request_interaction(&self, &RunHandle, InteractionRequest) -> Result<(), AgentRunError>`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn stage_ids_match_outcome_shape() {
    use finstack_ai_kernel::{Digest, ErrorCategory, ErrorDescriptor, ReducerStageOutcome,
        RetryClassification, RetryDirective};

    let retry = ReducerStageOutcome::Retry(
        RetryDirective::try_new(
            RetryClassification::Validation,
            Digest::raw_json(b"probe"),
            None,
        )
        .expect("directive"),
    );
    assert_eq!(StageIds::for_outcome(&retry), StageIds::retry());
    assert_eq!(
        StageIds::for_outcome(&ReducerStageOutcome::FinalizeAccepted),
        StageIds::finalize()
    );
    assert_eq!(
        StageIds::for_outcome(&ReducerStageOutcome::Continue),
        StageIds::continued()
    );
    let fail = ReducerStageOutcome::Fail(ErrorDescriptor::new(
        "probe_failed",
        "probe",
        ErrorCategory::Validation,
        false,
    ));
    assert_eq!(StageIds::for_outcome(&fail), StageIds::finalize());
}
```

Add `#[derive(Debug, PartialEq, Eq)]` to `struct StageIds` at `agent.rs:2266`
so `assert_eq!` compiles.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai --locked stage_ids_match_outcome_shape`
Expected: FAIL — no function `for_outcome`.

- [ ] **Step 3: Add the selector**

Insert into `impl StageIds` in `crates/finstack-ai/src/agent.rs`, after
`const fn retry()`:

```rust
    /// Id counts required for one folded aggregate outcome.
    ///
    /// The fold may change which `ReducerStageOutcome` a stage produces, so the
    /// count block must be derived from the final outcome rather than from the
    /// stage's default shape.
    const fn for_outcome(outcome: &ReducerStageOutcome) -> Self {
        match outcome {
            ReducerStageOutcome::Continue => Self::continued(),
            ReducerStageOutcome::ContextPrepared { .. } => Self::context(),
            ReducerStageOutcome::ModelRequestPrepared { .. } => Self::model_request(),
            ReducerStageOutcome::ToolBatchPrepared { .. } => Self::new(2, 1, 0, 0, 0, 0),
            ReducerStageOutcome::FinalizeAccepted
            | ReducerStageOutcome::ContinueModel { .. }
            | ReducerStageOutcome::Fail(_) => Self::finalize(),
            ReducerStageOutcome::Retry(_) => Self::retry(),
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p finstack-ai --locked stage_ids_match_outcome_shape`
Expected: PASS

- [ ] **Step 5: Add the shared helpers**

Add to `impl Agent` in `crates/finstack-ai/src/agent.rs`:

```rust
    /// Build the shared per-stage chain identity.
    ///
    /// No per-invocation effect is committed under the aggregate design, so
    /// `effect_id` is a deterministic correlation id derived from the chain
    /// digest and the stage cursor — not a journaled effect identity. It is
    /// stable across replay because every input is stable.
    fn stage_run_context(
        &self,
        cycle: u64,
        stage: Stage,
    ) -> Result<StageRunContext, AgentRunError> {
        let chain_digest = self.resolved.run_plan().middleware_chain().digest();
        let mut seed = Vec::with_capacity(64);
        seed.extend_from_slice(chain_digest.as_bytes());
        seed.extend_from_slice(&cycle.to_be_bytes());
        seed.extend_from_slice(crate::middleware_stage_name(stage).as_bytes());
        Ok(StageRunContext {
            run: self.run_call_context(EffectId::from_digest(Digest::raw_json(&seed)))?,
            chain_digest,
            parent: self.current_effect_requested()?,
        })
    }

    /// The unique compactor's frozen descriptor.
    fn compactor_descriptor(&self) -> Result<&MiddlewareDescriptor, AgentRunError> {
        self.resolved
            .run_plan()
            .middleware_chain()
            .stage(Stage::BeforeModel)
            .iter()
            .find(|resolved| {
                matches!(resolved.descriptor.role, MiddlewareRole::ContextCompactor { .. })
            })
            .map(|resolved| &resolved.descriptor)
            .ok_or_else(|| {
                AgentRunError::runtime_message("compact_context returned without a compactor role")
            })
    }
```

Add the free functions next to `submit_stage`:

```rust
/// Map a port-level middleware error onto the facade's run error, preserving
/// the component's stable code.
impl AgentRunError {
    fn from_middleware(error: MiddlewareError) -> Self {
        Self::runtime_message(&format!("{}: {error}", error.code()))
    }
}

/// Canonical bytes for an ordered message array.
fn canonical_messages(messages: &[Message]) -> Result<Vec<u8>, AgentRunError> {
    serde_json::to_vec(messages)
        .map_err(|error| AgentRunError::runtime_message(&format!("messages canonicalize: {error}")))
}

/// Canonical bytes for one message.
fn canonical_message(message: &Message) -> Result<Vec<u8>, AgentRunError> {
    serde_json::to_vec(message)
        .map_err(|error| AgentRunError::runtime_message(&format!("message canonicalize: {error}")))
}

/// Parse a `Replace` payload back into an ordered message array.
fn parse_messages(value: &RawJson) -> Result<Vec<Message>, AgentRunError> {
    serde_json::from_slice(value.as_bytes()).map_err(|error| {
        AgentRunError::runtime_message(&format!("replace payload is not a message array: {error}"))
    })
}

/// Canonical bytes for the terminal candidate before any terminal record exists.
fn canonical_terminal_candidate(state: &KernelState) -> Result<Vec<u8>, AgentRunError> {
    let candidate = state
        .terminal_candidate
        .as_ref()
        .ok_or_else(|| AgentRunError::runtime_message("before_finalize has no terminal candidate"))?;
    serde_json::to_vec(candidate)
        .map_err(|error| AgentRunError::runtime_message(&format!("candidate canonicalize: {error}")))
}
```

`model_context_profile` and `request_interaction` already exist on `Agent` —
grep `agent.rs` for `fn model_context_profile` and `fn request_interaction` and
reuse them. If either is spelled differently in the current source, rename the
call sites in Tasks 5 and 6 to match rather than adding a duplicate.

Export the stage-name shim once, in `crates/finstack-ai-runtime/src/lib.rs`,
alongside the Task 3 shim:

```rust
#[doc(hidden)]
pub use crate::middleware::stage_name as middleware_stage_name;
```

- [ ] **Step 6: Confirm the existing suite is unaffected**

Run: `cargo test -p finstack-ai --locked`
Expected: PASS — nothing calls the new helpers yet, so behavior is unchanged.

---

### Task 3: Drive the three `Continue`-only stages

`BeforeRun`, `AfterModel`, and `AfterToolBatch` all settle to
`ReducerStageOutcome::Continue` today and accept only `Continue` or `Fail` from
the reducer (`crates/finstack-ai-kernel/src/reducer/decide.rs:1107`, `:1165`).
They are the smallest correct increment: the chain runs, but the only reachable
behavior changes are `Fail`, `Retry`, and `RequestInteraction`.

`AfterToolBatch` currently has two byte-identical blocks at `agent.rs:843-853`
and `:902-912`. Factor them so they cannot drift.

**Files:**
- Modify: `crates/finstack-ai/src/agent.rs:768-775` (BeforeRun),
  `:843-853` and `:902-912` (AfterToolBatch), `:877-900` (AfterModel)
- Create: `crates/finstack-ai/tests/middleware_chain.rs`

**Interfaces:**
- Consumes: `invoke_stage` and `StageRunContext` (Task 1),
  `StageIds::for_outcome` (Task 2)
- Produces: `async fn Agent::run_simple_stage(&self, handle, cycle, stage,
  value: RawJson) -> Result<ReducerStageOutcome, AgentRunError>` — reused by
  Tasks 6 and 7

- [ ] **Step 1: Write the failing integration test**

Create `crates/finstack-ai/tests/middleware_chain.rs`:

```rust
//! End-to-end proof that a registered middleware chain changes run behavior.

use std::sync::Arc;

use finstack_ai::{Agent, ComponentRef};
use finstack_ai_kernel::{ErrorCategory, ErrorDescriptor, Stage};
use finstack_ai_runtime::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareOrder, MiddlewareRole,
    OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
};
use finstack_ai_test::ScriptedModel;

/// Fails the run at `BeforeRun` with a stable code.
struct DenyBeforeRun;

impl Middleware for DenyBeforeRun {
    fn descriptor(&self) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            component: ComponentRef::new("test.middleware.deny", None),
            stages: StageMask::from_stages([Stage::BeforeRun]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
        }
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, finstack_ai_runtime::MiddlewareError>> {
        Box::pin(async {
            Ok(StageOutcome::Fail(Box::new(ErrorDescriptor::new(
                "denied_by_middleware",
                "before_run middleware denied the run",
                ErrorCategory::Policy,
                false,
            ))))
        })
    }
}

#[tokio::test]
async fn before_run_middleware_fail_stops_the_run() {
    let agent = Agent::builder(Arc::new(ScriptedModel::always_text("unreachable")))
        .middleware(
            ComponentRef::new("test.middleware.deny", None),
            Arc::new(DenyBeforeRun),
        )
        .build()
        .await
        .expect("agent builds");

    let error = agent
        .run("hello")
        .await
        .expect_err("the run must fail at before_run");

    assert!(
        format!("{error}").contains("denied_by_middleware"),
        "expected the middleware's stable code, got: {error}"
    );
}
```

Adjust the `Agent::builder` / `run` calls to the real facade signatures if they
differ — read `crates/finstack-ai/tests/capability_catalog.rs` for the exact
builder shape used in this repo's integration tests.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai --locked --test middleware_chain`
Expected: FAIL — the run succeeds, because nothing invokes the chain.

- [ ] **Step 3: Add the simple-stage driver to the facade**

Add to `impl Agent` in `crates/finstack-ai/src/agent.rs`:

```rust
    /// Run the chain for a stage whose only reducer outcomes are `Continue`,
    /// `Retry`, and `Fail`, and fold the ordered results.
    async fn run_simple_stage(
        &self,
        cycle: u64,
        stage: Stage,
        value: RawJson,
    ) -> Result<ReducerStageOutcome, AgentRunError> {
        let chain = self.resolved.run_plan().middleware_chain();
        if chain.stage(stage).is_empty() {
            return Ok(ReducerStageOutcome::Continue);
        }
        let input = match stage {
            Stage::BeforeRun => StageInput::BeforeRun { value },
            Stage::AfterModel => StageInput::AfterModel { value },
            Stage::AfterToolBatch => StageInput::AfterToolBatch { value },
            other => {
                return Err(AgentRunError::runtime_message(&format!(
                    "run_simple_stage called for {other:?}"
                )))
            }
        };
        let ctx = self.stage_run_context(cycle, stage)?;
        let outcomes = finstack_ai_runtime::middleware_driver_invoke_stage(chain, &ctx, input)
            .await
            .map_err(AgentRunError::from_middleware)?;
        fold_simple_stage(outcomes)
    }
```

Because `middleware_driver` is `pub(crate)` inside `finstack-ai-runtime` and the
facade is a separate crate, expose exactly one `pub(crate)`-equivalent shim.
Add to `crates/finstack-ai-runtime/src/lib.rs`:

```rust
#[doc(hidden)]
pub use crate::middleware_driver::{invoke_stage as middleware_driver_invoke_stage, StageRunContext};
```

`#[doc(hidden)]` keeps it out of rendered docs. It **does** appear in a `pub use`
block, so add both names to
`fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`
in sorted position — additions never fail `compare()`, but the baseline must
stay a superset for the next removal check to be meaningful.

Add the fold next to `submit_stage` in `agent.rs`:

```rust
/// Fold a `Continue`-only stage's ordered outcomes.
///
/// Later components observe earlier ones only through the reducer, so the fold
/// is last-decisive: the first terminal outcome wins and the rest of the chain
/// has already been validated.
fn fold_simple_stage(outcomes: Vec<StageOutcome>) -> Result<ReducerStageOutcome, AgentRunError> {
    for outcome in outcomes {
        match outcome {
            StageOutcome::Continue => {}
            StageOutcome::Fail(descriptor) => {
                return Ok(ReducerStageOutcome::Fail(*descriptor))
            }
            StageOutcome::Retry(directive) => return Ok(ReducerStageOutcome::Retry(directive)),
            other => {
                return Err(AgentRunError::runtime_message(&format!(
                    "outcome {} is not foldable at a continue-only stage",
                    other.variant_name()
                )))
            }
        }
    }
    Ok(ReducerStageOutcome::Continue)
}
```

Add `variant_name` to `StageOutcome` in
`crates/finstack-ai-runtime/src/middleware.rs`:

```rust
    /// Stable variant label for diagnostics.
    #[must_use]
    pub const fn variant_name(&self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Replace(_) => "replace",
            Self::AddInstructions(_) => "add_instructions",
            Self::AddContext(_) => "add_context",
            Self::CompactContext(_) => "compact_context",
            Self::RequestCompactionModel(_) => "request_compaction_model",
            Self::FilterTools(_) => "filter_tools",
            Self::RequestInteraction(_) => "request_interaction",
            Self::Retry(_) => "retry",
            Self::Suspend(_) => "suspend",
            Self::Complete(_) => "complete",
            Self::Fail(_) => "fail",
        }
    }
```

- [ ] **Step 4: Replace the three hardcoded call sites**

At `agent.rs:768-775`, replace the literal `ReducerStageOutcome::Continue` with
the driven outcome:

```rust
        let before_run = self
            .run_simple_stage(0, Stage::BeforeRun, RawJson::parse(b"{}")?)
            .await?;
        submit_stage(
            handle,
            0,
            Stage::BeforeRun,
            before_run.clone(),
            StageIds::for_outcome(&before_run),
        )
        .await?;
```

At `agent.rs:877-900`, inside the `else` branch, replace the literal with the
same shape using `after_model.cycle` and `Stage::AfterModel`, building the
stage value from the last assistant message.

For `AfterToolBatch`, factor `agent.rs:843-853` and `:902-912` into one helper
and call it from both sites:

```rust
    async fn settle_after_tool_batch(
        &self,
        handle: &RunHandle,
        cycle: u64,
    ) -> Result<(), AgentRunError> {
        let outcome = self
            .run_simple_stage(cycle, Stage::AfterToolBatch, RawJson::parse(b"{}")?)
            .await?;
        submit_stage(
            handle,
            cycle,
            Stage::AfterToolBatch,
            outcome.clone(),
            StageIds::for_outcome(&outcome),
        )
        .await
    }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p finstack-ai --locked --test middleware_chain`
Expected: PASS

Run: `cargo test --workspace --locked`
Expected: PASS — the empty-chain fast path in `run_simple_stage` keeps every
existing run behaviorally identical.

---

### Task 4: `PrepareContext` stage with `AddInstructions` and `AddContext`

First producers for two dead variants. The landing is already built:
`ReducerStageOutcome::ContextPrepared { messages }` is admitted at
`PrepareContext` (`decide.rs:1131`) and length-checked (`decide.rs:1187`).

**Files:**
- Modify: `crates/finstack-ai/src/agent.rs:785-793`
- Test: `crates/finstack-ai/tests/middleware_chain.rs`

**Interfaces:**
- Consumes: `invoke_stage` (Task 1), `StageIds::for_outcome` (Task 2)
- Produces: `fn context_item_to_message(&ContextItem) -> Result<Message,
  AgentRunError>` — reused by Task 5's `BeforeModel` fold

- [ ] **Step 1: Write the failing test**

Append to `crates/finstack-ai/tests/middleware_chain.rs` a middleware declaring
`StageMask::from_stages([Stage::PrepareContext])` whose `invoke` returns
`StageOutcome::AddContext(Arc::from([item]))` for one `ContextItem` carrying the
text `"injected-by-middleware"`, plus:

```rust
#[tokio::test]
async fn prepare_context_middleware_injects_context() {
    let model = Arc::new(ScriptedModel::echoing_last_user_text());
    let agent = Agent::builder(model)
        .middleware(
            ComponentRef::new("test.middleware.inject", None),
            Arc::new(InjectContext),
        )
        .build()
        .await
        .expect("agent builds");

    let output = agent.run("hello").await.expect("run completes");

    assert!(
        output.contains("injected-by-middleware"),
        "middleware context never reached the model: {output}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai --locked --test middleware_chain prepare_context`
Expected: FAIL — the injected text never reaches the model.

- [ ] **Step 3: Implement the stage and the fold**

Replace `agent.rs:785-793` with:

```rust
            let mut messages = self.context_messages(&request.input, &state.messages)?;
            let chain = self.resolved.run_plan().middleware_chain();
            let outcome = if chain.stage(Stage::PrepareContext).is_empty() {
                ReducerStageOutcome::ContextPrepared {
                    messages: messages.into(),
                }
            } else {
                let value = RawJson::parse(&canonical_messages(&messages)?)?;
                let ctx = self.stage_run_context(state.cycle, Stage::PrepareContext)?;
                let outcomes = finstack_ai_runtime::middleware_driver_invoke_stage(
                    chain,
                    &ctx,
                    StageInput::PrepareContext { value },
                )
                .await
                .map_err(AgentRunError::from_middleware)?;
                self.fold_prepare_context(&mut messages, outcomes)?
            };
            submit_stage(
                handle,
                state.cycle,
                Stage::PrepareContext,
                outcome.clone(),
                StageIds::for_outcome(&outcome),
            )
            .await?;
```

Add the fold to `impl Agent`:

```rust
    /// Fold `PrepareContext` outcomes into one `ContextPrepared`.
    ///
    /// `AddInstructions` and `AddContext` append in chain order, so a later
    /// component's contribution sits after an earlier one's. `Replace`
    /// substitutes the whole message array.
    fn fold_prepare_context(
        &self,
        messages: &mut Vec<Message>,
        outcomes: Vec<StageOutcome>,
    ) -> Result<ReducerStageOutcome, AgentRunError> {
        for outcome in outcomes {
            match outcome {
                StageOutcome::Continue => {}
                StageOutcome::AddInstructions(items) | StageOutcome::AddContext(items) => {
                    for item in items.iter() {
                        messages.push(context_item_to_message(item)?);
                    }
                }
                StageOutcome::Replace(value) => {
                    *messages = parse_messages(&value)?;
                }
                StageOutcome::Fail(descriptor) => {
                    return Ok(ReducerStageOutcome::Fail(*descriptor))
                }
                StageOutcome::Retry(directive) => {
                    return Ok(ReducerStageOutcome::Retry(directive))
                }
                other => {
                    return Err(AgentRunError::runtime_message(&format!(
                        "outcome {} is not foldable at prepare_context",
                        other.variant_name()
                    )))
                }
            }
        }
        Ok(ReducerStageOutcome::ContextPrepared {
            messages: messages.clone().into(),
        })
    }
```

Add the normalizer near `context_messages` (`agent.rs:993`):

```rust
/// Normalize one `ContextItem` into a provider-neutral `Message`.
///
/// Authority is honored: an item whose authority is not `System` or `Developer`
/// becomes a quoted user-role message, matching the untrusted-context rule the
/// context port applies at `context.rs:282`.
fn context_item_to_message(item: &ContextItem) -> Result<Message, AgentRunError> {
    let role = match item.authority() {
        ContextAuthority::System => Role::System,
        ContextAuthority::Developer => Role::Developer,
        ContextAuthority::Untrusted => Role::User,
    };
    Message::try_new(role, Arc::from([Block::Text(TextBlock::new(item.text()))]))
        .map_err(|error| AgentRunError::runtime_message(&format!("context item invalid: {error}")))
}
```

Read `crates/finstack-ai-runtime/src/context.rs:120-170` for the exact
`ContextItem` accessor names and adjust `item.authority()` / `item.text()` to
match.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai --locked --test middleware_chain`
Expected: PASS

- [ ] **Step 5: Confirm no regression**

Run: `cargo test --workspace --locked`
Expected: PASS

---

### Task 5: `BeforeModel` stage — compaction, `FilterTools`, and the post-compaction tier

The highest-effort stage and the only one whose `StageInput` is typed rather
than opaque. `BeforeModelInput` (`middleware.rs:236`) needs
`source_entries: Arc<[CompactionSourceEntry]>`, where each entry carries
`entry_id`, `message`, `sensitivity`, `provenance_digest`, and `protected`.

`MiddlewareRole::PostCompactionValidator` and
`OrderTier::PostCompactionValidation` are in the frozen public API with zero
implementations anywhere in the repo. Honoring the tier here retires that unused
seam rather than adding another.

**Files:**
- Modify: `crates/finstack-ai/src/agent.rs:795-827`
- Test: `crates/finstack-ai/tests/middleware_chain.rs`

**Interfaces:**
- Consumes: `context_item_to_message` (Task 4), `validate_compaction_result`
  (`middleware.rs:984`)
- Produces: `fn build_before_model_input(&self, state, draft) ->
  Result<BeforeModelInput, AgentRunError>`

- [ ] **Step 1: Write the failing tests**

Two tests. First, `FilterTools` narrows the model's tool list:

```rust
#[tokio::test]
async fn before_model_filter_tools_narrows_the_model_tool_list() {
    let model = Arc::new(ScriptedModel::recording_tools());
    let agent = Agent::builder(model.clone())
        .toolset(
            ComponentRef::new("test.tools.calc", None),
            Arc::new(finstack_ai_tools_calculator::CalculatorToolset::new()),
        )
        .middleware(
            ComponentRef::new("test.middleware.filter", None),
            Arc::new(RetainNothing),
        )
        .build()
        .await
        .expect("agent builds");

    agent.run("hello").await.expect("run completes");

    assert!(
        model.last_request_tools().is_empty(),
        "FilterTools did not narrow the request: {:?}",
        model.last_request_tools()
    );
}
```

Second, the post-compaction narrowing at `middleware.rs:956` — a validator may
return only `Continue`, `Fail`, `Suspend`, or `RequestInteraction`:

```rust
#[tokio::test]
async fn post_compaction_validator_fail_stops_the_run() {
    let agent = Agent::builder(Arc::new(ScriptedModel::always_text("unreachable")))
        .middleware(
            ComponentRef::new("test.middleware.validator", None),
            Arc::new(ValidatorReturning(StageOutcome::Fail(Box::new(
                ErrorDescriptor::new(
                    "context_budget_exceeded",
                    "projection exceeds the hard input budget",
                    ErrorCategory::Limit,
                    false,
                ),
            )))),
        )
        .build()
        .await
        .expect("agent builds");

    let error = agent.run("hello").await.expect_err("validator must stop the run");
    assert!(format!("{error}").contains("context_budget_exceeded"));
}

#[tokio::test]
async fn post_compaction_validator_cannot_add_context() {
    let agent = Agent::builder(Arc::new(ScriptedModel::always_text("unreachable")))
        .middleware(
            ComponentRef::new("test.middleware.validator", None),
            Arc::new(ValidatorReturning(StageOutcome::AddContext(Arc::from([
                context_item("smuggled"),
            ])))),
        )
        .build()
        .await
        .expect("agent builds");

    let error = agent.run("hello").await.expect_err("narrowing must reject");
    assert!(
        format!("{error}").contains("middleware_outcome_not_allowed"),
        "expected the stage-matrix rejection, got: {error}"
    );
}
```

`ValidatorReturning` is a test middleware whose descriptor sets
`role: MiddlewareRole::PostCompactionValidator` and
`order.tier: OrderTier::PostCompactionValidation`, and whose `invoke` returns the
wrapped outcome verbatim.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai --locked --test middleware_chain before_model`
Expected: FAIL — the tool list is unchanged and the validator never runs.

- [ ] **Step 3: Build the typed stage input**

Add to `impl Agent`:

```rust
    /// Build the typed `BeforeModel` stage input.
    ///
    /// `protected` is sourced from the context port, which is the authoritative
    /// setter (`context.rs:138`); the compactor cannot set it. Entries the run
    /// has no provenance for are recorded as unprotected with a digest over the
    /// canonical message bytes.
    fn build_before_model_input(
        &self,
        state: &KernelState,
        draft: &ModelDraft,
    ) -> Result<BeforeModelInput, AgentRunError> {
        let mut source_entries = Vec::with_capacity(draft.messages.len());
        for (index, message) in draft.messages.iter().enumerate() {
            let bytes = canonical_message(message)?;
            source_entries.push(CompactionSourceEntry {
                entry_id: state
                    .messages
                    .get(index)
                    .map(|entry| entry.entry_id)
                    .ok_or_else(|| {
                        AgentRunError::runtime_message("compaction source entry has no entry id")
                    })?,
                message: message.clone(),
                sensitivity: state
                    .messages
                    .get(index)
                    .map_or(Sensitivity::Internal, |entry| entry.sensitivity),
                provenance_digest: Digest::raw_json(&bytes),
                protected: state
                    .messages
                    .get(index)
                    .is_some_and(|entry| entry.protected),
            });
        }
        let profile = self.model_context_profile()?;
        Ok(BeforeModelInput {
            source_entries: source_entries.into(),
            model_context_profile_digest: profile.digest,
            hard_input_tokens: profile
                .profile
                .context_window_tokens
                .saturating_sub(profile.profile.reserved_output_tokens)
                .saturating_sub(profile.profile.provider_overhead_tokens),
        })
    }
```

Read `crates/finstack-ai-runtime/src/middleware.rs:220-248` for the exact
`CompactionSourceEntry` and `BeforeModelInput` field list and adjust names to
match; the struct is `#[serde(deny_unknown_fields)]` so every field is required.

- [ ] **Step 4: Drive the stage in the four fixed order tiers**

Insert between the `draft` at `agent.rs:810` and `request_json` at `:811`:

```rust
            let chain = self.resolved.run_plan().middleware_chain();
            let mut tools: Vec<ToolSpec> =
                self.tools.tools().map(|tool| tool.spec.clone()).collect();
            if !chain.stage(Stage::BeforeModel).is_empty() {
                let input = StageInput::BeforeModel(Box::new(
                    self.build_before_model_input(&state, &draft)?,
                ));
                let ctx = self.stage_run_context(state.cycle, Stage::BeforeModel)?;
                let outcomes =
                    finstack_ai_runtime::middleware_driver_invoke_stage(chain, &ctx, input.clone())
                        .await
                        .map_err(AgentRunError::from_middleware)?;
                if let Some(terminal) =
                    self.fold_before_model(&input, &mut draft, &mut tools, outcomes)?
                {
                    submit_stage(
                        handle,
                        state.cycle,
                        Stage::BeforeModel,
                        terminal.clone(),
                        StageIds::for_outcome(&terminal),
                    )
                    .await?;
                    continue;
                }
            }
            draft.tools = tools;
```

`invoke_stage` already walks components in resolved order, and
`ResolvedMiddlewareChain::try_new` sorts by `OrderTier` before numeric priority
(`middleware.rs:593`), so the four tiers — `Standard`, `ContextCompaction`,
`PostCompactionValidation` — arrive in the correct order with no extra sorting
here.

Add the fold:

```rust
    /// Fold `BeforeModel` outcomes.
    ///
    /// Returns `Some(outcome)` when a component produced a terminal outcome that
    /// short-circuits the model request, and `None` when the run proceeds with
    /// the mutated draft.
    ///
    /// Multiple `FilterTools` outcomes INTERSECT — narrowing is monotone, so
    /// component order cannot change the result.
    fn fold_before_model(
        &self,
        input: &StageInput,
        draft: &mut ModelDraft,
        tools: &mut Vec<ToolSpec>,
        outcomes: Vec<StageOutcome>,
    ) -> Result<Option<ReducerStageOutcome>, AgentRunError> {
        let StageInput::BeforeModel(before_model) = input else {
            return Err(AgentRunError::runtime_message("before_model input mismatch"));
        };
        for outcome in outcomes {
            match outcome {
                StageOutcome::Continue => {}
                StageOutcome::AddInstructions(items) | StageOutcome::AddContext(items) => {
                    for item in items.iter() {
                        draft.messages.push(context_item_to_message(item)?);
                    }
                }
                StageOutcome::FilterTools(retain) => {
                    tools.retain(|spec| retain.iter().any(|id| *id == spec.id));
                }
                StageOutcome::CompactContext(result) => {
                    validate_compaction_result(
                        self.compactor_descriptor()?,
                        before_model,
                        &result,
                    )
                    .map_err(AgentRunError::from_middleware)?;
                    draft.messages = result.projection.to_vec();
                }
                StageOutcome::Replace(value) => {
                    draft.messages = parse_messages(&value)?;
                }
                StageOutcome::Fail(descriptor) => {
                    return Ok(Some(ReducerStageOutcome::Fail(*descriptor)))
                }
                StageOutcome::Retry(directive) => {
                    return Ok(Some(ReducerStageOutcome::Retry(directive)))
                }
                other => {
                    return Err(AgentRunError::runtime_message(&format!(
                        "outcome {} is not foldable at before_model",
                        other.variant_name()
                    )))
                }
            }
        }
        Ok(None)
    }
```

`RequestCompactionModel` is deliberately **not** handled here: it requires a
child model effect under `EffectPurpose::CompactionSummary` plus a re-entry into
the same middleware identity with `MiddlewareContext.compaction_resume`. That is
Task 9.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p finstack-ai --locked --test middleware_chain`
Expected: PASS

Run: `cargo test -p finstack-ai-middleware-compaction --locked`
Expected: PASS — the compaction leaf's own tests still drive `invoke` directly
and are unaffected.

---

### Task 6: `BeforeFinalize` stage and the `Retry` first producer

`ReducerStageOutcome::Retry` is fully built and golden-traced (admission at
`decide.rs:1162`, bodies at `decide.rs:1270`, `StageDisposition::RetryScheduled`)
but is reached today only by one hardcoded native structured-output retry at
`agent.rs:923`. This task gives it a middleware producer.

Run the chain **once**, before the `if` at `agent.rs:914`, and let the folded
result choose the branch. Do not run it on both paths.

**Files:**
- Modify: `crates/finstack-ai/src/agent.rs:914-956`
- Modify: `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs`
- Modify: `extensions/middleware/finstack-ai-middleware-verify/src/tests.rs`
- Test: `crates/finstack-ai/tests/middleware_chain.rs`

**Interfaces:**
- Consumes: `invoke_stage` (Task 1), `StageIds::for_outcome` (Task 2)
- Produces: `VerifyDecision::Retry { classification: RetryClassification }` on
  the verify leaf

- [ ] **Step 1: Write the failing test**

In `extensions/middleware/finstack-ai-middleware-verify/src/tests.rs`:

```rust
#[tokio::test]
async fn retry_decision_returns_retry_outcome() {
    let middleware = VerifyMiddleware::new(|_candidate| VerifyDecision::Retry {
        classification: RetryClassification::Validation,
    });
    let outcome = invoke(&middleware, candidate_input()).await.expect("invoke");
    assert!(
        matches!(outcome, StageOutcome::Retry(_)),
        "expected Retry, got {outcome:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai-middleware-verify --locked retry_decision`
Expected: FAIL — no variant `Retry` on `VerifyDecision`.

- [ ] **Step 3: Add the decision arm**

In `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs`, add to
`enum VerifyDecision`:

```rust
    /// Request one bounded whole-run semantic retry.
    Retry {
        /// Stable retry classification recorded in the directive.
        classification: RetryClassification,
    },
```

and to the `invoke` match, alongside the existing `Accept` / `Reject` arms:

```rust
            VerifyDecision::Retry { classification } => {
                let directive = RetryDirective::try_new(
                    classification,
                    Digest::raw_json(candidate.as_bytes()),
                    None,
                )
                .map_err(|error| {
                    MiddlewareError::stable(
                        VERIFY_DECISION_INVALID,
                        &format!("retry directive invalid: {error}"),
                    )
                })?;
                Ok(StageOutcome::Retry(directive))
            }
```

Add `pub const VERIFY_DECISION_INVALID: &str = "verify_decision_invalid";`
next to the crate's existing stable-code constants.

- [ ] **Step 4: Drive the stage in the facade**

Replace `agent.rs:914-956` so the chain runs once first:

```rust
            let finalize = self
                .run_finalize_stage(handle, &next)
                .await?;
            if let Some(outcome) = finalize {
                submit_stage(
                    handle,
                    next.cycle,
                    Stage::BeforeFinalize,
                    outcome.clone(),
                    StageIds::for_outcome(&outcome),
                )
                .await?;
                continue;
            }
            // existing structured-output retry check at 914, then FinalizeAccepted at 953
```

with:

```rust
    /// Run the `BeforeFinalize` chain once and fold it.
    ///
    /// Returns `None` when the chain continued, leaving the existing
    /// structured-output retry check and `FinalizeAccepted` path in charge.
    async fn run_finalize_stage(
        &self,
        handle: &RunHandle,
        state: &KernelState,
    ) -> Result<Option<ReducerStageOutcome>, AgentRunError> {
        let chain = self.resolved.run_plan().middleware_chain();
        if chain.stage(Stage::BeforeFinalize).is_empty() {
            return Ok(None);
        }
        let candidate = RawJson::parse(&canonical_terminal_candidate(state)?)?;
        let ctx = self.stage_run_context(state.cycle, Stage::BeforeFinalize)?;
        let outcomes = finstack_ai_runtime::middleware_driver_invoke_stage(
            chain,
            &ctx,
            StageInput::BeforeFinalize { candidate },
        )
        .await
        .map_err(AgentRunError::from_middleware)?;
        for outcome in outcomes {
            match outcome {
                StageOutcome::Continue => {}
                StageOutcome::Fail(descriptor) => {
                    return Ok(Some(ReducerStageOutcome::Fail(*descriptor)))
                }
                StageOutcome::Retry(directive) => {
                    return Ok(Some(ReducerStageOutcome::Retry(directive)))
                }
                StageOutcome::RequestInteraction(request) => {
                    self.request_interaction(handle, *request).await?;
                    return Ok(Some(ReducerStageOutcome::Continue));
                }
                other => {
                    return Err(AgentRunError::runtime_message(&format!(
                        "outcome {} is not foldable at before_finalize",
                        other.variant_name()
                    )))
                }
            }
        }
        Ok(None)
    }
```

`Replace` is correctly absent — the matrix forbids it at `BeforeFinalize`
(`middleware.rs:926`), and `verify/src/tests.rs:89` already asserts the
rejection.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p finstack-ai-middleware-verify --locked`
Expected: PASS

Run: `cargo test -p finstack-ai --locked --test middleware_chain`
Expected: PASS

---

### Task 7: Thread the chain into `RunTaskOwner` for `BeforeToolBatch`

`Stage::BeforeToolBatch` is the one stage the facade does not own. It is settled
inside `prepare_tool_batch_if_ready` (`settlement.rs:131`), called from the
runtime worker at `task.rs:952` and `task.rs:1021` — inside `RunTaskOwner`,
which has no access to the chain. This is pure plumbing and is separated from
Task 8's behavior change so a reviewer can reject one without the other.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/task.rs:295`, `:466`
- Modify: `crates/finstack-ai-runtime/src/settlement.rs:131`, `:198-207`
- Modify: `crates/finstack-ai/src/agent.rs` (the spawn call sites)
- Test: `crates/finstack-ai-runtime/src/settlement.rs` (inline tests)

**Interfaces:**
- Consumes: `ResolvedMiddlewareChain` from the facade's resolved run plan
- Produces: `SettlementSources.middleware_chain: Option<Arc<ResolvedMiddlewareChain>>`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn tool_batch_settlement_sees_the_middleware_chain() {
    let chain = Arc::new(
        ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration {
            middleware: Arc::new(RetainNothingAtToolBatch),
        }])
        .expect("chain resolves"),
    );
    let sources = settlement_sources_with_chain(Arc::clone(&chain));
    assert!(
        sources.middleware_chain.is_some(),
        "chain did not reach tool-batch settlement"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai-runtime --locked tool_batch_settlement_sees`
Expected: FAIL — no field `middleware_chain`.

- [ ] **Step 3: Add the parameter through the spawn path**

Add `middleware_chain: Option<Arc<ResolvedMiddlewareChain>>` to
`SettlementSources`, defaulting to `None`. Add the same parameter to
`RunTaskOwner::spawn_with_model` (`task.rs:295`) and
`spawn_with_model_and_tools` (`task.rs:466`), and pass it from the facade's
spawn sites as `Some(Arc::clone(self.resolved.run_plan().middleware_chain()))`.

Keeping it `Option` means every existing caller and test compiles by passing
`None`, so this task carries no behavior change at all.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Expected: PASS

Run: `cargo test --workspace --locked`
Expected: PASS — no behavior changed; only a field was threaded through.

---

### Task 8: `BeforeToolBatch` — `FilterTools` via `SyntheticClosure`, and `Replace`

`TDD:1247` requires the `ToolBatchPrepared` plan to cover every source
`ToolCallBlock` exactly once in source order — so a filtered-out call cannot be
dropped. `ToolCallPlan::SyntheticClosure(SyntheticToolClosure { call, execution,
failure_policy, error })` (`crates/finstack-ai-kernel/src/tools.rs:76`) is
precisely the "present in the plan but not executed" shape, so denial is
expressible with zero new surface.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/settlement.rs:131-207`
- Test: `crates/finstack-ai-runtime/src/settlement.rs` (inline tests)

**Interfaces:**
- Consumes: `SettlementSources.middleware_chain` (Task 7), `invoke_stage` (Task 1)
- Produces: no new public surface

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn filtered_tool_call_becomes_a_synthetic_closure() {
    let plans = prepare_with_chain(retain_nothing_chain(), one_tool_call()).await;
    assert_eq!(plans.len(), 1, "every source call must appear exactly once");
    assert!(
        matches!(plans[0], ToolCallPlan::SyntheticClosure(_)),
        "denied call must be a synthetic closure, got {:?}",
        plans[0]
    );
}

#[tokio::test]
async fn replace_substitutes_the_whole_batch_plan() {
    let replacement = serde_json::json!([{ "kind": "synthetic_closure", "call": { "id": "call-1" } }]);
    let plans = prepare_with_chain(
        replace_plan_chain(RawJson::parse(replacement.to_string().as_bytes()).unwrap()),
        one_tool_call(),
    )
    .await;
    assert_eq!(plans.len(), 1, "replacement must still cover every source call");
    assert!(matches!(plans[0], ToolCallPlan::SyntheticClosure(_)));
}

#[tokio::test]
async fn replace_that_drops_a_source_call_is_rejected() {
    let error = prepare_with_chain_err(
        replace_plan_chain(RawJson::parse(b"[]").unwrap()),
        one_tool_call(),
    )
    .await;
    assert!(
        format!("{error}").contains("tool_registration_invalid"),
        "a plan missing a source call must be rejected, got: {error}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-runtime --locked filtered_tool_call_becomes replace_substitutes replace_that_drops`
Expected: FAIL — the plan still contains a `Direct` entry and `Replace` is
rejected as unfoldable.

- [ ] **Step 3: Fold `FilterTools` into the plan**

In `prepare_tool_batch_if_ready`, after the plans are computed and before the
`KernelInput::StageSettled` at `settlement.rs:198`, run the chain when
`sources.middleware_chain` is `Some` and the stage is non-empty. For each plan
whose tool id is not retained, replace it with:

```rust
ToolCallPlan::SyntheticClosure(SyntheticToolClosure {
    call: call.clone(),
    execution: ToolExecutionMode::Direct,
    failure_policy: ToolFailurePolicy::Continue,
    error: ErrorDescriptor::new(
        TOOL_POLICY_DENIED,
        "middleware filtered this tool call",
        ErrorCategory::Policy,
        false,
    ),
})
```

Then handle `Replace`, which at this stage carries a whole replacement plan:

```rust
                StageOutcome::Replace(value) => {
                    plans = serde_json::from_slice::<Vec<ToolCallPlan>>(value.as_bytes())
                        .map_err(|error| {
                            MiddlewareError::stable(
                                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                                &format!("replace payload is not a tool batch plan: {error}"),
                            )
                        })?;
                }
```

**Coverage invariant.** After the whole fold, and before the
`KernelInput::StageSettled` submission, assert that the final plan covers every
source `ToolCallBlock` exactly once in source order (`TDD:1247`). A `Replace`
or `FilterTools` fold that drops or reorders a call must fail with the reserved
code `tool_registration_invalid`, not be silently accepted:

```rust
    if plans.len() != source_calls.len()
        || plans
            .iter()
            .zip(source_calls.iter())
            .any(|(plan, call)| plan.call_id() != call.id)
    {
        return Err(SettlementError::stable(
            TOOL_REGISTRATION_INVALID,
            "middleware tool batch plan does not cover every source call in order",
        ));
    }
```

**Fail-closed rule:** the run-deadline path at `settlement.rs:232-238`, which
emits `ReducerStageOutcome::Continue`, bypasses the chain entirely. A run that
is already out of budget must not spend more on middleware.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Expected: PASS

Run: `cargo test --workspace --locked`
Expected: PASS

---

### Task 9: `RequestCompactionModel` child-effect round trip

The compaction leaf already produces this outcome (`compaction/src/lib.rs:469`),
and `validate_compaction_model_effect` (`middleware.rs:859`) exists with zero
callers. Without this task, a model-assisted compaction strategy is registered
but its outcome is rejected by Task 5's fold.

**Files:**
- Modify: `crates/finstack-ai/src/agent.rs` (the Task 5 `BeforeModel` block)
- Test: `crates/finstack-ai/tests/middleware_chain.rs`

**Interfaces:**
- Consumes: `validate_compaction_model_effect` (`middleware.rs:859`),
  `MiddlewareContext.compaction_resume`, `CompactionModelResume`
- Produces: no new public surface

- [ ] **Step 1: Write the failing tests**

```rust
/// Returns `RequestCompactionModel` when `compaction_resume` is absent and
/// `CompactContext` over the child model's summary when it is present.
struct SummarizingCompactor;

#[tokio::test]
async fn compaction_model_round_trip_reaches_the_model() {
    let model = Arc::new(ScriptedModel::scripted(vec![
        // Child summary request, dispatched under EffectPurpose::CompactionSummary.
        ScriptedStep::text("SUMMARY"),
        // Parent request, which must now see the compacted projection.
        ScriptedStep::text("done"),
    ]));
    let agent = Agent::builder(model.clone())
        .middleware(
            ComponentRef::new("test.middleware.summarize", None),
            Arc::new(SummarizingCompactor),
        )
        .build()
        .await
        .expect("agent builds");

    agent.run("hello").await.expect("run completes");

    let parent = model.last_request_messages();
    assert!(
        parent.iter().any(|m| m.text().contains("SUMMARY")),
        "the parent request never saw the compacted projection: {parent:?}"
    );
}

#[tokio::test]
async fn a_second_compaction_model_request_from_one_component_is_rejected() {
    let agent = Agent::builder(Arc::new(ScriptedModel::always_text("SUMMARY")))
        .middleware(
            ComponentRef::new("test.middleware.loop", None),
            Arc::new(AlwaysRequestsCompactionModel),
        )
        .build()
        .await
        .expect("agent builds");

    let error = agent.run("hello").await.expect_err("must not loop");
    assert!(
        format!("{error}").contains("compaction_result_invalid"),
        "expected a bounded-resume rejection, got: {error}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai --locked --test middleware_chain compaction_model`
Expected: FAIL — `request_compaction_model is not foldable at before_model`.

- [ ] **Step 3: Implement the round trip**

Add the arm to `fold_before_model`. Because the resume needs to re-enter one
specific component, the fold changes from a flat loop to an indexed loop that
can re-invoke the component it is currently on:

```rust
                StageOutcome::RequestCompactionModel(request) => {
                    if resumed.contains(&index) {
                        return Err(AgentRunError::from_middleware(MiddlewareError::stable(
                            COMPACTION_RESULT_INVALID,
                            "a component may request a compaction model at most once per stage",
                        )));
                    }
                    resumed.insert(index);
                    let child = self
                        .dispatch_compaction_model(handle, &ctx, &resolved.descriptor, &request)
                        .await?;
                    let resume_ctx = MiddlewareContext {
                        compaction_resume: Some(child),
                        ..ctx.middleware_context(index)?
                    };
                    let again = resolved
                        .middleware
                        .invoke(resume_ctx, input.clone())
                        .await
                        .map_err(AgentRunError::from_middleware)?;
                    validate_stage_outcome(&resolved.descriptor, input, &again)
                        .map_err(AgentRunError::from_middleware)?;
                    outcomes_to_apply.push(again);
                }
```

and the child dispatch:

```rust
    /// Dispatch one authorized child model effect for a compaction summary.
    ///
    /// The child is committed under `EffectPurpose::CompactionSummary` and
    /// validated against the parent before it runs, so an unauthorized or
    /// mis-parented child cannot reach a provider.
    async fn dispatch_compaction_model(
        &self,
        handle: &RunHandle,
        ctx: &StageRunContext,
        descriptor: &MiddlewareDescriptor,
        request: &CompactionModelRequest,
    ) -> Result<CompactionModelResume, AgentRunError> {
        let relation = EffectRelation {
            parent_effect_id: ctx.run.effect_id,
            purpose: EffectPurpose::CompactionSummary {
                middleware_component_id: descriptor.component.id().to_owned(),
            },
        };
        let (child_envelope, request_id, effect_id) =
            self.commit_child_model_effect(handle, relation, request).await?;
        validate_compaction_model_effect(&ctx.parent, &child_envelope, request)
            .map_err(AgentRunError::from_middleware)?;
        let result = self.run_model_effect(handle, effect_id, request).await?;
        Ok(CompactionModelResume {
            request_id,
            effect_id,
            result,
            resume_state: request.resume_state.clone(),
        })
    }
```

`commit_child_model_effect` and `run_model_effect` are the existing model path —
grep `agent.rs` for how the primary model effect is committed and dispatched and
reuse those functions rather than writing a second dispatcher. If the primary
path is inlined rather than factored, extract it first as a no-behavior-change
refactor and land that as its own commit before this task's change.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai --locked --test middleware_chain`
Expected: PASS

- [ ] **Step 5: Run the compaction conformance suite**

Run: `cargo test -p finstack-ai-test --locked --test compaction_conformance`
Expected: PASS

---

### Task 10: Wire the published conformance suite into both leaf crates

`check_middleware_conformance` (`crates/finstack-ai-test/src/port_conformance.rs:301`)
proves four contracts — `middleware.invoke.completed`,
`middleware.outcome.stage_matrix`, `middleware.outcome.expected`,
`middleware.descriptor.stable` — and has exactly one call site in the repo
(`crates/finstack-ai-test/tests/port_helpers.rs:140`). Neither shipping
middleware leaf calls it, unlike every observer leaf.

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-compaction/Cargo.toml`
- Modify: `extensions/middleware/finstack-ai-middleware-compaction/src/tests.rs`
- Modify: `extensions/middleware/finstack-ai-middleware-verify/Cargo.toml`
- Modify: `extensions/middleware/finstack-ai-middleware-verify/src/tests.rs`

**Interfaces:**
- Consumes: `check_middleware_conformance`, `PortConformanceFailure`
- Produces: no new public surface

- [ ] **Step 1: Write the failing test**

Add to each crate's `src/tests.rs`:

```rust
#[tokio::test]
async fn middleware_satisfies_the_published_port_conformance_suite() {
    let middleware = build_middleware_under_test();
    finstack_ai_test::check_middleware_conformance(&middleware)
        .await
        .expect("published middleware conformance suite");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-middleware-verify --locked port_conformance`
Expected: FAIL — unresolved crate `finstack_ai_test`.

- [ ] **Step 3: Add the dev-dependency**

Add to both crates' `[dev-dependencies]`:

```toml
finstack-ai-test = { workspace = true }
```

matching how the observer leaves declare it.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-middleware-compaction -p finstack-ai-middleware-verify --locked`
Expected: PASS

Run: `mise run conformance`
Expected: PASS

---

### Task 11: Document the two deliberately dead variants

`Suspend` and `Complete` have no landing path and are being left without a
producer on purpose. Recording the reasoning prevents a future contributor from
re-litigating it or, worse, adding a kernel variant to "finish" them.

**Files:**
- Create: `docs/implementation/adrs/adr-middleware-aggregate-fold.md`
- Modify: `docs/implementation/adr-register.md`

**Interfaces:**
- Consumes: nothing
- Produces: an ADR id referenced by Task 12's ledger row

- [ ] **Step 1: Write the ADR**

Record four decisions with their evidence:

1. **Aggregate fold over per-component durable invocation.** The kernel models
   the boundary as one `StageOutcomeRecorded { cursor, disposition,
   settlement_digest }` per `(cycle, stage)`; TDD §17.5's "every invocation
   commits `EffectCompleted` plus `StageOutcomeRecorded`" is unsatisfiable for a
   stage with more than one component. §17.5 is amended to the aggregate model.
2. **Consequence: five runtime items stay unused.**
   `CommittedMiddlewareCall` (`middleware.rs:695`), `RecordedMiddlewareOutcome`
   (`:784`), `middleware_resume_action` (`:838`), `PendingMiddlewareEffect`
   (`:503`), and `Middleware::reconcile` (`:554`) are not called by the driver.
   They are pre-existing public surface and cannot be removed without a breaking
   change; mark them deprecated-in-place with a doc note pointing at this ADR.
3. **`Suspend` stays dead.** No `KernelInput` variant can express suspension
   (all 15 enumerated at `reducer/input.rs:33-62`), and it is typed
   `Box<ErrorDescriptor>`, making it indistinguishable from `Fail` on the wire.
4. **`Complete` stays dead.** `decide.rs:1107` admits only `Continue` at
   `BeforeRun`/`AfterToolBatch`; `FinalizeAccepted` is payload-free so it cannot
   carry an overriding result; `TDD:1693` forbids `RunCompleted` before
   `BeforeFinalize` settles.

Also record the **open contradiction** this plan does not resolve: `TDD:3172`
permits `Retry` at `AfterModel`/`AfterToolBatch`/`BeforeFinalize` and
`middleware.rs:937` implements exactly that, but `TDD:3713` says `Retry` is
"permitted only at `BeforeFinalize` for a failed candidate whose descriptor is
retryable." Task 3 and Task 6 implement the permissive matrix reading. Flag it
for a follow-up spec fix rather than narrowing a validated public contract here.

- [ ] **Step 2: Register the ADR**

Add the row to `docs/implementation/adr-register.md` following the existing
column format.

- [ ] **Step 3: Verify docs links**

Run: `mise run docs-links`
Expected: PASS

---

### Task 12: Record delivery evidence and the replay-safety invariant

**Files:**
- Modify: `docs/implementation/delivery-ledger.md`
- Modify: `docs/implementation/evidence-register.md`
- Create: `docs/implementation/artifacts/pr-018/implementation.md`
- Modify: `docs/site/middleware.md` (create if absent)
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: the ADR id from Task 11
- Produces: the closing record for this work

- [ ] **Step 1: Add the task ledger row**

Use the 12-column format at `docs/implementation/delivery-ledger.md:382` and the
`PR-NNN-T-short-slug-xxxxxxxxxxxx` id scheme at `:380`, where the final 12
lowercase hex characters are generated randomly when the row is created:

```text
| PR-018-T-mw-driver-<12 random hex> | PR-018 | Drive the resolved middleware chain at all seven stages and fold to the aggregate stage outcome | Done | <owner> | — | `<branch>` | PR-018-A01–A04 | [artifacts/pr-018/implementation.md](artifacts/pr-018/implementation.md) | 2026-08-15 | <updated> | Aggregate fold per ADR; per-component durable invocation not claimed |
```

Column 12 doubles as the negative-scope disposition field — state plainly that
`reconcile()` is not driven, mirroring how PR-030's row records its own
deferral at `:541`.

- [ ] **Step 2: Document the replay-safety invariant**

In `docs/site/middleware.md`, state the constraint the aggregate design imposes:

> A `Middleware` implementation must be pure with respect to external state.
> Individual invocations are not journaled; on recovery, a stage whose
> `StageOutcomeRecorded` is absent re-runs its entire chain from the start.
> Middleware that performs a non-repeatable external action will repeat it.

- [ ] **Step 3: Update the changelog**

Add under `## [Unreleased]` → `### Added` a bullet naming the driver and the new
first producers, and under `### Changed` a note that registered middleware now
affects run behavior where previously it was resolved but never invoked.

- [ ] **Step 4: Run the full gate**

Run: `mise run ci`
Expected: PASS

Run: `mise run conformance`
Expected: PASS

Run: `mise run supply-chain`
Expected: PASS

- [ ] **Step 5: Confirm the frozen surface is intact**

Run: `cargo test -p finstack-ai-test --offline --locked --test breaking_change`
Expected: PASS — `public_item_lists_reject_renames` reports no missing baseline
items.
