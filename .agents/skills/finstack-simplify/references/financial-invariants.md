# Financial invariants: what you MUST NOT break while simplifying

Simplification is a net positive only if it doesn't silently change behavior. In finstack, "behavior" includes several invariants that are load-bearing for downstream users (accounting pipelines, golden tests, regulatory outputs). Every refactor slice must be checked against this list.

When in doubt, keep the old behavior and flag the invariant in the audit. **Never "clean up" one of these without explicit user sign-off.**

---

## 1. Decimal equality (determinism)

**Invariant:** In Decimal mode, numerical outputs are bit-identical run-to-run, parallel-vs-serial, and across revisions of this library (modulo explicit changes to algorithms).

**How this is broken by refactoring:**
- Replacing `Decimal` arithmetic with `f64` "because it's faster".
- Reordering operations on Decimal in a way that changes intermediate scale (even if the final mathematical result is the same).
- Adding implicit rounding at new layer boundaries.
- Using `f64` for intermediates and only Decimal for inputs/outputs.
- Changing the order of iteration over an unordered collection (e.g., switching from `BTreeMap` to `HashMap`) when reduction is non-associative for the target type.

**What to do:**
- If you touch any numerical code path, run the golden tests twice with the exact same inputs and diff the results. Run once serial, once parallel. Diff those too.
- Never swap Decimal → f64 without explicit user sign-off; this is a semantic change, not a simplification.
- Never reorder fold/reduce over a `HashMap` or `HashSet`; collapse into an ordered container first.

**Detect risk:** `grep -n "f64\|as f64\|HashMap<.*Decimal\|HashSet" in the scope of your refactor.

---

## 2. Currency safety

**Invariant:** Arithmetic on `Amount` requires same-currency operands. Cross-currency math is explicit via `FxProvider` / `FxMatrix`. Every FX conversion is recorded in results metadata with the strategy used.

**How this is broken by refactoring:**
- Adding a helper that "auto-converts" currencies using a default rate.
- Relaxing a `Result<Amount, CurrencyMismatch>` return to a plain `Amount` that panics on mismatch.
- Introducing an `impl Add for Amount` that assumes same currency and panics otherwise (the existing impl should be a `Result`; if you find a panicking one, that's a bug, not a target for simplification).
- Collapsing two functions where one took an `FxProvider` and the other didn't — the non-`FxProvider` version is only infallible because it assumes a single currency; losing that assumption is a semantic change.

**What to do:**
- Any refactor that touches `Amount`, `Currency`, `FxProvider`, `FxMatrix`, `FxPolicy`, or result metadata carrying FX strategy must be flagged in the audit as "FX-sensitive."
- Prefer the FX-aware signature as the canonical entry point when collapsing pathways; the non-FX version can become a method on `Amount` with a precondition that's visible at the type level (e.g., `assert_same_currency`).

---

## 3. FX policy visibility

**Invariant:** The applied FX conversion strategy is recorded per layer (valuations, statements, portfolio). Downstream readers can inspect a result and see which strategy produced it.

**How this is broken by refactoring:**
- Dropping the `fx_policy` field from a result struct "because no one reads it."
- Collapsing two result types where one had the field and the other didn't.
- Constructing a new result struct via `Default::default()` and forgetting to stamp the active policy.

**What to do:**
- If your refactor touches a result-envelope struct (look for field names like `fx_policy`, `applied_policy`, `conversion_strategy`, `numeric_mode`, `parallel`, `rounding_context`), flag it. Keep the fields, even if the compiler tells you they're "unused" — they're part of the serde contract.

---

## 4. Serde stability (unknown-field-deny)

**Invariant:** Serde field names are stable. Inbound types deny unknown fields. This supports long-lived pipelines and golden tests.

**How this is broken by refactoring:**
- Renaming a field without a `#[serde(rename = "old_name")]` alias.
- Removing a variant from a tag enum.
- Changing a `flatten` boundary.
- Switching a field from `Option<T>` to `T` (breaks inbound payloads that omit the field).

**What to do:**
- If your refactor touches a struct or enum annotated with `#[derive(Serialize, Deserialize)]` and that type is part of a public API boundary (input or output of a scenario DSL, a statement model, a pricing request), **stop** and flag it. Serde changes go in their own slice with explicit migration notes.
- If in doubt, grep for the type name in `**/*.json`, `**/*.toml`, and `finstack-quant-py/tests/` — if it appears in test fixtures, it's a public contract.

---

## 5. Rounding context metadata

**Invariant:** Every numerical result carries the active `RoundingContext` used to produce it. This is separate from `RoundingConfig` (input) — the `RoundingContext` is stamped into results so downstream consumers can reason about precision.

