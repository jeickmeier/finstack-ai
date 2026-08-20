# BeforeToolBatch Stage-Contract Extension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `before_tool_batch` middleware the resolved tool universe and give every port call the run's live relation depth, so the tool-policy leaf's batch backstop enforces role allowlists AND child-depth gates against the real universe, and the depth gate reads live depth instead of a construction-time snapshot.

**Architecture:** Two runtime contract changes, then the middleware catches up. (a) `StageInput::BeforeToolBatch { value: RawJson }` becomes `BeforeToolBatch(Box<BeforeToolBatchInput>)` carrying the batch's calls plus an `Arc<[ToolSpec]>` universe projected from the `ResolvedToolCatalog` — mirroring the existing `BeforeModel(Box<BeforeModelInput>)` pattern. The catalog is already a parameter of the sole production caller's enclosing function (`prepare_tool_batch_if_ready`, `crates/finstack-ai-runtime/src/exec/settlement/stage.rs:52-137`), so this is a one-argument thread-through. (b) `RunCallContext` gains `relation_depth: u16`, sourced from `state.accepted.relation().depth()` — the stage-boundary builder's seed (`stage_dispatch_seed`, `crates/finstack-ai-runtime/src/exec/coordinator/dispatch.rs:90-100`) already dereferences `state.accepted` for the deadline, so the data source is one line. Then the tool-policy leaf: `ChildDepthGate` drops its frozen `current_depth` and reads `ctx.run.relation_depth`; `evaluate_before_tool_batch` narrows the real universe via the shared eval path.

**Explicitly out of scope (decided during design, record in docs):** re-checking write budgets and jailbreak triggers at `before_tool_batch`. Both need message history, which the batch input does not and should not carry (verified: the post-`before_model` narrowed request is not retained in `KernelState` after the model round-trip — only usage counters survive, `exec/settlement/stage.rs:400-410` — so the narrowed universe cannot be sourced either without new state plumbing). The depth-only enforcement hole closes; budget/jailbreak stay `before_model`-only, documented with this rationale.

**Tech Stack:** Rust workspace; changes span `crates/finstack-ai-runtime` (ports + exec), `extensions/middleware/finstack-ai-middleware-tool-policy`, test/fixture crates, docs, and public-API baselines.

**Spec:** The dispatched task description (batch backstop gaps confirmed in code review) plus the recon findings embedded per-task below. Fold contract authority: `crates/finstack-ai-runtime/src/exec/middleware_driver/fold.rs` (FilterTools intersects, monotone narrowing) — unchanged by this plan.

## Global Constraints

- House lint rules as before: workspace clippy pedantic `-D warnings`, `cargo fmt --all --check`, crate lint preambles unchanged.
- `StageInput` is process-internal plus two binding bridges: Python middleware callbacks receive it as a JSON dict (`bindings/finstack-ai-python/src/callbacks/engine.rs:175-196`) and JS/WASM as a JSON string (`bindings/finstack-ai-wasm/src/host_middleware.rs:156-179`). The new batch payload shape (`{"stage":"before_tool_batch","calls":[…],"tools":[…]}`) MUST be documented in `docs/site/middleware.md`. No golden wire fixtures exist for `StageInput` (verified) — no fixture updates needed for the shape itself.
- `RunCallContext` is not `#[non_exhaustive]`: adding a field breaks all ~70 struct-literal sites (15 production, 1 doctest, 4 shipped fixtures, ~50 test/bench). All must be updated in the same task. Production sites take the real depth when a `KernelState`/accepted record or seed is in scope; otherwise `relation_depth: 0` with a `// no accepted run in scope` comment. Test/fixture sites take `relation_depth: 0` unless the test exercises depth.
- WIT surface: NOT affected. `sanitize_call_context` (`plugins/finstack-ai-wit/src/mapping.rs:23-42`) projects field-by-field; do NOT add depth to the WIT `call-context` record (that would be a WIT version bump — out of scope).
- Public API baselines (regenerate at the end, expect exactly these to change): `finstack-ai-runtime.txt`, `finstack-ai-runtime+native-tokio.txt`, `finstack-ai-runtime+wasm-host.txt` (StageInput variant, BeforeToolBatchInput, RunCallContext), `finstack-ai-middleware-tool-policy.txt` (config/eval API changes, removed const). Hand-maintained breaking-change fixtures `fixtures/compatibility/breaking/public-rust-api/{valid--v0.1.0-public-items.txt,invalid--renamed-item.txt}` each need `finstack-ai-runtime::BeforeToolBatchInput` inserted in sorted position (convention: docs/superpowers/plans/2026-08-15-middleware-chain-driver.md:511-513); verify with `cargo test -p finstack-ai-test --test breaking_change`.
- Verification commands: `cargo nextest run -p <crate> --locked` per task; full gate at the end: `mise run check-rust`, `mise run test-rust`, `mise run check-public-api`, plus `uv run --no-project python scripts/compat/public_items.py --write` for regeneration.
- Commit per task. This continues branch `worktree-tool-policy-middleware`.

