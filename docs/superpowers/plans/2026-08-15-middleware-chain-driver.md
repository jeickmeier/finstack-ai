# Middleware Chain Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the resolved `ResolvedMiddlewareChain` actually run at all seven
stages, folding each stage's ordered component outcomes into the single
aggregate `ReducerStageOutcome` the kernel already accepts, and ship a first
real producer for every dead `StageOutcome` variant that has a landing path.

**Architecture:** Aggregate fold in the **runtime layer**, no kernel change.
`KernelInput::StageSettled { cursor, outcome }` and `StageOutcomeRecorded
{ cursor, disposition, settlement_digest }` already model the middleware
boundary as exactly one outcome per `(cycle, stage)`. Every facade stage
settlement funnels through `RunHandle::submit`, so the driver intercepts the
`KernelInput::StageSettled` branch of the worker command loops — catching six of
seven stages at one choke point with **no change to the facade's stage
sequence**. `BeforeToolBatch` hooks the existing runtime-owned site in
`prepare_tool_batch_if_ready`. Per-invocation effects are **not** committed.

> **Revised 2026-08-16.** The original plan drove six stages from the facade.
> That is impossible: `Agent` holds no per-run state, and more decisively the
> facade cannot build a `RunCallContext` — it has no `CancellationSignal` and no
> `AuthorizationContext`, both of which are assembled only in the runtime from
> dispatch seeds. Tasks 3-9 were rewritten against verified runtime interfaces.
> The full verified design, with exact signatures and file:line anchors, is at
> `.superpowers/sdd/2026-08-15-middleware-chain-driver/runtime-driver-design.json`
> — read it before implementing any of Tasks 4-9.

**Governing invariant — passthrough when the chain is empty.** With no driver
installed, or no component registered for a stage, the worker submits the
facade's `env` and `input` byte-for-byte unchanged. Existing journals, digests,
and tests are unaffected; the feature is opt-in per agent.

**Tech Stack:** Rust 1.97.1, tokio, `finstack-ai-kernel`, `finstack-ai-runtime`,
`finstack-ai` facade, `finstack-ai-test` conformance suite.

## Global Constraints

- **No kernel change.** `crates/finstack-ai-kernel/` is not modified by this
  plan. If a task appears to require a `KernelInput` variant, a `KernelState`
  field, or an `apply_effect_*` branch, stop and escalate — the design decision
  was explicitly aggregate-fold.
- **The driver is a documented public API of `finstack-ai-runtime`.** Decided
  2026-08-16. Real `pub use` entries with full doc comments — **not**
  `#[doc(hidden)]` shims. This is surface the project owes compatibility on at
  1.0, so Task 11's ADR must record it. The exact set grows across Tasks 4-9;
  Task 9 freezes it. `CommitCoordinator::install_middleware_chain` and
  `middleware_chain` must be `pub` specifically because their peers
  `install_dispatcher` (`coordinator.rs:825`) and `install_event_publisher`
  (`:851`) are `pub(crate)` and therefore unreachable from the facade.
- **The facade changes by exactly one line.** `coordinator.install_middleware_chain(...)`
  between `agent.rs:598` and the spawn calls. The seven `submit_stage` call
  sites, `submit_stage` itself, `NativeIds::environment`, and the `StageIds`
  constructors are all untouched — the facade keeps proposing base outcomes and
  never learns that a chain ran. Do **not** change `RunTaskConfig` (it is `Copy`,
  `run_types.rs:59`) or the public `spawn_with_model*` signatures.
- **`RunCallContext.effect_id` at a stage boundary is a derived correlation id,
  not a committed effect identity.** No `KernelInput` commits an
  `EffectKind::Middleware` effect; stage settlement emits only
  `StageOutcomeRecorded` (`decide.rs:2369`). It is derived deterministically so
  replay reproduces it. Anything treating it as journaled is wrong.
- **Public-surface mechanics.** `scripts/compat/public_items.py` scrapes `pub use`
  blocks from exactly `crates/finstack-ai-kernel/src/lib.rs`,
  `crates/finstack-ai-runtime/src/lib.rs`, and `crates/finstack-ai/src/lib.rs`.
  Removals and renames fail `public_item_lists_reject_renames`
  (`crates/finstack-ai-test/tests/breaking_change.rs:113`), which runs inside
  `mise run test`. Additions never fail `compare()`, but the three new names
  MUST still be added to
  `fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`
  in sorted position, or the next removal check is meaningless.
- **No other new public surface.** Beyond those three names, add nothing to any
  frozen `lib.rs`. In particular, no new `StageOutcome` variant.