**How this is broken by refactoring:**
- Collapsing `RoundingContext` and `RoundingConfig` into one type (tempting, since they look similar — but one is output metadata, the other is input).
- Dropping the stamp because "no one looks at it."

**What to do:**
- Keep these separate. Mention them both in the audit if you see a simplification opportunity that might touch either.

---

## 6. Parallel ≡ Serial

**Invariant:** Results in Decimal mode are identical whether a computation runs serially or in parallel.

**How this is broken by refactoring:**
- Adding a `par_iter()` in a reduction where the binary op isn't associative at the Decimal level.
- Removing a `par_iter()` in favor of a sequential loop where the previous code had a serial-equivalent guarantee — usually fine, but worth verifying.
- Changing the partitioning strategy of a `par_chunks_mut` in a way that affects accumulation order.

**What to do:**
- Any refactor that touches `rayon`, `par_iter`, `par_chunks`, `par_extend`, or a custom parallel fold must be flagged "parallelism-sensitive." The simplification is probably fine; the verification is: run the golden test twice, once with `RAYON_NUM_THREADS=1`, once with the default, and diff.

---

## 7. ISDA day-counts and calendar conventions

**Invariant:** Day-count and business-day convention handling is ISDA-conformant. These are numerical but with a long paper trail of edge cases (leap days, end-of-month, holiday calendars).

**How this is broken by refactoring:**
- Replacing a day-count implementation with "a simpler one" without running the property tests against the ISDA test vectors.
- Collapsing two calendar implementations where one had a subtle policy difference.

**What to do:**
- Any refactor in `finstack-quant/core/src/time/` or anywhere touching `DayCount`, `BusinessDayConvention`, `Calendar`, `Holiday` must be flagged "day-count-sensitive." Read the existing tests carefully — they exist for a reason.

---

## 8. Parity contract (Rust ↔ Python ↔ WASM)

**Invariant:** See `binding-drift.md`. Parity contract entries are how downstream users trust the bindings.

**How this is broken by refactoring:**
- Renaming a Rust public symbol without updating parity.
- Deleting a Rust public symbol without deleting the parity entry.
- Adding a Python-only "convenience" wrapper (logic drift).

**What to do:**
- Every slice that touches a public Rust symbol re-runs `uv run pytest finstack-quant-py/tests/parity`. If it fails, the slice is not done.

---

## 9. Model evaluation precedence (Value > Forecast > Formula)

**Invariant:** In `finstack-quant/statements`, period-level evaluation follows a deterministic precedence: explicit Values beat Forecasts beat Formulas. This is not an implementation detail; it's a contract documented in the project description.

**How this is broken by refactoring:**
- Reordering the phases of an evaluator "to be more consistent."
- Collapsing two evaluators whose semantics differed on this precedence.

**What to do:**
- Never touch the precedence order. If a refactor *tempts* you to, stop and escalate to the user.

---

## 10. Unsafe code / `unwrap` / `panic` in bindings

**Invariant:** Binding crates have `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::panic)]`, `#![forbid(unsafe_code)]`. This is per `AGENTS.md` and it's enforced by clippy.

**How this is broken by refactoring:**
- Adding `.unwrap()` in a binding to "simplify" an error path.
- Adding `unsafe` "because it's faster."

**What to do:**
- These will fail lint. If you're tempted, you've misread the simplification opportunity — the answer is to improve the error type or add a graceful fallback, not to panic.

---

## 11. Polars DataFrame/Series as the canonical time-series surface

**Invariant:** Polars is the canonical DataFrame/Series surface (re-exported from `core`). Time-series outputs use Polars, not ad-hoc series types.

**How this is broken by refactoring:**
- "Simplifying" a Polars return type to a `Vec<(DateTime, Decimal)>` for ergonomics.
- Introducing a new ad-hoc series type instead of reusing Polars.

**What to do:**
- Keep Polars surfaces. If ergonomics are painful, the fix is a helper function, not a new series type.

---

## Invariant checklist for every refactor slice

Before committing:

- [ ] Did I change any Decimal arithmetic? → run Decimal-equality golden tests.
- [ ] Did I change any FX-aware code? → confirm the FX-aware path is the canonical one; confirm policy stamping.
- [ ] Did I change any serde'd public type? → check rename aliases; check JSON/TOML fixtures; run parity tests.
- [ ] Did I change any public Rust symbol visible to bindings? → update both bindings; update `.pyi`; update parity contract.
- [ ] Did I change any parallel reduction? → run golden test serially AND parallel; diff.
- [ ] Did I change any day-count or calendar code? → run ISDA vectors.
- [ ] Did I change model evaluator phase order? → stop. You shouldn't.
- [ ] Did I introduce `unsafe` / `unwrap` / `panic` in a binding? → stop. Lint will catch it; the refactor is wrong.

If any of the above is yes and the answer is "not verified", you're not done.