## File Structure

```
crates/finstack-ai-runtime/src/ports/middleware/types.rs     # BeforeToolBatchInput + variant reshape + stage()
crates/finstack-ai-runtime/src/ports/middleware/mod.rs       # export
crates/finstack-ai-runtime/src/lib.rs                        # export
crates/finstack-ai-runtime/src/exec/stage_settlement/tool_batch.rs  # build typed input (catalog param)
crates/finstack-ai-runtime/src/exec/settlement/stage.rs      # pass catalog at call site
crates/finstack-ai-runtime/src/ports/model/context.rs        # RunCallContext.relation_depth
crates/finstack-ai-runtime/src/exec/coordinator/dispatch.rs  # StageDispatchSeed.relation_depth + builder
crates/finstack-ai-runtime/src/exec/stage_settlement/driver.rs  # seed → RunCallContext
[+ ~68 other RunCallContext literal sites, mechanical]
extensions/middleware/finstack-ai-middleware-tool-policy/src/{config,eval,lib,tests}.rs
extensions/middleware/finstack-ai-middleware-tool-policy/README.md
docs/site/middleware.md
fixtures/compatibility/public-rust-api/cargo-public-api/*.txt          # regenerated
fixtures/compatibility/breaking/public-rust-api/*.txt                  # +1 sorted line each
```

---

### Task 1: Typed `BeforeToolBatchInput` in the runtime

**Files:**
- Modify: `crates/finstack-ai-runtime/src/ports/middleware/types.rs` (variant at :254, `stage()` at :279, new struct near `BeforeModelInput` :217)
- Modify: `crates/finstack-ai-runtime/src/ports/middleware/mod.rs:41-46`, `crates/finstack-ai-runtime/src/lib.rs:164-175` (exports)
- Modify: `crates/finstack-ai-runtime/src/exec/stage_settlement/tool_batch.rs:53-72` (signature + construction), `crates/finstack-ai-runtime/src/exec/settlement/stage.rs:114` (call site), `crates/finstack-ai-runtime/src/exec/stage_settlement/tests/terminal_fold.rs:158` (test call site — needs a catalog; use the same fixture catalog the surrounding tests use, or an empty catalog if one is constructible)
- Test: existing runtime tests referencing the variant (`ports/middleware/tests.rs:375,522,555`, `exec/middleware_driver/tests.rs:264`, `exec/settlement/tests/*`) — mechanical updates; plus one new unit test.

**Interfaces:**
- Produces (consumed by Task 3):

```rust
/// Immutable input for one `before_tool_batch` invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeforeToolBatchInput {
    /// Canonicalized source calls of the batch, in source order.
    pub calls: Arc<[ToolCallBlock]>,
    /// The resolved tool universe visible to this run's catalog, in catalog
    /// order (`ResolvedToolCatalog::tools()`), including side-effect classes.
    pub tools: Arc<[ToolSpec]>,
}
```

and the variant `BeforeToolBatch(Box<BeforeToolBatchInput>)` (internally-tagged serde: serializes as `{"stage":"before_tool_batch","calls":[…],"tools":[…]}` — same mechanism as `BeforeModel`).

- `run_tool_batch_chain` gains the catalog:

```rust
pub(crate) async fn run_tool_batch_chain(
    coordinator: &CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    driver: Option<&StageDriver>,
    cursor: StageCursor,
    calls: &[ToolCallBlock],
) -> Result<ToolBatchPolicy, RunHandleError>
```