- **Test fixtures use the real `finstack-ai-test` API.** `ScriptedModel` has
  **no** `always_text`, `echoing_last_user_text`, `recording_tools`, or
  `scripted` constructors, and **no** `last_request_tools` /
  `last_request_messages` accessors. The real API is
  `ScriptedModel::from_plans(profile, plans)` /
  `from_inputs(profile, inputs)`, with `last_request() -> Option<ModelRequest>`
  and `request_count()`. Copy the `profile()` and `completed(...)` helpers from
  `crates/finstack-ai/tests/nfr_perf_007.rs:22-33` verbatim as the fixture
  template. Where a task's test snippet below names a constructor that does not
  exist, translate it to this API rather than inventing one — the snippet shows
  the assertion intent, the real API is binding.
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
- Produces:
  - `pub fn stage_name(Stage) -> &'static str` — public API, re-exported at the
    crate root as `middleware_stage_name` in Task 2
  - `pub(crate) fn parse_stage(&str) -> Option<Stage>` — crate-internal, only
    the round-trip test needs it
  - `pub struct MiddlewareStageContext` with `pub fn middleware_context(&self,
    index: usize) -> Result<MiddlewareContext, MiddlewareError>` — public API,
    re-exported in Task 3, used by every later stage task
  - `pub async fn invoke_middleware_stage(&ResolvedMiddlewareChain,
    &MiddlewareStageContext, StageInput) -> Result<Vec<StageOutcome>,
    MiddlewareError>` — public API, re-exported in Task 3

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
///
/// Constructed once per `(cycle, stage)` boundary and reused for every component
/// in that stage, so all components in a stage observe the same run identity and
/// the same locked chain digest.
#[derive(Debug, Clone)]
pub struct MiddlewareStageContext {
    /// Run-scoped call context reused for every component in the stage.
    pub run: RunCallContext,
    /// Locked chain digest from the resolved agent.
    pub chain_digest: Digest,
    /// Committed parent effect this stage runs under, used to validate any
    /// child compaction-model effect.
    pub parent: EffectRequested,
}

