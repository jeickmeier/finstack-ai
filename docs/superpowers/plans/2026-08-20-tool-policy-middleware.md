# Tool Policy Filter Middleware Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single middleware leaf crate, `finstack-ai-middleware-tool-policy`, that runs at `before_model` and `before_tool_batch` and returns `StageOutcome::FilterTools` so the model-visible tool set is policy-narrowed — with role allowlists, write budgets, jailbreak triggers, and child-depth gates as configs on this one crate.

**Architecture:** The runtime already owns the whole enforcement mechanism: `StageOutcome::FilterTools(Arc<[ToolId]>)` has retain semantics ("retain only the named tools"), the fold intersects every leaf's retain set (`crates/finstack-ai-runtime/src/exec/middleware_driver/fold.rs:111-117`), `before_model` narrowing lands on `ModelRequestDraft.tools` (`crates/finstack-ai-runtime/src/exec/stage_settlement/apply.rs:161-169`), and `before_tool_batch` narrowing becomes a per-call `ToolPolicyDecision::Deny` → `SyntheticClosure` (`crates/finstack-ai-runtime/src/exec/settlement/stage.rs:293-306`). This crate is therefore a *pure decision function*: config + stage input → retain set (or `Continue`/`Fail`). No runtime changes, no new fold semantics, no state, no store.

**Tech Stack:** Rust workspace crate under `extensions/middleware/`, mirroring `finstack-ai-middleware-verify` (structure) and `finstack-ai-middleware-compaction` (config + digest pattern). Deps: `finstack-ai-kernel`, `finstack-ai-runtime` (default-features = false), `serde`, `serde_json`, `thiserror`.

