# Behavioral invariants: what you MUST NOT break while simplifying

Simplification is a net positive only if it doesn't silently change behavior. In finstack, "behavior" includes several invariants that are load-bearing for downstream users (bindings, journals, plugins, conformance fixtures). Every refactor slice must be checked against this list.

When in doubt, keep the old behavior and flag the invariant in the audit. **Never "clean up" one of these without explicit user sign-off.**

---

## 1. Kernel determinism and I/O freedom

**Invariant:** `finstack-ai-kernel` is synchronous and I/O-free. Semantic decisions are deterministic given the same committed records. `decide` does not mutate state; only committed records are applied.

**How this is broken by refactoring:**
- Adding file, network, clock, or RNG access to kernel code.
- Making `decide` mutate in-place "because it's simpler."
- Reordering reductions over unordered collections when the result is part of durable history.

**What to do:**
- If you touch kernel decision or apply paths, run the targeted kernel tests twice with the same inputs and diff the results.
- Never introduce I/O or time into the kernel without explicit user sign-off; this is a semantic change, not a simplification.

**Detect risk:** `grep -n "std::fs\|std::net\|SystemTime\|Instant\|rand\|HashMap" in the kernel scope of your refactor.

---

## 2. Commit-before-effect

**Invariant:** Recoverable effect intent is committed before execution, including in-memory execution. Original effect/batch identity, idempotent duplicate handling, fail-closed conflicts, explicit uncertainty, and run lineage are preserved.

**How this is broken by refactoring:**
- Executing a tool, model, or store write before the corresponding record is committed.
- Collapsing commit and execute into one helper that hides the ordering.
- Dropping lineage or batch identity fields "because no one reads them."

**What to do:**
- Any refactor that touches effect dispatch, recovery, or journal apply must be flagged "lifecycle-sensitive."
- Prefer the commit-then-execute path as the canonical entry point when collapsing pathways.

---

## 3. Serde and protocol stability

**Invariant:** Serde field names, durable record/event kind names, error `code` strings, and WIT identifiers are stable. Inbound types that deny unknown fields stay that way.

**How this is broken by refactoring:**
- Renaming a field without a `#[serde(rename = "old_name")]` alias.
- Removing a variant from a tag enum.
- Changing a `flatten` boundary.
- Switching a field from `Option<T>` to `T` (breaks inbound payloads that omit the field).

**What to do:**
- If your refactor touches a struct or enum annotated with `#[derive(Serialize, Deserialize)]` and that type is part of a public API, journal, or protocol boundary, **stop** and flag it. Serde changes go in their own slice with explicit migration notes.
- If in doubt, grep for the type name in `**/*.json`, `**/*.toml`, and binding tests — if it appears in fixtures, it's a public contract.

---

## 4. Binding parity (Rust ↔ Python ↔ WASM)

**Invariant:** See `binding-drift.md`. Frozen public items and conformance fixtures are how downstream users trust the bindings.

**How this is broken by refactoring:**
- Renaming a Rust public symbol without updating bindings and `uv run --no-project python scripts/compat/public_items.py --check`.
- Deleting a Rust public symbol without deleting the binding and public-item entry.
- Adding a Python-only or JS-only "convenience" wrapper (logic drift).

**What to do:**
- Every slice that touches a public Rust symbol re-runs `uv run --no-project python scripts/compat/public_items.py --check` and the affected binding tests. If they fail, the slice is not done.

---

## 5. Generated artifacts

**Invariant:** Generated output (wasm-bindgen glue, WIT bindgen, plugin guests, lockfiles) is regenerated via documented commands. Hand-edits of generated trees are forbidden.

**How this is broken by refactoring:**
- Editing `bindings/finstack-ai-wasm/js/generated/` by hand.
- Changing WIT packages without running `uv run --no-project python scripts/wit_bindgen/generate.py` / `uv run --no-project python scripts/wit_bindgen/generate.py --check`.
- Leaving generator drift uncommitted.

**What to do:**
- Regenerate via the documented command. Treat a dirty generated tree as a broken change.

---

## 6. Observer and compaction contracts

**Invariant:** Observers never change behavior or terminal state. Model-context compaction has at most one active late-tier `before_model` owner per resolved agent.

**How this is broken by refactoring:**
- Giving an observer a write path "for convenience."
- Adding a second late-tier compaction owner.
- Collapsing observer and middleware types that had different authority.

**What to do:**
- Flag any refactor that touches observers, middleware stages, or compaction ownership.

---

## 7. Parallel tool order vs durable history

**Invariant:** Parallel tool calls may complete out of order, but durable history and final semantic events preserve model source order.

**How this is broken by refactoring:**
- Recording completion order as source order.
- Changing the partitioning strategy of a parallel fold in a way that affects durable event order.

**What to do:**
- Any refactor that touches `rayon`, `par_iter`, tool-batch completion, or event emission must be flagged "ordering-sensitive."

---

## 8. Unsafe code / `unwrap` / `panic` in bindings

**Invariant:** Binding crates should not introduce `unwrap`, `expect`, `panic`, or `unsafe` on non-test paths.

**How this is broken by refactoring:**
- Adding `.unwrap()` in a binding to "simplify" an error path.
- Adding `unsafe` "because it's faster."

**What to do:**
- These will fail lint. If you're tempted, you've misread the simplification opportunity — the answer is to improve the error type or add a graceful fallback, not to panic.

---

## Invariant checklist for every refactor slice

Before committing:

- [ ] Did I change kernel decide/apply or add I/O? → run kernel tests twice; confirm I/O-free.
- [ ] Did I change effect commit/execute ordering? → confirm commit-before-effect.
- [ ] Did I change any serde'd public type? → check rename aliases; check JSON/TOML fixtures.
- [ ] Did I change any public Rust symbol visible to bindings? → update both bindings; update `.pyi`; run `uv run --no-project python scripts/compat/public_items.py --check`.
- [ ] Did I change generated artifacts? → regenerate via the documented command.
- [ ] Did I change observer or compaction ownership? → stop and flag.
- [ ] Did I change parallel completion vs durable order? → run ordering/conformance tests.
- [ ] Did I introduce `unsafe` / `unwrap` / `panic` in a binding? → stop. The refactor is wrong.

If any of the above is yes and the answer is "not verified", you're not done.