constructing:

```rust
let input = StageInput::BeforeToolBatch(Box::new(BeforeToolBatchInput {
    calls: calls.to_vec().into(),
    tools: catalog.tools().map(|tool| tool.spec.clone()).collect::<Vec<_>>().into(),
}));
```

(`canonical(&calls)` and its error path disappear from this function; keep the `debug_assert_eq!` on the cursor stage.)

- [ ] **Step 1: Write the failing test** — in the runtime's middleware tests (next to the existing `StageInput` structural tests), a round-trip test asserting the serialized shape:

```rust
#[test]
fn before_tool_batch_input_serializes_with_stage_tag_calls_and_tools() {
    let input = StageInput::BeforeToolBatch(Box::new(BeforeToolBatchInput {
        calls: Arc::from([]),
        tools: Arc::from([]),
    }));
    let json = serde_json::to_value(&input).expect("serialize");
    assert_eq!(json["stage"], "before_tool_batch");
    assert!(json["calls"].is_array());
    assert!(json["tools"].is_array());
    assert_eq!(input.stage(), Stage::BeforeToolBatch);
}
```

- [ ] **Step 2: Run to verify failure** (`cargo nextest run -p finstack-ai-runtime --locked -E 'test(before_tool_batch_input)'` — compile failure counts).
- [ ] **Step 3: Implement** the struct, variant reshape, `stage()` arm, exports, `run_tool_batch_chain` change, and both call sites. Fix every compile error the reshape surfaces (the grep-verified matcher list is small: `tool_batch.rs:67`, `types.rs:254/:279`, plus the tool-policy crate — leave the tool-policy crate BROKEN in this task only if unavoidable; otherwise insert a minimal temporary adaptation. Preferred: make the tool-policy `lib.rs` batch arm compile by binding the typed input and ignoring the new fields (`StageInput::BeforeToolBatch(_input) => …evaluate as today…`), deleting the manual deserialization; Task 3 does the real consumption. Keep its tests compiling with minimal edits.)
- [ ] **Step 4: Run** `cargo nextest run -p finstack-ai-runtime -p finstack-ai-middleware-tool-policy --locked` → all pass; workspace clippy + fmt clean.
- [ ] **Step 5: Commit** — `feat: typed BeforeToolBatchInput carrying calls and tool universe`

---

### Task 2: `relation_depth` on `RunCallContext`

**Files:**
- Modify: `crates/finstack-ai-runtime/src/ports/model/context.rs:135-164` (field + doc), `crates/finstack-ai-runtime/src/exec/coordinator/dispatch.rs` (`StageDispatchSeed` :226-234, `stage_dispatch_seed` :90-100), `crates/finstack-ai-runtime/src/exec/stage_settlement/driver.rs:348-383` (seed → context)
- Modify (mechanical, all ~70 literal sites; the 15 production sites listed in the recon: `driver/native/tool.rs:242`, `driver/native/model.rs:167,743`, `exec/compaction_driver/mod.rs:199`, `exec/settlement/nested_sample.rs:136,190`, `exec/settlement/poll.rs:217`, `exec/settlement/model.rs:59`, `exec/settlement/tool.rs:140,533`, `exec/host_task/dispatcher.rs:108,158,221`, `exec/stage_settlement/driver.rs:363`, `exec/context_driver/collect.rs:66`; doctest `ports/tool/authority.rs:33`; shipped fixtures `bindings/finstack-ai-wasm/src/fixture.rs:33`, `bindings/finstack-ai-python/src/callback_fixture.rs:37`, `fixtures/ci/{model,toolset}-port-leaf/src/lib.rs`; plus all `#[cfg(test)]` sites the compiler surfaces)
- Test: one new runtime test asserting the stage-boundary context carries the accepted run's depth.

**Interfaces:**
- Produces (consumed by Task 3): `pub relation_depth: u16` on `RunCallContext` — "Relation depth of the owning run (0 for a root run), from the accepted run record. Capped by `finstack_ai_kernel::MAX_RUN_RELATION_DEPTH`."
- Data source: in `stage_dispatch_seed` add `relation_depth: state.accepted.as_ref()?.relation().depth()` to the seed; `invoke_stage_chain` copies it into the context. For the other production sites: use the accepted record where already in scope (several settlement sites read `coordinator.state().accepted` for deadlines — mirror that), else `relation_depth: 0` with the comment.

