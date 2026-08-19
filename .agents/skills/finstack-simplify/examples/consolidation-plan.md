# Consolidation Plan — Phase 2 output template

Use this format verbatim.

---

# Consolidation Plan: `<crate>::<module>`

**Based on:** Audit Report dated YYYY-MM-DD
**User priorities:** <short summary of what the user said to focus on>
**Plan date:** YYYY-MM-DD

## Slicing principles applied

- One theme per slice.
- Tier 1 (delete-only) slices first.
- Within a tier, deletions → internal collapses → public surface changes.
- Every binding-sensitive slice updates Rust + Python + WASM + `.pyi` + public items in one commit.
- Target size: 1–5 files per slice, <300 LOC net change. Larger slices get broken up.

## Slice 1 — <theme>

**Tier:** 1 (delete-only)
**Estimated net LOC:** −180
**Files touched:**
- `crates/finstack-ai-kernel/src/reducer/legacy.rs` (DELETE)
- `crates/finstack-ai-kernel/src/reducer/mod.rs` (remove `pub mod legacy;`)
- `crates/finstack-ai-kernel/tests/legacy_apply.rs` (remove one unreferenced test)

**Addresses findings:** F1, F2

**Invariants touched:** none

**Rationale:** The module is orphaned — `git grep` shows no call-sites outside its own file, and the binding layer never exposed it. Safe delete.

**Verify:**
```bash
mise run check-all && mise run test-all
```

**Bindings touched:** none. Python/WASM tests don't need to re-run.

**Rollback:** straight `git revert`. No downstream impact.

## Slice 2 — <theme>

**Tier:** 2 (internal collapse)
**Estimated net LOC:** −95
**Files touched:**
- `crates/finstack-ai-kernel/src/reducer/apply/shapes.rs`
- `crates/finstack-ai-kernel/src/reducer/apply/traits.rs` (single-impl trait deleted)

**Addresses findings:** F3

**Invariants touched:** none

**Rationale:** Applying tactic T3 (inline single-impl trait). The trait has exactly one impl; removing it simplifies every call-site.

**Verify:**
```bash
mise run check-all && mise run test-all
```

**Bindings touched:** none (the trait was internal).

**Rollback:** revert; no external observable change.

**Depends on:** Slice 1 (cleaner file tree makes the trait's scope easier to reason about).

## Slice 3 — <theme>

**Tier:** 3 (public surface change)
**Estimated net LOC:** −160
**Files touched:**
- `crates/finstack-ai/src/agent.rs`
- `bindings/finstack-ai-python/python/finstack_ai/__init__.py`
- `bindings/finstack-ai-python/python/finstack_ai/_finstack_ai.pyi`
- `bindings/finstack-ai-wasm/js/src/agent.ts`

**Addresses findings:** F4, F5 (cluster A)

**Invariants touched:** none directly, but any test that hits `Agent::new` needs to keep passing.

**Rationale:** Applying tactic T4 (collapse parallel constructors). Currently `Agent::new`, `Agent::from_spec`, and a free fn are three paths to the same struct. Collapse to `Agent::new(AgentSpec) -> Result<Self, Error>`.

**Verify:**
```bash
mise run check-all && mise run test-all
mise run build-wasm -- release && mise run check-wasm
uv run --no-project python scripts/compat/public_items.py --check
```

**Bindings touched:** Python + WASM both updated. Public-item inventory updated.

**Rollback:** revert requires reverting stubs too. Keep as one commit.

**Depends on:** Slice 2.

## Slice 4 — <theme>

**Tier:** 4 (invariant-sensitive)
**Estimated net LOC:** −50 (but high diff surface)
**Files touched:**
- `crates/finstack-ai-kernel/src/effects/`
- Various call-sites in runtime
- Python + WASM bindings for the same surface

**Addresses findings:** F6

**Invariants touched:** commit-before-effect (ordering must not change).

**Rationale:** Two dispatch helpers with subtle differences in commit ordering. Merge into one. Needs explicit user sign-off before execution.

**Verify:**
```bash
mise run check-all && mise run test-all
mise run build-wasm -- release && mise run check-wasm
uv run --no-project python scripts/compat/public_items.py --check
cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests
```

**Bindings touched:** Yes, full stack.

**Rollback:** Hard — touches public items. Keep as one atomic commit.

**Depends on:** user sign-off. Do not execute without explicit "go."

## Slice dependency graph

```
Slice 1 (delete dead) ───► Slice 2 (collapse trait) ───► Slice 3 (constructor)
                                                                │
                                                                ▼
Slice 4 (dispatch merge, needs user sign-off) ◄──────────────── │
```

## Not in this plan

Findings explicitly excluded:

- **F8 (documentation gaps):** out of scope; suggest running `finstack-documentation-maintainer` skill separately.
- **F9 (performance concern):** out of scope; suggest running `finstack-performance-reviewer` separately.
- **H1 (`.unwrap()` in binding):** this is a bug, not slop. Schedule a separate fix.

## What we expect at the end

After all slices land:

- Lines removed net: ~485
- Public surface items removed: 7
- Single-impl traits removed: 1
- Parallel constructors collapsed: 2
- Binding drift resolved: 2 items

The user should be able to explain the module's public API in one paragraph to a new hire, which they cannot today.

## Next

**Awaiting user input:** which slice should I execute first?