**Spec:** The user request (this plan's §"Spec deviations" records where reality differed) plus the written fold contract: `crates/finstack-ai-runtime/src/exec/middleware_driver/mod.rs:1-139` and `docs/site/middleware.md` (stage/outcome matrix row: `FilterTools` legal at `before_model`, `before_tool_batch` only — enforced at `crates/finstack-ai-runtime/src/ports/middleware/validate.rs:100-102`).

## Spec deviations (verified against the tree, 2026-08-20)

1. **There is no "§7.2 policy filter" doc.** The two §7.2s in `docs/planning/` are "Agent resolution" (02-architecture-spec) and "Reference memory composition" (05-future-capabilities). The authoritative filter contract is `docs/site/middleware.md` + the `middleware_driver` module docs. The plan cites those instead.
2. **"Sandbox" ships as a toolset** (`extensions/toolsets/finstack-ai-sandbox-e2b`), not middleware. Irrelevant to this crate; listed only so nobody goes looking for a sandbox middleware to imitate.
3. **No write-budget or jailbreak primitive exists anywhere** (grep-verified). Both are defined fresh here as pure functions of the stage input, so the leaf stays `InvocationRecovery::RecomputeSafe`.
4. **Middleware cannot see run depth.** `RunCallContext`/`OperationLocator` carry tenant/session/lane/run ids only — no `RunRelation.depth`. Depth enforcement today lives in the SDK child layer (`crates/finstack-ai/src/agent/child.rs:766-786`, `ChildRunPolicy::Allow { max_depth }`). So the child-depth gate takes `current_depth` **at construction time**: the host that resolves a child agent knows its depth and configures the leaf accordingly. This composes with (does not replace) `ChildRunPolicy`.
5. **`before_tool_batch` input is only the batch** (`StageInput::BeforeToolBatch { value }` = canonical `&[ToolCallBlock]`, `crates/finstack-ai-runtime/src/exec/stage_settlement/tool_batch.rs:67-69`) — no message history, no catalog. Because `FilterTools` is retain-semantics, a leaf at that stage may only emit a *complete* allow set. Role allowlists and child-depth gates can express one (their config enumerates `ToolId`s); write budgets and jailbreak triggers cannot (they need message history), so those two rules run at `before_model` only. The batch stage is defense-in-depth against a provider emitting calls to tools the model was never shown.

## Global Constraints

- Crate version is lockstep `version.workspace = true` → 1.0.0 (`docs/implementation/1.0-leaf-versioning.md`: "Do not invent per-leaf versions").
- Every leaf `Cargo.toml` ends with `[lints] workspace = true`; runtime dep is `finstack-ai-runtime = { workspace = true, default-features = false }` (`.agents/rules/01-engineering-conformance.md:38`).
- Crate-level lint preamble copied verbatim from `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs:1-22` (`#![forbid(unsafe_code)]`, `#![deny(clippy::unwrap_used)]`, etc.).
- Unit tests live in `src/tests.rs` behind `#[cfg(test)] mod tests;` — not `tests/` (house style for extension leaves).
- `MiddlewareRole::Standard` + `OrderTier::RequestShaping` ("Request and policy shaping", `crates/finstack-ai-runtime/src/ports/middleware/types.rs:75-77`). Never `PostCompactionValidator` — that role may not return `FilterTools` (`validate.rs:126-136`).
- Config is `Serialize`-only (no `Deserialize`) — serialization exists purely to compute the descriptor's `configuration_digest` via `Digest::raw_json(&serde_json::to_vec(&config)?)`, per `finstack-ai-middleware-compaction/src/lib.rs:145-150`. Construction is `try_new` + `with_*` builders with private fields (house style: `ShellPolicy`, `McpConfig`).
- Public API baselines are bidirectional (additions also fail CI). The new crate is auto-discovered by `scripts/compat/public_api.py`; its baseline must be generated in the same PR: `uv run --no-project python scripts/compat/public_items.py --write`.
- Verification commands: `mise run check-rust` (fmt + clippy `-D warnings`), `cargo nextest run -p finstack-ai-middleware-tool-policy --locked` per task, `mise run test-rust` + `mise run check-public-api` at the end. Do not trust `rtk tsc`/`rtk lint`-style summaries for anything gating; check exit codes.
- Commit after every task. Message style: `feat: …` / `test: …` / `docs: …` / `chore: …`.

## File Structure

```
extensions/middleware/finstack-ai-middleware-tool-policy/
  Cargo.toml          # workspace-member leaf manifest
  README.md           # what it does, config surface, one wiring example
  src/
    lib.rs            # lint preamble, error enum, ToolPolicyMiddleware, Middleware impl
    config.rs         # ToolPolicyConfig + the four rule types + bounds validation
    eval.rs           # pure decision functions (no I/O, no ctx) — the testable core
    tests.rs          # unit tests (config, eval, invoke)
Cargo.toml            # root: add [workspace] member + [workspace.dependencies] entry
docs/site/middleware.md                       # add row to shipping-leaves table
crates/finstack-ai-test/tests/lanes/tool_policy.rs   # integration lane test
fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-middleware-tool-policy.txt  # generated
```

Responsibilities: `config.rs` owns construction-time validation and the digest input; `eval.rs` owns the retain-set math as pure functions over borrowed inputs (unit-testable without building a `MiddlewareContext`); `lib.rs` owns descriptor assembly and the thin `invoke` that parses the stage input, calls `eval`, and maps verdicts to `StageOutcome`.

---

### Task 1: Crate scaffold + workspace wiring + config types

**Files:**
- Create: `extensions/middleware/finstack-ai-middleware-tool-policy/Cargo.toml`
- Create: `extensions/middleware/finstack-ai-middleware-tool-policy/src/lib.rs` (preamble + modules only for now)
- Create: `extensions/middleware/finstack-ai-middleware-tool-policy/src/config.rs`
- Create: `extensions/middleware/finstack-ai-middleware-tool-policy/src/tests.rs`
- Create: `extensions/middleware/finstack-ai-middleware-tool-policy/README.md` (stub; finished in Task 8)
- Modify: root `Cargo.toml` — add `"extensions/middleware/finstack-ai-middleware-tool-policy"` to `[workspace] members` and `finstack-ai-middleware-tool-policy = { path = "extensions/middleware/finstack-ai-middleware-tool-policy", version = "1.0.0" }` to `[workspace.dependencies]` (both lists, matching every existing crate's double entry).

**Interfaces:**
- Produces (used by Tasks 2–7):
  - `pub struct ToolPolicyConfig` with `pub fn try_new() -> …` is NOT provided — instead `ToolPolicyConfig::builder()`-free house style: `try_new` takes nothing and each rule arrives via a `with_*` that validates. Exact surface:
    - `ToolPolicyConfig::try_new() -> Result<Self, ToolPolicyError>` — starts empty; **finalization requires ≥1 rule** (checked in `ToolPolicyMiddleware::try_new`, Task 2, reason `"empty_policy"` — AGENTS.md forbids speculative no-op components).
    - `with_role_allowlist(self, roles: BTreeMap<Arc<str>, BTreeSet<ToolId>>, default_allowed: BTreeSet<ToolId>) -> Result<Self, ToolPolicyError>`
    - `with_write_budget(self, max_write_calls: u32) -> Result<Self, ToolPolicyError>`
    - `with_jailbreak_triggers(self, patterns: Vec<Arc<str>>, action: JailbreakAction) -> Result<Self, ToolPolicyError>`
    - `with_child_depth_gate(self, current_depth: u16, max_depth: u16, restricted: BTreeSet<ToolId>) -> Result<Self, ToolPolicyError>`
    - accessors used by `eval.rs`: `role_allowlist() -> Option<&RoleAllowlist>`, `write_budget() -> Option<&WriteBudget>`, `jailbreak() -> Option<&JailbreakTriggers>`, `child_depth() -> Option<&ChildDepthGate>`, `is_empty() -> bool`
  - `pub enum JailbreakAction { RestrictTo(BTreeSet<ToolId>), Fail }`
  - `pub enum ToolPolicyError { Configuration { reason: &'static str } }` (thiserror, mirroring `VerifyError`, `verify/src/lib.rs:55-63`)

**Bounds (constants in `config.rs`, all tested):** `MAX_ROLES = 128`; `MAX_TOOLS_PER_SET = 1_024` (= `ModelRequestDraft::MAX_TOOLS`); `MAX_PATTERNS = 64`; `MAX_PATTERN_BYTES = 256`; patterns non-empty, no NUL; role names non-empty, ≤ 256 bytes, no NUL; `max_depth ≤ 16` (kernel `RunRelation.depth` cap, TDD §"run relation") and `current_depth ≤ 16`. Violations → `Configuration { reason }` with reasons like `"too_many_roles"`, `"empty_pattern"`, `"pattern_too_long"`, `"depth_exceeds_kernel_cap"`.

- [ ] **Step 1: Write `Cargo.toml`** — copy `finstack-ai-middleware-compaction/Cargo.toml` shape (no `publish = false`; this is a real battery, not a fixture):

```toml
[package]
name = "finstack-ai-middleware-tool-policy"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Tool policy filter middleware for finstack-ai: role allowlists, write budgets, jailbreak triggers, and child-depth gates as FilterTools outcomes"
readme = "README.md"

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros"] }

[lints]
workspace = true
```

- [ ] **Step 2: Wire the root workspace `Cargo.toml`** (both `members` and `[workspace.dependencies]`), create `src/lib.rs` with the verbatim lint preamble from `verify/src/lib.rs:1-22`, crate doc line `//! Tool policy filter middleware: policy-narrows the model-visible tool set.`, and `mod config; pub use config::*;` plus `#[cfg(test)] mod tests;`. `cargo check -p finstack-ai-middleware-tool-policy` must fail only on the not-yet-written types, not on workspace resolution.

- [ ] **Step 3: Write failing config tests in `src/tests.rs`** (representative set — write all of these):

```rust
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::ToolId;

use crate::{JailbreakAction, ToolPolicyConfig, ToolPolicyError};

fn tid(s: &str) -> ToolId {
    ToolId::parse(s).expect("tool id")
}

#[test]
fn role_allowlist_rejects_oversized_role_set() {
    let roles: BTreeMap<Arc<str>, BTreeSet<ToolId>> = (0..129)
        .map(|i| (Arc::from(format!("role-{i}").as_str()), BTreeSet::new()))
        .collect();
    let err = ToolPolicyConfig::try_new()
        .expect("empty config")
        .with_role_allowlist(roles, BTreeSet::new())
        .expect_err("129 roles must be rejected");
    assert!(matches!(
        err,
        ToolPolicyError::Configuration { reason: "too_many_roles" }
    ));
}

#[test]
fn jailbreak_rejects_empty_and_oversized_patterns() {
    let base = || ToolPolicyConfig::try_new().expect("empty config");
    assert!(base()
        .with_jailbreak_triggers(vec![Arc::from("")], JailbreakAction::Fail)
        .is_err());
    assert!(base()
        .with_jailbreak_triggers(vec![Arc::from("x".repeat(257).as_str())], JailbreakAction::Fail)
        .is_err());
    assert!(base()
        .with_jailbreak_triggers(vec![Arc::from("ignore previous instructions")], JailbreakAction::Fail)
        .is_ok());
}

#[test]
fn child_depth_gate_rejects_depth_over_kernel_cap() {
    let err = ToolPolicyConfig::try_new()
        .expect("empty config")
        .with_child_depth_gate(0, 17, BTreeSet::from([tid("finstack.tools.subagent")]))
        .expect_err("max_depth 17 exceeds kernel cap 16");
    assert!(matches!(
        err,
        ToolPolicyError::Configuration { reason: "depth_exceeds_kernel_cap" }
    ));
}

#[test]
fn config_serialization_is_deterministic_for_digest() {
    let build = || {
        ToolPolicyConfig::try_new()
            .expect("empty config")
            .with_write_budget(3)
            .expect("budget")
    };
    let a = serde_json::to_vec(&build()).expect("serialize a");
    let b = serde_json::to_vec(&build()).expect("serialize b");
    assert_eq!(a, b);
}
```

(Note: check `ToolId::parse` requires a `.` namespace — `crates/finstack-ai-kernel/src/primitives/ids.rs:327-339`. Use namespaced ids like `finstack.tools.subagent` in every test.)

- [ ] **Step 4: Run to verify failure** — `cargo nextest run -p finstack-ai-middleware-tool-policy --locked`. Expected: compile errors for the missing types (that counts as the failing state for scaffold tasks).

- [ ] **Step 5: Implement `config.rs`** — `#[derive(Debug, Clone, Serialize)] #[serde(rename_all = "snake_case")]` on every type; private fields; the four rule structs (`RoleAllowlist { roles, default_allowed }`, `WriteBudget { max_write_calls }`, `JailbreakTriggers { patterns, action }`, `ChildDepthGate { current_depth, max_depth, restricted }`) each `pub` with private fields and `pub(crate)` (or pub) accessor methods; `ToolPolicyConfig { role_allowlist: Option<…>, write_budget: Option<…>, jailbreak: Option<…>, child_depth: Option<…> }`; each `with_*` validates its bounds and returns `Self` with the slot filled; setting the same slot twice → `Configuration { reason: "duplicate_rule" }`. `BTreeMap`/`BTreeSet` (not Hash*) everywhere so serialization is canonical and the digest deterministic.

- [ ] **Step 6: Run tests to verify pass** — `cargo nextest run -p finstack-ai-middleware-tool-policy --locked`. Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml extensions/middleware/finstack-ai-middleware-tool-policy
git commit -m "feat: scaffold finstack-ai-middleware-tool-policy with validated policy config"
```

---

### Task 2: Descriptor + `Middleware` impl skeleton

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-tool-policy/src/lib.rs`
- Test: `extensions/middleware/finstack-ai-middleware-tool-policy/src/tests.rs`

**Interfaces:**
- Consumes: `ToolPolicyConfig` (Task 1).
- Produces: `pub struct ToolPolicyMiddleware` with `pub fn try_new(config: ToolPolicyConfig) -> Result<Self, ToolPolicyError>`; `impl Middleware for ToolPolicyMiddleware`. Component identity `finstack.middleware.tool-policy` (charset `[a-z][a-z0-9._-]` with mandatory `.` — hyphen is legal, `ids.rs:324-339`). Stage handling is stubbed to `Continue`; Tasks 4/7 replace the stubs.

- [ ] **Step 1: Write failing tests**

```rust
use finstack_ai_kernel::Stage;
use finstack_ai_runtime::{Middleware, MiddlewareRole, OrderTier};

use crate::ToolPolicyMiddleware;

fn any_config() -> crate::ToolPolicyConfig {
    crate::ToolPolicyConfig::try_new()
        .expect("empty config")
        .with_write_budget(3)
        .expect("budget")
}

#[test]
fn descriptor_declares_both_filter_stages_and_request_shaping_tier() {
    let mw = ToolPolicyMiddleware::try_new(any_config()).expect("leaf");
    let d = mw.descriptor();
    assert!(d.stages.contains(Stage::BeforeModel));
    assert!(d.stages.contains(Stage::BeforeToolBatch));
    assert!(!d.stages.contains(Stage::BeforeFinalize));
    assert!(matches!(d.order.tier, OrderTier::RequestShaping));
    assert!(matches!(d.role, MiddlewareRole::Standard));
    assert_eq!(d.invocation.component.as_str(), "finstack.middleware.tool-policy");
    d.validate().expect("descriptor must validate");
}

#[test]
fn empty_policy_is_rejected() {
    let err = ToolPolicyMiddleware::try_new(
        crate::ToolPolicyConfig::try_new().expect("empty config"),
    )
    .expect_err("a policy with zero rules is a no-op and must be rejected");
    assert!(matches!(
        err,
        crate::ToolPolicyError::Configuration { reason: "empty_policy" }
    ));
}

#[test]
fn distinct_configs_produce_distinct_digests() {
    let a = ToolPolicyMiddleware::try_new(any_config()).expect("leaf a");
    let b = ToolPolicyMiddleware::try_new(
        crate::ToolPolicyConfig::try_new().expect("cfg").with_write_budget(4).expect("budget"),
    )
    .expect("leaf b");
    assert_ne!(
        a.descriptor().invocation.configuration_digest,
        b.descriptor().invocation.configuration_digest
    );
}
```

(If `MiddlewareDescriptor::validate` isn't `pub`, drop that one assertion — the chain validates at resolution; the rest stand.)

- [ ] **Step 2: Run to verify failure** — expected: `ToolPolicyMiddleware` not found.

- [ ] **Step 3: Implement** — mirror `VerifyMiddleware::try_new` (`verify/src/lib.rs:87-116`) exactly, with these substitutions:
  - `ComponentId::parse("finstack.middleware.tool-policy")`, `Version { major: 1, minor: 0, patch: 0 }` const.
  - `configuration_digest: Digest::raw_json(&serde_json::to_vec(&config).map_err(|_| ToolPolicyError::Configuration { reason: "invalid_configuration_encoding" })?)` (the compaction pattern, `compaction/src/lib.rs:145-150`).
  - `recovery: InvocationRecovery::RecomputeSafe` (every verdict is a pure function of config + stage input).
  - `stages: StageMask::from_stages([Stage::BeforeModel, Stage::BeforeToolBatch])`.
  - `order: MiddlewareOrder { tier: OrderTier::RequestShaping, priority: 0, before: Arc::from([]), after: Arc::from([]) }`, `role: MiddlewareRole::Standard`, `metadata: Metadata::empty()`.
  - Reject `config.is_empty()` with `"empty_policy"` before building the descriptor.
  - `invoke`: clone `self.config`, `Box::pin(async move { … })`, match on `StageInput::BeforeModel(_) | StageInput::BeforeToolBatch { .. } => Ok(StageOutcome::Continue)` for now; any other stage → the `MIDDLEWARE_OUTCOME_NOT_ALLOWED` `MiddlewareError` exactly as `verify/src/lib.rs:131-139` does.

- [ ] **Step 4: Run tests to verify pass.**

- [ ] **Step 5: Commit** — `git commit -m "feat: tool-policy middleware descriptor and invoke skeleton"`

---

### Task 3: `eval.rs` — role allowlist + child-depth gate (the ToolId-set rules)

**Files:**
- Create: `extensions/middleware/finstack-ai-middleware-tool-policy/src/eval.rs` (add `mod eval;` to lib.rs; keep it `pub(crate)` — the public API stays config + middleware)
- Test: `src/tests.rs`

**Interfaces:**
- Consumes: `ToolPolicyConfig` accessors (Task 1).
- Produces (consumed by Tasks 4–7):

```rust
/// Outcome of evaluating the policy against one stage's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PolicyVerdict {
    /// No rule narrowed anything: emit `StageOutcome::Continue`.
    Identity,
    /// Retain exactly these tools: emit `StageOutcome::FilterTools`.
    Retain(BTreeSet<ToolId>),
    /// A `JailbreakAction::Fail` trigger fired: emit `StageOutcome::Fail`.
    Fail { reason: &'static str },
}

/// ToolId-set rules only (role allowlist + child-depth gate), applied to a
/// known universe of visible tools. Pure; used by both stages.
pub(crate) fn narrow_universe(
    config: &ToolPolicyConfig,
    universe: &BTreeSet<ToolId>,
    granted_roles: &[Arc<str>],
) -> BTreeSet<ToolId>;
```

**Semantics (write these as doc comments on the functions):**
- Role allowlist: effective allow = `default_allowed ∪ ⋃(roles[r] for r in granted_roles)`. Deny-by-default: a tool not in the effective allow set is dropped. Roles come from `ctx.run.authorization.roles` (`crates/finstack-ai-runtime/src/ports/model/context.rs:23-24`); unknown granted roles are ignored (they contribute nothing). No role rule configured → no narrowing from this rule.
- Child-depth gate: if `current_depth >= max_depth`, drop every tool in `restricted` from the result. Below the threshold → no narrowing.
- `narrow_universe` returns `universe ∩ (rule constraints)`; leaves never "add" tools (narrowing is monotone — the fold intersects anyway, `fold.rs:111-117`).

- [ ] **Step 1: Write failing tests**

```rust
mod eval_tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use finstack_ai_kernel::ToolId;

    use crate::eval::narrow_universe;
    use crate::ToolPolicyConfig;

    fn tid(s: &str) -> ToolId {
        ToolId::parse(s).expect("tool id")
    }

    fn universe() -> BTreeSet<ToolId> {
        BTreeSet::from([tid("t.read"), tid("t.write"), tid("t.spawn")])
    }

    #[test]
    fn role_allowlist_is_deny_by_default_union_of_granted_roles() {
        let cfg = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_role_allowlist(
                BTreeMap::from([
                    (Arc::<str>::from("reader"), BTreeSet::from([tid("t.read")])),
                    (Arc::<str>::from("writer"), BTreeSet::from([tid("t.write")])),
                ]),
                BTreeSet::new(),
            )
            .expect("roles");
        let granted = [Arc::<str>::from("reader")];
        assert_eq!(
            narrow_universe(&cfg, &universe(), &granted),
            BTreeSet::from([tid("t.read")])
        );
        let both = [Arc::<str>::from("reader"), Arc::<str>::from("writer")];
        assert_eq!(
            narrow_universe(&cfg, &universe(), &both),
            BTreeSet::from([tid("t.read"), tid("t.write")])
        );
        // no granted roles, empty default → everything filtered
        assert!(narrow_universe(&cfg, &universe(), &[]).is_empty());
    }

    #[test]
    fn child_depth_gate_hides_restricted_tools_at_threshold() {
        let cfg = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_child_depth_gate(2, 2, BTreeSet::from([tid("t.spawn")]))
            .expect("gate");
        assert_eq!(
            narrow_universe(&cfg, &universe(), &[]),
            BTreeSet::from([tid("t.read"), tid("t.write")])
        );
        let below = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_child_depth_gate(1, 2, BTreeSet::from([tid("t.spawn")]))
            .expect("gate");
        assert_eq!(narrow_universe(&below, &universe(), &[]), universe());
    }

    #[test]
    fn rules_compose_by_intersection() {
        let cfg = ToolPolicyConfig::try_new()
            .expect("cfg")
            .with_role_allowlist(
                BTreeMap::from([(
                    Arc::<str>::from("agent"),
                    BTreeSet::from([tid("t.read"), tid("t.spawn")]),
                )]),
                BTreeSet::new(),
            )
            .expect("roles")
            .with_child_depth_gate(3, 2, BTreeSet::from([tid("t.spawn")]))
            .expect("gate");
        let granted = [Arc::<str>::from("agent")];
        assert_eq!(
            narrow_universe(&cfg, &universe(), &granted),
            BTreeSet::from([tid("t.read")])
        );
    }
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement `narrow_universe`** — start from `universe.clone()`; if role rule present, intersect with the effective allow set; if depth rule fires, subtract `restricted`. ≤ 30 lines.
- [ ] **Step 4: Run tests to verify pass.**
- [ ] **Step 5: Commit** — `git commit -m "feat: tool-policy eval core for role allowlists and child-depth gates"`

---

### Task 4: `before_model` — full evaluation (adds write budget + jailbreak scan) and wiring into `invoke`

**Files:**
- Modify: `src/eval.rs`, `src/lib.rs`
- Test: `src/tests.rs`

**Interfaces:**
- Consumes: `BeforeModelInput { request: ModelRequestDraft, .. }` (`crates/finstack-ai-runtime/src/ports/middleware/types.rs:219-230`); `ModelRequestDraft { messages: Arc<[Message]>, tools: Arc<[ToolSpec]>, .. }` (`ports/model/request.rs:184-197`); `Message::role()/content()` (`kernel/src/conversation/message.rs:454-461`); `ContentBlock::{Text, ToolCall}` (`kernel/src/content/content_block.rs`); `TextBlock::text()` (`content/text.rs:95`); `ToolSpec { id, model_name, side_effect, .. }`; `SideEffectClass::{ReadOnly, IdempotentWrite, NonIdempotentWrite}` (`ports/model/request.rs:94-101`).
- Produces:

```rust
pub(crate) fn evaluate_before_model(
    config: &ToolPolicyConfig,
    input: &BeforeModelInput,
    granted_roles: &[Arc<str>],
) -> PolicyVerdict;
```

**Semantics:**
- Universe = `input.request.tools` ids. Start with `narrow_universe` (Task 3).
- **Write budget:** count `ContentBlock::ToolCall` blocks in `MessageRole::Assistant` messages of `input.request.messages` whose `tool_name()` matches (by `ToolSpec.model_name`) a tool in `input.request.tools` with `side_effect != SideEffectClass::ReadOnly`. If `count >= max_write_calls`, subtract every non-`ReadOnly` tool from the retain set. Pure function of the draft — deterministic on replay. (Known limitation, record it in the module docs: calls whose tool has since been filtered out of the draft don't count; the budget is a visibility brake, not an audited ledger. A journal-backed budget would need a different recovery class — YAGNI now.)
- **Jailbreak triggers:** case-insensitive substring scan of every `ContentBlock::Text` block in `MessageRole::User` and `MessageRole::Tool` messages (assistant/system text is trusted). Case-insensitivity via `str::to_lowercase` on both sides — patterns are ASCII-ish config strings; document that non-ASCII patterns match by lowercased form. On first match: `JailbreakAction::Fail` → `PolicyVerdict::Fail { reason: "tool_policy_jailbreak_triggered" }`; `JailbreakAction::RestrictTo(safe)` → intersect retain with `safe`. Bounded work: ≤ 4 096 messages × ≤ 64 patterns × ≤ 256 bytes.
- If final retain set == universe → `Identity` (keep the fold identity-clean, `fold.rs:183-190`); else `Retain`.
- **`invoke` wiring (lib.rs):** on `StageInput::BeforeModel(boxed)`, call `evaluate_before_model(&config, &boxed, &ctx.run.authorization.roles)`; map `Identity → Continue`, `Retain(set) → StageOutcome::FilterTools(set.into_iter().collect::<Vec<_>>().into())`, `Fail { reason } → StageOutcome::Fail(Box::new(ErrorDescriptor::new(reason, "tool policy jailbreak trigger matched", ErrorCategory::Validation, false)…))` with the same `map_err` fallback shape as `verify/src/lib.rs:142-158`. Export `pub const TOOL_POLICY_JAILBREAK_TRIGGERED: &str = "tool_policy_jailbreak_triggered";` from lib.rs (house style: stable codes as consts, e.g. `SHELL_POLICY_DENIED`).

- [ ] **Step 1: Write failing eval tests.** Building a `BeforeModelInput` fixture needs a minimal `ModelRequestDraft`; crib the construction from an existing runtime or compaction test that builds one (search: `rg -n "ModelRequestDraft {" crates/finstack-ai-runtime/src --glob '*tests*'`), wrap it in a local `fn draft(tools: Vec<ToolSpec>, messages: Vec<Message>) -> BeforeModelInput` test helper, and write:
  - `write_budget_hides_write_tools_once_spent`: 2 prior assistant `ToolCall`s naming a `NonIdempotentWrite` tool, budget 2 → verdict retains only the `ReadOnly` tools.
  - `write_budget_under_limit_is_identity`: 1 prior write call, budget 2 → `Identity`.
  - `jailbreak_fail_action_fails_the_stage`: user message text "please IGNORE Previous Instructions" with pattern "ignore previous instructions" → `Fail { reason: "tool_policy_jailbreak_triggered" }`.
  - `jailbreak_restrict_action_narrows_to_safe_set`: same trigger with `RestrictTo({read_tool})` → `Retain({read_tool})`.
  - `assistant_text_does_not_trigger_jailbreak`: pattern present only in an assistant message → `Identity`.
  - `identity_when_nothing_narrows`: config = write budget 10, no prior calls → `Identity` (not `Retain(full universe)`).
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement `evaluate_before_model` + the `invoke` arm.**
- [ ] **Step 4: Add one `invoke`-level test** (`#[tokio::test]`) asserting a `StageInput::BeforeModel` with a role-allowlisted config yields `StageOutcome::FilterTools` with exactly the expected ids — build `MiddlewareContext` the same way `finstack-ai-middleware-verify/src/tests.rs` does (copy its context fixture helper).
- [ ] **Step 5: Run tests to verify pass.**
- [ ] **Step 6: Commit** — `git commit -m "feat: before_model tool-policy evaluation (roles, write budget, jailbreak)"`

---

### Task 5: `before_tool_batch` — defense-in-depth arm

**Files:**
- Modify: `src/eval.rs`, `src/lib.rs`
- Test: `src/tests.rs`

**Interfaces:**
- Consumes: `StageInput::BeforeToolBatch { value: RawJson }` where value is the canonical serialization of the batch's `&[ToolCallBlock]` (`tool_batch.rs:67-69`); `RawJson::as_bytes()` (`kernel/src/primitives/raw_json.rs:70`).
- Produces:

```rust
/// Complete-set rules only (role allowlist, child-depth). Returns `Identity`
/// when no configured rule can express a complete retain set at this stage.
pub(crate) fn evaluate_before_tool_batch(
    config: &ToolPolicyConfig,
    granted_roles: &[Arc<str>],
) -> PolicyVerdict;
```

**Semantics (document in the module header — this is the subtle part):**
- `FilterTools` is retain-semantics, and at this stage the leaf sees no catalog, so it may only emit a set it knows to be complete. The role allowlist's effective allow set (`default_allowed ∪ granted roles' sets`) *is* complete by construction. If a child-depth gate is also configured and firing, subtract `restricted` from it. If **no role rule** is configured, return `Identity` — a depth-only or budget-only policy cannot enumerate the universe here, and `before_model` already did the hiding; this stage only backstops role policy against a provider emitting calls to tools the model was never shown. The runtime turns the retain set into per-call `Deny` → `SyntheticClosure` (`stage.rs:293-306`) — filtered calls are answered, never dropped.
- The batch payload itself is not needed for the verdict (the retain set is config-derived), but `invoke` must still verify the stage input parses: `serde_json::from_slice::<Vec<finstack_ai_kernel::ToolCallBlock>>(value.as_bytes())`, failing with a `MiddlewareError` (code `MIDDLEWARE_OUTCOME_NOT_ALLOWED`, message `"tool batch payload malformed"`) on error — a leaf must not silently `Continue` past an input it cannot read.

- [ ] **Step 1: Write failing tests:**
  - `batch_stage_with_role_policy_emits_complete_allow_set`: roles `{agent: {t.read, t.spawn}}`, granted `[agent]`, depth gate firing on `t.spawn` → `Retain({t.read})`.
  - `batch_stage_without_role_policy_is_identity`: write-budget-only config → `Identity`.
  - `invoke`-level `#[tokio::test]`: `StageInput::BeforeToolBatch` with a canonical `Vec<ToolCallBlock>` payload (build blocks via `ToolCallBlock` constructor, serialize with `serde_json::to_vec`, wrap in `RawJson::parse`) → `StageOutcome::FilterTools` for a role config, and a malformed payload (`RawJson::parse(b"{}")`) → `Err(MiddlewareError)`.
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests to verify pass.**
- [ ] **Step 5: Commit** — `git commit -m "feat: before_tool_batch defense-in-depth arm for tool-policy middleware"`

---

### Task 6: Integration lane test

**Files:**
- Create: `crates/finstack-ai-test/tests/lanes/tool_policy.rs`
- Modify: whatever lane-registration module lists lane tests (see how `crates/finstack-ai-test/tests/lanes/document_ingest.rs` is included — mirror its `mod` registration and harness usage exactly)
- Modify: `crates/finstack-ai-test/Cargo.toml` — add `finstack-ai-middleware-tool-policy = { workspace = true }` to dev-dependencies

**Interfaces:**
- Consumes: `AgentBuilder::middleware(component: ComponentRef, middleware: Arc<dyn Middleware>)` (`crates/finstack-ai/src/agent/builder.rs:207`; wiring example at `examples/rust-minimal/src/bin/coding.rs:121-123`); the scripted-provider harness used by existing lane tests.

- [ ] **Step 1: Read `document_ingest.rs` end to end** and identify: how a lane test registers toolsets + middleware, how the scripted model provider is told to emit tool calls, and what the test can assert about (a) the tools array in the captured model request and (b) tool-call settlement outcomes.
- [ ] **Step 2: Write the lane test (failing first if the harness allows building it before the dep is wired):**
  - Register two tools (one `ReadOnly`, one write-class — reuse whatever fixture toolset the harness provides) plus the tool-policy leaf configured with a role allowlist granting only the read tool (grant the role via the harness's `AuthorizationContext` fixture).
  - Assert the captured model request's `tools` contains only the read tool (proves the `before_model` fold landed via `apply.rs:161-169`).
  - Script the provider to call the write tool anyway; assert the run does not execute it and the call settles as a denial/synthetic closure rather than a dropped call (proves `stage.rs:293-306` + `assert_plan_coverage`).
- [ ] **Step 3: Run** — `cargo nextest run -p finstack-ai-test --locked -E 'test(tool_policy)'`. Expected: PASS.
- [ ] **Step 4: Commit** — `git commit -m "test: tool-policy lane test covering model-request narrowing and batch denial"`

---

### Task 7: Docs + README

**Files:**
- Modify: `docs/site/middleware.md` — add `finstack-ai-middleware-tool-policy` to the shipping-leaves table (today it lists exactly `verify` and `compaction`), one row: stages `before_model`, `before_tool_batch`; outcome `FilterTools` / `Fail`; one-line purpose.
- Create/finish: `extensions/middleware/finstack-ai-middleware-tool-policy/README.md`

- [ ] **Step 1: README** — sections: What it does (one paragraph, retain-semantics + intersection with other leaves per the fold contract); The four rules (role allowlist, write budget, jailbreak triggers, child-depth gate — each with its exact semantics and its stage coverage, including the `before_tool_batch` complete-set caveat and the construction-time-depth caveat vs `ChildRunPolicy`); Configuration example (compilable snippet: build `ToolPolicyConfig`, `ToolPolicyMiddleware::try_new`, register via `AgentBuilder::middleware`); Stable error codes (`tool_policy_jailbreak_triggered`, `tool_policy_configuration_invalid` reasons list).
- [ ] **Step 2: docs/site/middleware.md row.**
- [ ] **Step 3: Commit** — `git commit -m "docs: document tool-policy middleware leaf"`

---

### Task 8: Public-API baseline + full verification

**Files:**
- Create (generated): `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-middleware-tool-policy.txt`

- [ ] **Step 1: Regenerate baselines**

```bash
uv run --no-project python scripts/compat/public_items.py --write
```

Inspect the diff: only the new crate's baseline file should appear (plus nothing else — if other crates' baselines changed, something leaked `pub` that shouldn't be).

- [ ] **Step 2: Full verification, checking exit codes explicitly**

```bash
mise run check-rust
```

```bash
mise run test-rust
```

```bash
mise run check-public-api
```

Expected: all exit 0. Fix anything that doesn't; clippy runs with `-D warnings` and pedantic on.

- [ ] **Step 3: Commit**

```bash
git add fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-middleware-tool-policy.txt
git commit -m "chore: add public-api baseline for finstack-ai-middleware-tool-policy"
```

---

## Self-Review (performed at planning time)

- **Spec coverage:** before_model/before_tool_batch leaf returning FilterTools → Tasks 2/4/5. Role allowlists → Tasks 1/3. Write budgets → Task 4. Jailbreak triggers → Tasks 1/4. Child-depth gates → Tasks 1/3. "Configs on this crate, not four crates" → single `ToolPolicyConfig` with four optional slots. "Path and command allowlists stay in the toolsets" → this crate takes only `ToolId`-level policy; no path/command config anywhere in it. Fold contract already landable → zero runtime changes planned.
- **Known open risks for the executor:** (1) exact construction of `ModelRequestDraft`/`Message` test fixtures — crib from existing runtime tests, budget extra time in Task 4 Step 1; (2) `finstack-ai-test` lane-harness capabilities for asserting synthetic closures — Task 6 Step 1 exists precisely to resolve that before writing assertions; (3) whether `MiddlewareDescriptor::validate` is public — assertion is droppable.
- **Type consistency:** `PolicyVerdict`, `narrow_universe`, `evaluate_before_model`, `evaluate_before_tool_batch`, `ToolPolicyConfig` accessor names are used identically across Tasks 3–5.