- [ ] **Step 1: Write the failing test** — extend the existing stage-driver test fixture (whichever test in `exec/stage_settlement/tests/` or `exec/middleware_driver/tests.rs` builds a chain with an accepted run and inspects the context; if none inspects the context, add a capturing middleware fixture that records `ctx.run.relation_depth` and assert it equals the fixture's accepted relation depth).
- [ ] **Step 2: Run to verify failure** (compile failure across the workspace is the expected first signal).
- [ ] **Step 3: Implement** field + seed + all literal sites. Let the compiler enumerate; fix every site per the sourcing rule above.
- [ ] **Step 4: Run** `cargo nextest run --workspace --locked --exclude finstack-ai-plugin-host --exclude finstack-ai-wasm` (the reshape touches fixtures across many crates; a workspace pass here saves surprises at Task 4) plus workspace clippy + fmt.
- [ ] **Step 5: Commit** — `feat: expose run relation depth on RunCallContext`

---

### Task 3: Tool-policy middleware consumes universe + live depth

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-tool-policy/src/config.rs`, `src/eval.rs`, `src/lib.rs`, `src/tests.rs`, `README.md`
- Modify: `docs/site/middleware.md`
- Test: `crates/finstack-ai-test/tests/lanes/tool_policy.rs` (assertions unchanged in spirit; re-run; extend if cheap — see Step 4)

**Interfaces (final shapes):**
- `config.rs`: `with_child_depth_gate(self, max_depth: u16, restricted: BTreeSet<ToolId>) -> Result<Self, ToolPolicyError>` — `current_depth` parameter and the `ChildDepthGate::current_depth()` accessor are REMOVED. Validation keeps `max_depth <= MAX_KERNEL_DEPTH` (reason `"depth_exceeds_kernel_cap"`) and non-empty/bounded `restricted`. Doc: "The gate compares the run's live relation depth (`RunCallContext::relation_depth`) against `max_depth` at every stage invocation; when `relation_depth >= max_depth`, the `restricted` tools are hidden." Delete the staleness warning added earlier (it no longer applies) and the README's equivalent.
- `eval.rs`:
  - `compute_effective_allow(config: &ToolPolicyConfig, granted_roles: &[Arc<str>], relation_depth: u16) -> Option<BTreeSet<ToolId>>` (gate subtraction now keyed on `relation_depth >= gate.max_depth()`)
  - `narrow_universe(config: &ToolPolicyConfig, universe: &BTreeSet<ToolId>, granted_roles: &[Arc<str>], relation_depth: u16) -> BTreeSet<ToolId>`
  - `evaluate_before_model(config: &ToolPolicyConfig, input: &BeforeModelInput, granted_roles: &[Arc<str>], relation_depth: u16) -> PolicyVerdict`
  - `evaluate_before_tool_batch(config: &ToolPolicyConfig, input: &BeforeToolBatchInput, granted_roles: &[Arc<str>], relation_depth: u16) -> PolicyVerdict` — builds `universe: BTreeSet<ToolId>` from `input.tools` ids, runs `narrow_universe`, returns `Identity` when nothing narrows (retain == universe) else `Retain`. The role-allowlist-required precondition DISAPPEARS: depth-only configs now narrow the real universe. Rewrite the function's doc comment: the complete-set constraint is now satisfied by the carried universe; write budget and jailbreak remain `before_model`-only because they need message history the batch input deliberately does not carry (cite the plan's out-of-scope note).
- `lib.rs`: batch arm binds `StageInput::BeforeToolBatch(input)`, calls `evaluate_before_tool_batch(&config, &input, &ctx.run.authorization.roles, ctx.run.relation_depth)`; the before_model arm passes `ctx.run.relation_depth` too. DELETE `TOOL_POLICY_BATCH_PAYLOAD_MALFORMED` (the runtime now guarantees a typed input; a pub const removal → baseline + README error-code section update). The wrong-stage arm is unchanged.

- [ ] **Step 1: Write/adjust failing tests** (in `src/tests.rs`):
  - Rewrite `child_depth_gate_hides_restricted_tools_at_threshold`: gate `with_child_depth_gate(2, {t.spawn})`, `narrow_universe(&cfg, &universe, &[], 2)` → spawn hidden; `…, &[], 1)` → universe unchanged.
  - `batch_stage_depth_only_config_now_narrows_real_universe`: config with ONLY a depth gate (`max_depth 2`, restricted `{t.spawn}`); `BeforeToolBatchInput { calls: [], tools: [spec(t.read), spec(t.spawn)] }`; `evaluate_before_tool_batch(&cfg, &input, &[], 2)` → `Retain({t.read})`; at depth 1 → `Identity`.
  - `batch_stage_role_policy_narrows_universe_not_config_set`: role allowlist `default_allowed = {t.read, t.ghost}` where `t.ghost` is NOT in the input universe; universe `{t.read, t.write}` → `Retain({t.read})` (proves intersection with the real universe, not the raw config set).
  - Update the two existing batch-stage tests and the invoke-level batch test to the typed input (build `BeforeToolBatchInput` directly; the malformed-payload assertion is deleted with rationale in the commit message).
  - Update every `evaluate_before_model`/`narrow_universe` call for the new depth parameter (pass 0 where depth is irrelevant).
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** config/eval/lib changes.
- [ ] **Step 4: Docs** — README: rewrite the stage table and the child-depth section (live depth; construction-time caveat GONE; batch stage now backstops role + depth against the true universe; budget/jailbreak scoping rationale). `docs/site/middleware.md`: document the typed `before_tool_batch` payload shape for Python/JS middleware authors (one short subsection: stage tag + `calls` + `tools`, note this changed from the bare calls array). Lane test: re-run; if the harness exposes relation depth cheaply, add a depth-gate lane assertion — otherwise leave the lane test as-is (unit tests cover depth).
- [ ] **Step 5: Run** `cargo nextest run -p finstack-ai-middleware-tool-policy -p finstack-ai-test --locked`, workspace clippy, fmt.
- [ ] **Step 6: Commit** — `feat: batch-stage universe narrowing and live-depth gate in tool-policy middleware`

---

### Task 4: Baselines, breaking fixtures, full verification

**Files:**
- Regenerate: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-runtime{,+native-tokio,+wasm-host}.txt`, `finstack-ai-middleware-tool-policy.txt` (inspect the diff: ONLY these four may change; other crates changing = pub leak → stop)
- Modify: `fixtures/compatibility/breaking/public-rust-api/valid--v0.1.0-public-items.txt` and `invalid--renamed-item.txt` — insert `finstack-ai-runtime::BeforeToolBatchInput` in sorted position (near `finstack-ai-runtime::BeforeModelInput`)

