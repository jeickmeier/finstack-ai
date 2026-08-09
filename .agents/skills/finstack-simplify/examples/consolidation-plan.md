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
- Every binding-sensitive slice updates Rust + Python + WASM + `.pyi` + parity in one commit.
- Target size: 1–5 files per slice, <300 LOC net change. Larger slices get broken up.

## Slice 1 — <theme>

**Tier:** 1 (delete-only)
**Estimated net LOC:** −180
**Files touched:**
- `finstack-quant/statements/src/checks/runner.rs` (DELETE)
- `finstack-quant/statements/src/checks/mod.rs` (remove `pub mod runner;`)
- `finstack-quant/statements/src/prelude.rs` (remove re-export)
- `finstack-quant/statements/tests/spec/registry_tests.rs` (remove one unreferenced test)

**Addresses findings:** F1, F2

**Invariants touched:** none

**Rationale:** The module is orphaned — `git grep` shows no call-sites outside its own file, and the binding layer never exposed it. Safe delete.

**Verify:**
```bash
mise run rust-lint && mise run rust-test
```

**Bindings touched:** none. Python/WASM tests don't need to re-run.

**Rollback:** straight `git revert`. No downstream impact.

## Slice 2 — <theme>

**Tier:** 2 (internal collapse)
**Estimated net LOC:** −95
**Files touched:**
- `finstack-quant/statements/src/checks/suite.rs`
- `finstack-quant/statements/src/checks/traits.rs` (single-impl trait deleted)
- `finstack-quant/statements/src/checks/builtins/*.rs` (update to use concrete type)

**Addresses findings:** F3

**Invariants touched:** none

**Rationale:** Applying tactic T3 (inline single-impl trait). The `CheckRunner` trait has exactly one impl; removing it simplifies every call-site.

**Verify:**
```bash
mise run rust-lint && mise run rust-test
```

**Bindings touched:** none (the trait was internal).

**Rollback:** revert; no external observable change.

**Depends on:** Slice 1 (cleaner file tree makes the trait's scope easier to reason about).

## Slice 3 — <theme>

**Tier:** 3 (public surface change)
**Estimated net LOC:** −160
**Files touched:**
- `finstack-quant/core/src/market_data/surfaces/vol_surface.rs`
- `finstack-quant/core/src/market_data/surfaces/mod.rs`
- `finstack-quant-py/src/bindings/core/market_data/surfaces.rs`
- `finstack-quant-wasm/src/api/core_ns/market_data/surfaces.rs`
- `finstack-quant-py/finstack_quant/core.pyi`
- `parity_contract.toml`

**Addresses findings:** F4, F5 (cluster A)

**Invariants touched:** none directly, but any numerical test that hits `VolSurface::new` needs to keep passing.

**Rationale:** Applying tactic T4 (collapse parallel constructors) + T7 (move binding logic to Rust). Currently `VolSurface::new`, `VolSurface::from_grid`, and free fn `vol_surface_from_points` are three paths to the same struct. Collapse to `VolSurface::new(VolSurfaceInput) -> Result<Self, Error>`.

**Verify:**
```bash
mise run rust-lint && mise run rust-test
mise run python-build
mise run python-lint && mise run python-test
mise run wasm-build
mise run wasm-lint && mise run wasm-test
uv run pytest finstack-quant-py/tests/parity -x
```

**Bindings touched:** Python + WASM both updated. Parity contract updated.

**Rollback:** revert requires reverting parity + `.pyi` too. Keep as one commit.

**Depends on:** Slice 2 (this slice removes a trait internal to surfaces; doing it after the internal cleanup reduces blast radius).

## Slice 4 — <theme>

**Tier:** 4 (invariant-sensitive)
**Estimated net LOC:** −50 (but high diff surface)
**Files touched:**
- `finstack-quant/analytics/src/drawdown.rs`
- `finstack-quant/portfolio/src/drawdown.rs` (DELETE — merges into analytics)
- Various call-sites in `valuations/`, `statements-analytics/`
- Python + WASM bindings for both `analytics.drawdown` and `portfolio.drawdown`
- `parity_contract.toml`
- `finstack-quant-py/finstack_quant/*.pyi`

**Addresses findings:** F6

**Invariants touched:** Decimal equality (numerical output must not change).

**Rationale:** Two drawdown implementations with subtle differences in NaN handling. Merge into one in `analytics/`, delete the `portfolio/` version. Needs explicit user sign-off before execution — the merge could expose existing consumers to NaN-handling changes.

**Verify:**
```bash
mise run rust-lint && mise run rust-test
# Run golden tests twice, diff:
mise run rust-test  # serial
RAYON_NUM_THREADS=1 mise run rust-test  # explicit serial
mise run python-build && mise run python-lint && mise run python-test
mise run wasm-build && mise run wasm-lint && mise run wasm-test
uv run pytest finstack-quant-py/tests/parity -x
# Review golden diffs for all drawdown tests in analytics/ and portfolio/.
```

**Bindings touched:** Yes, full stack.

**Rollback:** Hard — touches parity. Keep as one atomic commit.

**Depends on:** user sign-off. Do not execute without explicit "go."

## Slice dependency graph

```
Slice 1 (delete dead) ───► Slice 2 (collapse trait) ───► Slice 3 (surface constructor)
                                                                │
                                                                ▼
Slice 4 (drawdown merge, needs user sign-off) ◄──────────────── │
```

## Not in this plan

Findings explicitly excluded:

- **F8 (documentation gaps):** out of scope; suggest running `finstack-documentation-maintainer` skill separately.
- **F9 (performance concern in surface interp):** out of scope; suggest running `finstack-performance-reviewer` separately.
- **H1 (`.unwrap()` in binding):** this is a bug, not slop. Suggest running `finstack-quality-gate-triage` skill separately.

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
