# Workflow: the phased Audit → Plan → Refactor → Verify loop

This is the procedural spine of the skill. Read it at the start of every session. Each phase has a deliverable, a checkpoint, and explicit "done" criteria.

---

## Phase 1 — Audit (read-only)

**Inputs:** a scope (usually a crate or subsystem, e.g., `statements/checks/`, `margin/`, `core/market_data/surfaces/`).

**Actions:**
1. Read every file in the scope. Also read the binding counterparts under `finstack-quant-py/src/bindings/` and `finstack-quant-wasm/src/api/` for the same crate domain.
2. Apply every category from `slop-patterns.md`. For each finding, collect: file, line, pattern category, brief explanation, proposed fix, impact (H/M/L), risk (H/M/L).
3. Apply `binding-drift.md` checks if the scope has bindings.
4. Cross-check against `financial-invariants.md` — every finding that touches numerics, FX, serde, parity, or parallelism must be flagged as invariant-sensitive.
5. Produce the **Audit Report** using `examples/audit-report.md`.

**Deliverable:** one markdown report. Nothing else.

**Done when:** report covers all categories and is sorted by (Impact desc, Risk asc).

**Checkpoint:** present to the user. Wait for priorities and scope confirmation before Phase 2.

---

## Phase 2 — Plan

**Inputs:** the audit report + user-picked priorities.

**Actions:**
1. Break the prioritized findings into **PR-sized slices**. A slice is a single coherent refactor that lands as one commit. Rough sizing: 1–5 files touched, <300 lines changed net, one theme per slice.
2. Order slices by **risk tier** (see below), lowest-risk first. Within a tier, order by blast radius (deletions first, renames second, signature changes last).
3. For each slice, capture: theme, files touched (Rust + bindings), expected net LOC delta, invariants touched, verify commands, risk tier, dependencies on other slices.
4. Produce the **Consolidation Plan** using `examples/consolidation-plan.md`.

**Risk tiers:**
- **Tier 1 — Delete-only:** removing dead code, unused variants, orphaned files. No call-sites change semantically. Verify: `mise run rust-lint && mise run rust-test`.
- **Tier 2 — Internal collapse:** inlining private wrappers, collapsing internal single-impl traits, merging duplicate private helpers. No public surface change. Verify: Rust-side only.
- **Tier 3 — Public surface simplification:** removing public parallel APIs, collapsing `try_*` shadows, renaming public symbols. Binding updates required. Verify: full stack.
- **Tier 4 — Invariant-sensitive:** anything that touches serde, Decimal math, FX policy, parity contract, evaluator precedence. Verify: full stack + golden tests + parity tests + explicit user sign-off before merge.

**Deliverable:** one markdown plan with slice-by-slice breakdown.

**Done when:** plan lists every slice, orders by risk, and specifies verify commands per slice.

**Checkpoint:** user picks the slice to execute next. **Never execute more than one slice per turn.**

---

## Phase 3 — Refactor (one slice at a time)

**Inputs:** one selected slice from the plan.