- [ ] **Step 1:** `uv run --no-project python scripts/compat/public_items.py --write`; inspect `git diff --stat`.
- [ ] **Step 2:** Edit the two breaking fixtures; run `cargo test -p finstack-ai-test --test breaking_change --locked` → pass.
- [ ] **Step 3:** Full gate, exit codes checked: `mise run check-rust` → `mise run test-rust` → `mise run check-public-api`. Fix only trivial breakage from this branch.
- [ ] **Step 4: Commit** — `chore: regenerate baselines for batch-stage contract extension`

---

## Self-Review

- Spec coverage: universe in batch input → Task 1; relation depth → Task 2; batch backstop re-check (role + depth; budget/jailbreak evaluated and excluded with recorded rationale) → Task 3 + header; fold contract respected (still emits complete retain sets, intersection unchanged) → Task 3 semantics; docs/site/middleware.md + README scoping updates → Task 3 Step 4; baselines → Task 4.
- Known risks for the executor: (1) Task 2's compiler-driven sweep is wide — the recon's site list is the map, but trust the compiler's list over it; (2) the tool-policy crate must keep compiling through Task 1 (temporary adaptation is specified); (3) breaking-fixture edit convention — follow the 2026-08-15 plan's documented pattern.
- Type consistency: `BeforeToolBatchInput` fields (`calls`, `tools`), `relation_depth: u16`, and the four eval signatures are stated once each and used identically across tasks.