impl MiddlewareStageContext {
    /// Per-component invocation context for the component at `index`.
    ///
    /// # Errors
    ///
    /// Returns a stable `middleware_resolution_invalid` when the chain index
    /// exceeds `u32`.
    pub fn middleware_context(
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
pub async fn invoke_middleware_stage(
    chain: &ResolvedMiddlewareChain,
    ctx: &MiddlewareStageContext,
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

In `crates/finstack-ai-runtime/src/middleware.rs`, widen line 1509 from
`fn stage_name(` to `pub fn stage_name(` and line 1521 from `fn parse_stage(`
to `pub(crate) fn parse_stage(`. `stage_name` becomes public API (re-exported as
`middleware_stage_name` in Task 2) and so needs a doc comment to satisfy
`#![warn(missing_docs)]`; `parse_stage` stays crate-internal because only the
round-trip test needs it.

In `crates/finstack-ai-runtime/src/lib.rs`, add `pub mod middleware_driver;`
immediately after the existing `mod middleware;` line. The crate-root re-export
of its two items lands in Task 3, together with the compatibility-baseline
update.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p finstack-ai-runtime --locked middleware_driver`
Expected: PASS

- [ ] **Step 5: Verify no public surface moved**

Run: `uv run --no-project python scripts/compat/public_items.py --check`
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
- Consumes: `MiddlewareStageContext` (Task 1), `StageIds` unchanged
- Produces — every later task uses these exact names and signatures:
  - `const fn StageIds::for_outcome(&ReducerStageOutcome) -> Self`
  - `fn Agent::stage_run_context(&self, cycle: u64, stage: Stage) -> Result<MiddlewareStageContext, AgentRunError>`
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
    ) -> Result<MiddlewareStageContext, AgentRunError> {
        let chain_digest = self.resolved.run_plan().middleware_chain().digest();
        let mut seed = Vec::with_capacity(64);
        seed.extend_from_slice(chain_digest.as_bytes());
        seed.extend_from_slice(&cycle.to_be_bytes());
        seed.extend_from_slice(crate::middleware_stage_name(stage).as_bytes());
        Ok(MiddlewareStageContext {
            run: self.run_call_context(EffectId::from_bytes(effect_id_bytes(&seed)))?,
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

Export the stage-name accessor once, in
`crates/finstack-ai-runtime/src/lib.rs`. Task 1 made `stage_name` `pub(crate)`;
widen it to `pub` with a doc comment and re-export it under its unambiguous
crate-root name:

```rust
pub use middleware::stage_name as middleware_stage_name;
```

Add `finstack-ai-runtime::middleware_stage_name` to
`fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`
in sorted position.

Also add the deterministic id derivation used by `stage_run_context` above.
`EffectId` is `Id<EffectTag>` and its only constructor is
`Id::from_bytes([u8; 16])` (`crates/finstack-ai-kernel/src/ids.rs:69`) — there is
no `from_digest`:

```rust
/// Derive 16 stable bytes from a seed for a non-journaled correlation id.
fn effect_id_bytes(seed: &[u8]) -> [u8; 16] {
    let digest = Digest::raw_json(seed);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    bytes
}
```

- [ ] **Step 6: Confirm the existing suite is unaffected**

Run: `cargo test -p finstack-ai --locked`
Expected: PASS — nothing calls the new helpers yet, so behavior is unchanged.

---

### Task 3: Stage allocation table, deadline fix, and the 8bec785 revert

Commit `8bec785` added `StageIds::for_outcome` to the facade. It is **wrong**,
not merely misplaced: it keys allocation on the outcome alone, while the kernel
keys on `(stage, outcome)` in `stage_id_requirements`
(`crates/finstack-ai-kernel/src/reducer/decide.rs:1101-1183`).

- `Fail(_) -> finalize()` = `(2,1,0,0,0,0)` is correct only at `BeforeFinalize`;
  the kernel requires `(1,0,0,0,0,0)` at the other six (`decide.rs:1165-1177`).
  A middleware `StageOutcome::Fail` is legal at **every** stage
  (`middleware.rs:920`), so this arm will be hit and will be wrong.
- `ContinueModel { .. } -> finalize()` is wrong; kernel requires
  `(1,0,0,0,0,0)` (`decide.rs:1146-1157`).
- `ToolBatchPrepared { .. } -> (2,1,0,0,0,0)` is unusable; the real allocation
  is `tool_opening_counts` (`settlement.rs:526-561`) plus `allocate_tool_opening`
  (`settlement.rs:742-763`), and `StageIds` models no tool batches, tool calls,
  cancellations, or interactions at all.

This task replaces it with a runtime-side allocator that mirrors the kernel
arm-for-arm, and fixes a **pre-existing bug** found while mapping:
`fail_closed_on_run_deadline` (`settlement.rs:227`) submits
`ReducerStageOutcome::Continue` at `Stage::BeforeToolBatch`, but the kernel
admits `Continue` only at `BeforeRun`/`AfterModel`/`AfterToolBatch`
(`decide.rs:1107-1111`). That path is untested today.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/settlement.rs`
- Modify: `crates/finstack-ai/src/agent.rs` (revert the 8bec785 additions)
- Modify: `crates/finstack-ai-runtime/src/lib.rs` (revert the 8bec785 re-export line only if it becomes unused — check first)
- Modify: `fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`
- Test: `crates/finstack-ai-runtime/src/settlement.rs` inline tests

**Interfaces:**
- Consumes: `stage_id_requirements` semantics from `decide.rs:1101-1183` (read it; do not guess), `allocate_tool_opening` (`settlement.rs:742`), `tool_opening_counts` (`settlement.rs:526`)
- Produces: `pub(crate) fn stage_allocation<C: Clock, R: RandomSource>(state: &KernelState, cursor: StageCursor, outcome: &ReducerStageOutcome, sources: &SettlementSources<C, R>) -> Result<AllocatedIds, RunHandleError>` — used by Tasks 6 and 7

- [ ] **Step 1: Write the failing test**

The test that matters is the one that would have caught the bug in `for_outcome`
— allocation must differ by stage for the same outcome:

```rust
#[test]
fn fail_allocation_differs_between_before_finalize_and_other_stages() {
    let state = accepted_state();
    let sources = test_sources();
    let fail = ReducerStageOutcome::Fail(ErrorDescriptor::new(
        "probe_failed",
        "probe",
        ErrorCategory::Validation,
        false,
    ));

    let at_finalize = stage_allocation(
        &state,
        StageCursor { cycle: 0, stage: Stage::BeforeFinalize },
        &fail,
        &sources,
    )
    .expect("finalize allocation");

    let at_before_run = stage_allocation(
        &state,
        StageCursor { cycle: 0, stage: Stage::BeforeRun },
        &fail,
        &sources,
    )
    .expect("before_run allocation");

    assert_eq!(at_finalize.records.len(), 2, "BeforeFinalize Fail needs 2 records");
    assert_eq!(at_finalize.events.len(), 1, "BeforeFinalize Fail needs 1 event");
    assert_eq!(at_before_run.records.len(), 1, "BeforeRun Fail needs 1 record");
    assert_eq!(at_before_run.events.len(), 0, "BeforeRun Fail needs 0 events");
}

#[test]
fn run_deadline_fail_closed_uses_a_stage_legal_outcome() {
    // fail_closed_on_run_deadline settles Stage::BeforeToolBatch. The kernel
    // rejects Continue there (decide.rs:1107-1111), so the fail-closed path
    // must not emit Continue.
    let outcome = run_deadline_outcome();
    assert!(
        !matches!(outcome, ReducerStageOutcome::Continue),
        "BeforeToolBatch cannot accept Continue; got {outcome:?}"
    );
}
```

Adjust `AllocatedIds` field names to the real ones — read the struct before
writing the asserts. `accepted_state()` and `test_sources()` follow the existing
fixture helpers in `settlement.rs`'s own test module; reuse them rather than
inventing new ones.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-runtime --locked stage_allocation run_deadline_fail_closed`
Expected: FAIL — no `stage_allocation` function.

- [ ] **Step 3: Implement the allocator**

Write `stage_allocation` mirroring `decide.rs:1101-1183` **arm for arm**. Open
`decide.rs` side by side and transcribe; do not infer the tuples. The six-tuple
order is `(records, events, effects, turns, model_requests, messages)`, verified
against `IdRequirements::new` (`crates/finstack-ai-kernel/src/reducer/allocated_ids.rs:22-29`).

For `ReducerStageOutcome::ToolBatchPrepared { calls, .. }`, do not compute a
tuple — delegate to the existing `allocate_tool_opening` (`settlement.rs:742`),
which already accounts for `tool_opening_counts` plus one `ToolBatchId` and one
`AppendBatchId`.

Return `RunHandleError` on any `(stage, outcome)` pair the kernel rejects, so an
unlandable fold fails loudly at allocation time rather than being rejected later
by the reducer with a less specific error.

- [ ] **Step 4: Fix the deadline path**

Rewrite `fail_closed_on_run_deadline` (`settlement.rs:227-262`) to submit an
outcome the kernel actually admits at `Stage::BeforeToolBatch`, and to take its
ids from `stage_allocation` rather than the hand-built `AllocatedIds` at
`settlement.rs:239-254`. Read `decide.rs` to determine which outcome is correct
there — do not guess between `ToolBatchPrepared` and `Fail`.

This changes behavior on a previously untested path. Add a test that drives the
run-deadline path end to end and asserts the kernel accepts the submission.

- [ ] **Step 5: Revert the 8bec785 additions**

In `crates/finstack-ai/src/agent.rs`, remove: `StageIds::for_outcome`,
`AgentRunError::from_middleware`, `canonical_messages`, `canonical_message`,
`parse_messages`, `canonical_terminal_candidate`, and `compactor_descriptor`.
All seven carry `#[allow(dead_code, ...)]` and have no callers. The codec
helpers are re-created in the runtime in Task 6, on live coordinator state
rather than the facade's recovered copy. Leave the `#[derive(Debug, PartialEq,
Eq)]` on `StageIds` only if something still needs it; otherwise revert that too.

`AgentRunError::from_middleware` is superseded by the `RunHandleError::Middleware`
variant added in Task 9 — the facade needs no mapping because
`handle.submit(...).map_err(AgentRunError::runtime)` (`agent.rs:2452`) already
takes the whole `RunHandleError`.

Check whether the `middleware_stage_name` re-export in
`crates/finstack-ai-runtime/src/lib.rs` still has a consumer. It is used by the
driver, so it very likely stays — if it stays, leave both it and its fixture
line alone. If you remove it, you MUST also remove its line from
`fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`
or `public_item_lists_reject_renames` will fail.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Run: `cargo test -p finstack-ai --locked`
Run: `uv run --no-project python scripts/compat/public_items.py --check`
Expected: all PASS, compat exits 0.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit
```

---

### Task 4: Stage identity — coordinator seed, chain install, derived effect id

The runtime can build a faithful `RunCallContext` for every field except
`effect_id`, which has no faithful value anywhere: no `KernelInput` commits an
`EffectKind::Middleware` effect, and stage settlement emits only
`StageOutcomeRecorded` (`decide.rs:2369`). This task supplies the seed and a
deterministic derived id, and gives the facade a way to hand over the chain.

**⚠️ Concurrency note:** another session may be editing
`crates/finstack-ai-runtime/src/coordinator.rs` to fix two pre-existing clippy
errors (unused import at `:1688`, dead struct at `:1902`, both in the test
module). Run `git pull --rebase` / check `git log` before starting, and if that
fix has not landed, coordinate before editing this file.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/coordinator.rs`
- Modify: `crates/finstack-ai-runtime/src/middleware_driver.rs`
- Modify: `crates/finstack-ai/src/agent.rs` (exactly one added line)
- Test: inline tests in both runtime files

**Interfaces:**
- Consumes: `dispatch_security_context` (`coordinator.rs:1630`, private free fn), `state.accepted.effective_deadline()`, `state.retry.attempts`
- Produces:
  - `pub fn CommitCoordinator::install_middleware_chain(&mut self, chain: Arc<ResolvedMiddlewareChain>)` and `pub fn CommitCoordinator::middleware_chain(&self) -> Option<&Arc<ResolvedMiddlewareChain>>`
  - `pub(crate) struct StageDispatchSeed` and `pub(crate) fn stage_dispatch_seed(...)`
  - `pub fn derived_stage_effect_id(locator: &OperationLocator, cycle: u64, stage: Stage) -> EffectId`
  - `MiddlewareStageContext` with `parent` **removed** and `cursor: StageCursor` added, plus `pub const fn new(run, chain_digest, cursor)` and `pub const fn stage(&self)`

Read `.superpowers/sdd/2026-08-15-middleware-chain-driver/runtime-driver-design.json`
(`design.signatures`) for the exact verified signatures before writing.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn derived_stage_effect_id_is_deterministic_across_calls() {
    let locator = test_locator();
    let a = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
    let b = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
    assert_eq!(a, b, "derived id must be stable so replay reproduces it");
}

#[test]
fn derived_stage_effect_id_separates_stage_and_cycle() {
    let locator = test_locator();
    let base = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
    assert_ne!(base, derived_stage_effect_id(&locator, 4, Stage::BeforeModel));
    assert_ne!(base, derived_stage_effect_id(&locator, 3, Stage::AfterModel));
}

#[test]
fn installed_chain_is_retrievable() {
    let mut coordinator = test_coordinator();
    assert!(coordinator.middleware_chain().is_none());
    let chain = Arc::new(ResolvedMiddlewareChain::try_new(Vec::new()).expect("empty chain"));
    coordinator.install_middleware_chain(Arc::clone(&chain));
    assert!(coordinator.middleware_chain().is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-runtime --locked derived_stage_effect_id installed_chain`
Expected: FAIL — functions do not exist.

- [ ] **Step 3: Implement**

`derived_stage_effect_id` uses `Digest::domain_separated("middleware-stage-invocation", 1, ...)`
(`crates/finstack-ai-kernel/src/digest.rs:102`) over `(locator, cycle, stage_name(stage))`,
then takes the first 16 bytes of `digest.as_bytes()` (`digest.rs:56`, returns
`&[u8; 32]`) into `EffectId::from_bytes` (`ids.rs:72`, `pub const`). Domain
separation and the derived-not-random construction are what keep it stable
across replay and non-colliding with UuidV7 committed effect ids.

Document on the function, in prose, that this id is **not** a committed effect
identity — it is a correlation id — and that anything treating it as journaled
is wrong.

`install_middleware_chain` / `middleware_chain` are additive public peers of
`install_dispatcher` (`coordinator.rs:825`) and `install_event_publisher`
(`:851`); those are `pub(crate)` and therefore unreachable from the facade,
which is why these two must be `pub`.

`StageDispatchSeed` is the exact peer of `pending_model_seed` (`coordinator.rs:282`)
and `pending_tool_seeds` (`:297`), built from the private `dispatch_security_context`.

Change `MiddlewareStageContext`: remove `parent: EffectRequested` (no producer
exists), add `cursor: StageCursor`. Both `parent`'s only consumer,
`validate_compaction_model_effect` (`middleware.rs:859`), and the compaction
child-effect round trip are out of scope for this plan — record that in Task 9's
docs rather than leaving a field nothing can fill.

- [ ] **Step 4: Add the one facade line**

Between `agent.rs:598` and the `RunTaskOwner::spawn_with_model*` calls at
`agent.rs:614`/`:626`, add:

```rust
coordinator.install_middleware_chain(Arc::clone(self.resolved.run_plan().middleware_chain()));
```

`ResolvedRunPlan::middleware_chain()` returns `&Arc<ResolvedMiddlewareChain>`
(`registry.rs:1523`) and the facade already holds the `CommitCoordinator` there.
Do **not** change `RunTaskConfig` — it is `Copy` (`run_types.rs:59`) and cannot
carry an `Arc`. Do **not** change the public `spawn_with_model` /
`spawn_with_model_and_tools` signatures.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Run: `cargo test -p finstack-ai --locked`
Expected: PASS — nothing consumes the seed or chain yet, so no behavior changed.

- [ ] **Step 6: Commit**

---

### Task 5: The pure fold and the `StageDriver` handle

Pure logic, no IO, no coordinator. This is the semantic core and it is the task
most worth reviewing carefully.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/middleware_driver.rs`
- Test: inline tests

**Interfaces:**
- Consumes: `MiddlewareStageContext` (Task 4), `validate_stage_outcome` (`middleware.rs:912`), the legality table from Task 3's `stage_allocation`
- Produces: `pub struct StageFold`, `pub enum StageTerminal`, `StageFold::accumulate(stage, &[StageOutcome]) -> Result<Self, MiddlewareError>`, `StageFold::is_identity() -> bool`, `pub struct StageDriver` with `new`/`chain`/`is_active`/`cancellation`/`run_stage`, and the stable codes `MIDDLEWARE_STAGE_UNLANDABLE` and `MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn empty_outcomes_fold_to_identity() {
    let fold = StageFold::accumulate(Stage::PrepareContext, &[]).expect("fold");
    assert!(fold.is_identity(), "an empty chain must not perturb the base outcome");
}

#[test]
fn add_context_accumulates_in_chain_order() {
    let fold = StageFold::accumulate(
        Stage::PrepareContext,
        &[
            StageOutcome::AddContext(Arc::from([item("first")])),
            StageOutcome::AddContext(Arc::from([item("second")])),
        ],
    )
    .expect("fold");
    let texts: Vec<_> = fold.context.iter().map(|i| i.text().to_owned()).collect();
    assert_eq!(texts, vec!["first", "second"], "later components append after earlier");
}

#[test]
fn filter_tools_intersects_so_order_cannot_matter() {
    let ab = StageFold::accumulate(
        Stage::BeforeModel,
        &[
            StageOutcome::FilterTools(Arc::from([tool_id("a"), tool_id("b")])),
            StageOutcome::FilterTools(Arc::from([tool_id("b"), tool_id("c")])),
        ],
    )
    .expect("fold");
    let ba = StageFold::accumulate(
        Stage::BeforeModel,
        &[
            StageOutcome::FilterTools(Arc::from([tool_id("b"), tool_id("c")])),
            StageOutcome::FilterTools(Arc::from([tool_id("a"), tool_id("b")])),
        ],
    )
    .expect("fold");
    assert_eq!(ab.retained_tools, ba.retained_tools, "intersection must be order-independent");
    assert_eq!(ab.retained_tools.unwrap().len(), 1, "only b survives");
}

#[test]
fn first_terminal_short_circuits_the_rest_of_the_chain() {
    let fold = StageFold::accumulate(
        Stage::AfterModel,
        &[
            StageOutcome::Fail(Box::new(descriptor("first_failure"))),
            StageOutcome::Fail(Box::new(descriptor("second_failure"))),
        ],
    )
    .expect("fold");
    match fold.terminal {
        Some(StageTerminal::Fail(d)) => assert_eq!(d.code(), "first_failure"),
        other => panic!("expected first Fail to win, got {other:?}"),
    }
}

#[test]
fn unlandable_outcome_is_a_stable_error_not_a_silent_drop() {
    let error = StageFold::accumulate(
        Stage::BeforeRun,
        &[StageOutcome::Complete(RawJson::parse(b"{}").unwrap())],
    )
    .expect_err("Complete has no kernel landing at BeforeRun");
    assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-runtime --locked stage_fold`
Expected: FAIL — no `StageFold`.

- [ ] **Step 3: Implement the fold**

Left-to-right over the ordered outcomes. `Continue` is a no-op.
`AddInstructions`/`AddContext` append in chain order.
`FilterTools` **intersects** — narrowing is monotone, so component order cannot
change the result. `Replace` substitutes. `Fail` then `Retry` short-circuit as
`StageTerminal`.

Outcomes with no kernel landing path at the given stage — `Suspend`, `Complete`,
`RequestInteraction`, `RequestCompactionModel`, and `Retry`/`Replace` at stages
the kernel rejects them — become an explicit `MIDDLEWARE_STAGE_UNLANDABLE`
error. Never silently drop one.

Enforce bounds in the fold, because they move from the kernel to the driver
once middleware can add items: `ContextPrepared.messages` past
`SEMANTIC_ARRAY_MAX_ITEMS` (kernel rejects at `decide.rs:1195-1202`) and
`draft.messages` past `ModelRequestDraft::MAX_MESSAGES` are
`MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`.

`StageDriver` holds the `Arc<ResolvedMiddlewareChain>` and a `CancellationSignal`;
`is_active(stage)` is the passthrough gate — `false` when the chain has no
component for that stage.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Expected: PASS

- [ ] **Step 5: Commit**

---

### Task 6: Wire the six facade-authored stages through the choke point

Every facade stage settlement funnels through `RunHandle::submit`
(`task.rs:79-103`) into one `coordinator.submit(command.env, command.input)`
call. Intercepting `KernelInput::StageSettled` there catches `BeforeRun`,
`PrepareContext`, `BeforeModel`, `AfterModel`, `AfterToolBatch`, and
`BeforeFinalize` with **no change to the facade's stage sequence**.

**Governing invariant — passthrough when the chain is empty.** If no driver is
installed or `!driver.is_active(cursor.stage)`, submit the facade's `env` and
`input` byte-for-byte unchanged. Every existing journal, digest, and test must
be unaffected; the feature is opt-in per agent. A reviewer should be able to
verify this by running the full existing suite untouched.

**Files:**
- Create: `crates/finstack-ai-runtime/src/stage_settlement.rs`
- Modify: `crates/finstack-ai-runtime/src/task.rs` (`:1000`, `:871`)
- Modify: `crates/finstack-ai-runtime/src/host_task.rs` (`:983`)
- Modify: `crates/finstack-ai-runtime/src/lib.rs` (`mod stage_settlement;`)
- Test: `crates/finstack-ai-runtime/src/stage_settlement.rs` inline tests

**Interfaces:**
- Consumes: `stage_allocation` (Task 3), `StageDispatchSeed`/`derived_stage_effect_id` (Task 4), `StageFold`/`StageDriver` (Task 5)
- Produces: `pub(crate) async fn settle_facade_stage(...)`, `pub(crate) async fn run_stage_chain(...)`, `pub(crate) fn apply_context_prepared(...)` — `run_stage_chain` is reused by Task 7

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn no_chain_installed_submits_the_facade_input_unchanged() {
    let mut coordinator = test_coordinator();
    let env = test_env();
    let input = stage_settled(Stage::BeforeRun, ReducerStageOutcome::Continue);
    let before = coordinator.state().clone();

    settle_facade_stage(&mut coordinator, None, &test_sources(), env.clone(), input.clone())
        .await
        .expect("passthrough");

    assert_eq!(
        coordinator.state().cycle,
        before.cycle,
        "passthrough must not perturb state beyond the plain submit"
    );
}

#[tokio::test]
async fn prepare_context_middleware_adds_messages_to_the_committed_outcome() {
    let mut coordinator = test_coordinator();
    let driver = driver_adding_context("injected-by-middleware");
    let base = ReducerStageOutcome::ContextPrepared { messages: Arc::from([user("hi")]) };

    settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &test_sources(),
        test_env(),
        stage_settled(Stage::PrepareContext, base),
    )
    .await
    .expect("settles");

    let committed = coordinator.state().current_turn.as_ref().expect("turn");
    assert!(
        committed.context.messages.iter().any(|m| m.text().contains("injected-by-middleware")),
        "middleware context never reached the committed ContextPrepared"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-runtime --locked settle_facade_stage`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement**

`settle_facade_stage` destructures the incoming `StageSettled { cursor, outcome }`,
returns the plain `coordinator.submit(env, input)` when the driver is inactive,
and otherwise: builds the `StageInput` for the cursor's stage, calls
`run_stage_chain`, applies the fold to the base outcome, re-allocates ids via
`stage_allocation` when the folded requirement tuple differs from the base, and
submits the folded outcome.

Recreate the four codec helpers reverted in Task 3 here, in the runtime, on
`canonical_bytes`/JCS and on live `coordinator.state()` rather than the facade's
recovered copy.

Add the `KernelInput::StageSettled` branch to all three command loops. Missing
one means middleware silently no-ops on that worker.

**Re-allocation note:** when the folded tuple differs from the base, the ids the
facade already minted (`agent.rs:2200-2210`) are discarded and new ones taken
from `SettlementSources`, which shares the run's clock and random source.
Document this in the function.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Run: `cargo test --workspace --locked`
Expected: PASS — the passthrough invariant means the existing suite is untouched.

- [ ] **Step 5: Commit**

---

### Task 7: `BeforeToolBatch` via the existing `decide_plan` policy hook

The one stage the facade never settles. It needs no new machinery:
`ResolvedToolCatalog::decide_plan` already takes `middleware: Option<ToolPolicyDecision>`
(`tool.rs:844`) and `settlement.rs:186` passes `None` today.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/settlement.rs`
- Modify: `crates/finstack-ai-runtime/src/stage_settlement.rs`
- Modify: `crates/finstack-ai-runtime/src/task.rs`, `host_task.rs` (six call sites)
- Test: `settlement.rs` inline tests

**Interfaces:**
- Consumes: `run_stage_chain` (Task 6), `StageDriver` (Task 5)
- Produces: `prepare_tool_batch_if_ready` gains a `driver: Option<&StageDriver>` parameter

- [ ] **Step 1: Write the failing tests**

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
async fn fully_filtered_batch_still_commits_every_source_call() {
    // prepare_tool_batch_if_ready guards on source calls.is_empty()
    // (settlement.rs:167-172), NOT on plans, so denying every call still
    // commits a batch of all-SyntheticClosure plans.
    let plans = prepare_with_chain(retain_nothing_chain(), three_tool_calls()).await;
    assert_eq!(plans.len(), 3);
    assert!(plans.iter().all(|p| matches!(p, ToolCallPlan::SyntheticClosure(_))));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-runtime --locked filtered_tool_call fully_filtered_batch`
Expected: FAIL — no `driver` parameter.

- [ ] **Step 3: Implement**

Run the chain after the source `calls` are collected (`settlement.rs:161-179`)
and **before** the `catalog.decide_plan(...)` loop (`:183-192`), so the folded
`FilterTools` set feeds `decide_plan`'s existing `middleware` parameter. The base
`ToolBatchPrepared` (`:203-206`) and `allocate_tool_opening` (`:207`) are then
computed from the post-filter plans.

**Update all six call sites**: `task.rs:598`, `task.rs:952`, `task.rs:1021`,
`host_task.rs:387`, `host_task.rs:1030`, `host_task.rs:1139`. Missing one means
`BeforeToolBatch` middleware runs on some resume paths and not others — a silent
correctness hole, not a compile error, if any site passes `None` by default.

The fail-closed rule from Task 3 stands: the run-deadline path bypasses the
chain entirely.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Run: `cargo test --workspace --locked`
Expected: PASS

- [ ] **Step 5: Commit**

---

### Task 8: `BeforeModel` — typed `StageInput` and model-draft folding

`StageInput::BeforeModel` is the only typed stage input. `BeforeModelInput`
(`middleware.rs:217-251`) needs `request: ModelRequestDraft`, `source_entries:
Arc<[CompactionSourceEntry]>`, `model_context_profile_digest`,
`hard_input_tokens`, and `checkpoint`.

`LockedModelContextProfile` arrives at `task.rs:300`/`:506` and is immediately
moved into `ModelDispatcher::new` (`task.rs:549-554`); the worker retains
nothing. It must be cloned before that move and threaded into the three worker
functions.

**Files:**
- Modify: `crates/finstack-ai-runtime/src/stage_settlement.rs`
- Modify: `crates/finstack-ai-runtime/src/task.rs`, `host_task.rs`
- Test: `stage_settlement.rs` inline tests

**Interfaces:**
- Consumes: `run_stage_chain` (Task 6), `validate_compaction_result` (`middleware.rs:984`)
- Produces: `apply_model_draft(fold, draft) -> Result<ModelRequestDraft, RunHandleError>`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn before_model_filter_tools_narrows_the_model_draft() {
    let draft = drive_before_model(retain_nothing_chain(), draft_with_tools(2)).await;
    assert!(draft.tools.is_empty(), "FilterTools did not narrow the draft");
}

#[tokio::test]
async fn compact_context_replaces_only_the_projection() {
    let draft = drive_before_model(compacting_chain("SUMMARY"), draft_with_messages(10)).await;
    assert_eq!(draft.messages.len(), 1, "projection replaced");
    assert!(draft.messages[0].text().contains("SUMMARY"));
}

#[tokio::test]
async fn post_compaction_validator_may_not_add_context() {
    let error = drive_before_model_err(validator_returning_add_context()).await;
    assert!(
        format!("{error}").contains("middleware_outcome_not_allowed"),
        "the post-compaction narrowing (middleware.rs:956) must reject AddContext"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-runtime --locked before_model`
Expected: FAIL

- [ ] **Step 3: Implement**

Clone `LockedModelContextProfile` before the `ModelDispatcher::new` move and
thread it into `run_worker_with_model`, `run_worker_with_model_and_tools`, and
`run_worker_with_effects`.

Assemble `BeforeModelInput` from the base `ModelRequestPrepared` outcome plus
live coordinator state. `protected` comes from the context port, which is its
authoritative setter (`context.rs:138`) — the compactor cannot set it.

Apply the fold to the draft: `AddInstructions`/`AddContext` append,
`FilterTools` intersects, `CompactContext` replaces the projection after
`validate_compaction_result` passes, `Replace` substitutes.

`RequestCompactionModel` is **out of scope for this plan** — it requires a
committed child model effect under `EffectPurpose::CompactionSummary` and a
re-entry with `compaction_resume`, which the aggregate-fold design has no place
for. It folds to `MIDDLEWARE_STAGE_UNLANDABLE`. Record this in Task 9's docs as
a known limitation of the aggregate design.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-runtime --locked`
Run: `cargo test --workspace --locked`
Expected: PASS

- [ ] **Step 5: Commit**

---

### Task 9: Documentation, stable-code registry, and public-API freeze

**Files:**
- Modify: `crates/finstack-ai-runtime/src/middleware_driver.rs`, `stage_settlement.rs`, `lib.rs`, `run_types.rs`
- Modify: `fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`
- Create: `docs/site/middleware.md`

**Interfaces:**
- Consumes: every public item added by Tasks 4-8
- Produces: `RunHandleError::Middleware { code: Arc<str> }`, the frozen public-item baseline

- [ ] **Step 1: Add the error variant**

`RunHandleError::Middleware { code: Arc<str> }`, peer of
`RunHandleError::Model` (`run_types.rs:164`) and `Tool` (`:176`).
`MiddlewareError::code() -> &str` (`middleware.rs:1202`) supplies the value.
The facade needs no edit — `AgentRunError::runtime` takes the whole
`RunHandleError` (`agent.rs:2452`).

- [ ] **Step 2: Write the module contract**

A module-level doc section on `middleware_driver` stating, in prose:

1. **The aggregate-fold invariant.** N `StageOutcome`s fold into exactly one
   `ReducerStageOutcome` per `(cycle, stage)` cursor, reaching the kernel only
   through `KernelInput::StageSettled`.
2. **Replay safety is a contract, not a type.** Invocations are never journaled;
   on recovery a cursor whose `StageOutcomeRecorded` is absent re-runs its
   entire chain. A middleware that performs a non-repeatable external action
   will repeat it.
3. **The derived effect id is not a real effect.** `RunCallContext.effect_id` at
   a stage boundary is a correlation id, not a committed identity. It is
   projected across the WIT boundary by `sanitize_call_context`
   (`plugins/finstack-ai-wit/src/mapping.rs:22`), so a host that treats it as
   journaled is wrong.
4. **First-terminal-wins short-circuits.** When component 2 of 4 returns `Fail`,
   components 3 and 4 never run — so the resolved order (`OrderTier`, then
   `order.before`/`after`, then `registration_index`) is semantically
   load-bearing.
5. **Known limitations of the aggregate design:** `Suspend` and `Complete` have
   no landing path; `RequestCompactionModel` is unsupported; `Middleware::reconcile`,
   `CommittedMiddlewareCall`, `RecordedMiddlewareOutcome`,
   `middleware_resume_action`, and `PendingMiddlewareEffect` are never called.

- [ ] **Step 3: Freeze the public surface**

Add every new `pub use` name to
`fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`
in sorted position.

Run: `uv run --no-project python scripts/compat/public_items.py --check`
Expected: exit 0.

Run: `cargo test -p finstack-ai-test --offline --locked --test breaking_change`
Expected: PASS.

- [ ] **Step 4: Commit**

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
5. **The driver is supported public API, not a hidden shim.** Decided
   2026-08-16. `finstack-ai-runtime` gains three documented `pub use` entries —
   `invoke_middleware_stage`, `MiddlewareStageContext`, and
   `middleware_stage_name` — because the driver must be callable from both the
   facade crate and `settlement.rs` inside the runtime crate. The alternative,
   moving the driver into the facade, was rejected: `BeforeToolBatch` is settled
   in `settlement.rs`, so it would have cut one of the landable variants.
   `#[doc(hidden)]` was rejected as understating a real commitment. Record the
   1.0 compatibility obligation this creates, and confirm the three names were
   added to
   `fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt`.

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