**Actions:**
1. Re-read the files touched by this slice (don't rely on stale memory — the tree may have changed since the audit).
2. Apply the relevant tactic(s) from `refactor-tactics.md`.
3. Edit only the files in the slice. If you find yourself reaching outside the slice boundary, stop and re-scope with the user.
4. Binding rule: if the slice touches a public Rust symbol, it also touches the Python binding, the WASM binding, `.pyi`, and `parity_contract.toml` in the same commit — or the slice isn't done.
5. Produce the **Refactor Diff** note using `examples/refactor-diff.md`.

**Deliverable:** the edits + a short refactor-diff note.

**Done when:** all intended changes applied, no orphan references, refactor-diff note written.

**Don't commit yet.** Verify comes first.

---

## Phase 4 — Verify

**Inputs:** the changes from Phase 3 + the slice's risk tier.

**Actions (in order):**

### Every slice

```bash
mise run rust-lint
mise run rust-test
```

Prefer the repo's `mise run` tasks from `AGENTS.md`. Use focused checks while iterating, then broader lint/test gates when the refactor slice crosses crate or binding boundaries.

### If the slice touches Rust that is bound to Python

```bash
mise run python-build       # release profile — debug is too slow for portfolio
mise run python-lint
mise run python-test
```

(Note: `AGENTS.md` warns that debug Python builds are "too slow for portfolio valuation." The Makefile uses `MATURIN_PROFILE=release` for `python-dev` by design.)

### If the slice touches Rust that is bound to WASM

```bash
mise run wasm-build
mise run wasm-lint
mise run wasm-test
```

### If the slice touches the WASM UI layer

```bash
mise run lint-ui
mise run test-ui
```

### If the slice is Tier 3 or Tier 4 (any public surface change or invariant-sensitive)

```bash
uv run pytest finstack-quant-py/tests/parity -x
```

Plus: diff golden test outputs serial vs parallel for any invariant-sensitive slice.

### Output rule

Paste the actual last-10 lines of each command you ran into your response. **Never claim green without showing it.** If the output is too long, include at least the final status line (pass/fail summary).

### If anything fails

**Stop.** Do not "fix it and keep going."

1. Report the failing command and the error text.
2. State your hypothesis about the root cause.
3. Propose a minimal fix (either revert the slice or a narrow addendum).
4. Wait for the user's decision.

Root causes are usually one of:
- Parity drift: a public symbol changed and bindings/parity weren't updated.
- Golden test divergence: numerical behavior changed; the simplification was not behavior-preserving.
- Lint: a new `#[allow(...)]` is needed, OR (more likely) the refactor can be done without one.
- Serde fixture: an inbound field was renamed without an alias.

**Never skip hooks or use `--no-verify`.** If a hook fails, investigate the underlying issue.

### Done criteria

The slice is done when:
- All relevant verify commands pass and the output is in the record.
- The refactor-diff note is complete.
- The user has been offered: continue / re-audit / stop.

---

## Commit boundaries

**One slice = one commit.** Each commit message should:

- Have a subject line like `refactor(<crate>): <slice theme>` (matches project's Conventional Commits style from recent `git log`).
- In the body, reference the audit finding(s) addressed.
- In the body, reference the verify commands that passed.

Example:
```
refactor(statements): collapse dual check runners

Audit cluster 3 (checks/runner.rs vs checks/suite.rs). Collapsed runner
into suite; runner.rs deleted. All call-sites updated. Binding surface
unchanged.

Verified: mise run rust-lint, mise run rust-test, mise run python-lint,
mise run python-test, mise run wasm-lint, mise run wasm-test. All green.
```

**Do not squash multiple slices into one commit.** The user reviews slices one at a time; squashing defeats the point.

---

## When to rebuild the bindings

Per `.cursor/rules/project-rules.md`: **if you change the Rust library, you will need to rebuild the Python and WASM bindings before using in python/wasm.**

- Rebuild Python: `mise run python-build` (release).
- Rebuild WASM: `mise run wasm-build`.

If your slice touched `finstack-quant/*` but not the binding crates, you still need to rebuild **if you want the binding tests to pick up the change.** Always rebuild before running `mise run python-test` or `mise run wasm-test`.

---

## Handling a multi-slice session

If the user wants to work through several slices in one session:

1. Execute slice 1 → Verify → commit → **checkpoint with user**.
2. Re-read the plan — it may need updating (slice 2's assumptions may have shifted).
3. Execute slice 2 → Verify → commit → checkpoint.
4. Repeat.

Do **not** execute slices 2, 3, 4 in a single turn without checkpoints, even if the user says "just do the next three." Checkpoints protect against cascading errors.

**Exception:** if the user explicitly says "execute slices 2 through 4 without checkpointing, I trust the plan," comply. But require a full stack verify after each slice before starting the next — that's non-negotiable.

---

## When to abandon a slice

Abandon a slice and replan if:

- A Verify step fails and the fix would materially change the slice's scope.
- You discover a new finding mid-slice that makes the planned fix wrong.
- You find that the slice depends on a different (unplanned) slice being done first.

Abandonment protocol:
1. `git stash` or revert the work-in-progress.
2. Update the Consolidation Plan with what you learned.
3. Ask the user which slice to tackle next.

Don't power through a slice that's gone wrong — the cost of a bad commit is higher than the cost of replanning.

---

## Anti-patterns in this workflow

- **"Just audit then do it all"**: no. The point of phasing is the checkpoint. Audit → checkpoint → plan → checkpoint → slice 1 → verify → checkpoint. Every transition.
- **"Run tests at the end"**: no. Verify after every slice. Batched verification hides which slice broke what.
- **"Minor cleanup while I'm here"**: no. Every change that's not in the current slice goes in its own slice. This keeps diffs reviewable and rollbacks cheap.
- **"Ship a half-migration"**: no. A slice that leaves the tree in an intermediate state (old path deprecated but not deleted, new path not wired everywhere) is a landmine. Either fully migrate or don't start.
- **"Skip parity tests because it's 'just' a rename"**: no. Renames are exactly the change parity tests exist to catch.
