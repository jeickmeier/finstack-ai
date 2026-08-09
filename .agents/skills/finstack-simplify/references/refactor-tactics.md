# Refactor tactics: the concrete moves

Each tactic has: **when to use**, **when NOT to use**, and a **before/after** in finstack-quant idiom.

Apply tactics one at a time per slice. Don't chain multiple tactics unless the chain is the slice — e.g., "Inline the single-impl trait AND delete the resulting wrapper" is a reasonable coupled slice.

---

## T1 — Delete

**When:** Dead code, unused variants, unused imports, commented-out blocks, orphaned files, unused trait bounds.

**Not when:** The code is reachable through a feature flag you haven't verified disabled. Check `Cargo.toml` features and conditional `cfg` attrs before deleting.

**Example:**

*Before:*
```rust
// in statements/src/checks/runner.rs
pub struct LegacyCheckRunner { /* ... */ }
impl LegacyCheckRunner {
    pub fn run_all(&self) -> Vec<CheckResult> { /* dead — no call-sites */ }
}
```

*After:*
Delete `runner.rs`. Remove `pub mod runner;` from `checks/mod.rs`. Remove any re-exports from `prelude.rs`.

**Size of slice:** one deletion = one slice. Batching multiple unrelated deletions is fine as "Tier 1 — cleanup sweep" — but not multi-crate, not multi-module unless they're genuinely orphaned together.

---

## T2 — Collapse (delegation chains)

**When:** A public function `A` only calls `B`, which only calls `C`, which does the work. Intermediaries add nothing.

**Not when:** `B` or `C` does error translation, type conversion, or normalization that actually matters.

**Example:**

*Before:*
```rust
// in analytics/src/sharpe.rs
pub fn sharpe(returns: &[f64], rf: f64) -> f64 {
    sharpe_impl(returns, rf)
}

fn sharpe_impl(returns: &[f64], rf: f64) -> f64 {
    sharpe_core(returns, rf, 252.0)
}

fn sharpe_core(returns: &[f64], rf: f64, annualization: f64) -> f64 {
    // real work
}
```

*After:*
```rust
pub fn sharpe(returns: &[f64], rf: f64, annualization: f64) -> f64 {
    // real work inlined
}
```

If call-sites always pass `252.0`, keep the signature but document the default expectation; don't hide it behind a wrapper.

---

## T3 — Inline (single-impl traits)

**When:** A trait has exactly one impl, is not used for mocking, and callers could use the concrete type directly.

**Not when:** The trait is load-bearing for polymorphic dispatch even with one current impl (e.g., a plugin point that genuinely will grow). Be strict: "it might grow" is not justification; "there's a second impl landing next week" is.

**Example:**

*Before:*
```rust
pub trait ScenarioAdapter {
    fn apply(&self, state: &mut State) -> Result<(), Error>;
}

pub struct MarketAdapter { /* ... */ }
impl ScenarioAdapter for MarketAdapter { /* only impl */ }

pub fn run<A: ScenarioAdapter>(adapter: A, state: &mut State) -> Result<(), Error> {
    adapter.apply(state)
}
```

*After:*
```rust
pub struct MarketAdapter { /* ... */ }
impl MarketAdapter {
    pub fn apply(&self, state: &mut State) -> Result<(), Error> { /* ... */ }
}

pub fn run(adapter: &MarketAdapter, state: &mut State) -> Result<(), Error> {
    adapter.apply(state)
}
```

Delete the trait. Tests that needed polymorphism can use a test-specific mock by accepting a closure or by composing fakes directly against `MarketAdapter`.

---

## T4 — Collapse parallel constructors

**When:** A type has multiple `new`-ish constructors (`new`, `from_parts`, `try_new`, `build_new`, `create`) that do overlapping work.

**Not when:** Each constructor has a genuinely different input type and the difference encodes a precondition (e.g., `Currency::from_iso_str(&str)` vs `Currency::from_validated(ValidatedIso)` — the latter can't fail because the input is pre-validated).

**Example:**

*Before:*
```rust
impl DiscountCurve {
    pub fn new(pillars: Vec<Pillar>) -> Self { /* panics on bad input */ }
    pub fn try_new(pillars: Vec<Pillar>) -> Result<Self, Error> { /* Result version */ }
    pub fn from_market_data(md: &MarketData) -> Self { /* calls new() */ }
    pub fn build(builder: CurveBuilder) -> Self { /* calls new() */ }
}
```

*After:*
```rust
impl DiscountCurve {
    pub fn new(pillars: Vec<Pillar>) -> Result<Self, Error> {
        // Validate input, construct.
    }
}

impl From<MarketData> for DiscountCurve {
    type Error = Error;
    fn try_from(md: MarketData) -> Result<Self, Self::Error> {
        Self::new(md.into_pillars())
    }
}
```

- One `new`, returns `Result`. The panicking variant is gone (per clippy rules in bindings, it's unusable anyway).
- `From`/`TryFrom` impls for conversion sources.
- `CurveBuilder` becomes an internal helper that ultimately calls `DiscountCurve::new`.

---

## T5 — Replace single-instantiation generic with concrete

**When:** A generic function or struct is only ever instantiated with one concrete type in the workspace (and is not exposed as a library extension point).

**Not when:** The generic is used at a binding boundary or explicitly documented as a library extension point.

**Example:**

*Before:*
```rust
pub fn evaluate_period<T: Numeric>(ctx: &Context<T>, period: Period) -> T { /* ... */ }
// Only ever called with T = Decimal.
```

*After:*
```rust
pub fn evaluate_period(ctx: &Context, period: Period) -> Decimal { /* ... */ }
```

Delete the `Numeric` trait if nothing else uses it. Update bindings — generics can't be exposed through PyO3 or wasm-bindgen anyway, so this usually *improves* the binding layer too.

---

## T6 — Collapse try_ / non-try pairs

**When:** A type has both `x()` and `try_x()`, where `x()` is just `try_x().expect(...)`.

**Example:**

*Before:*
```rust
impl Currency {
    pub fn new(iso: &str) -> Self { Self::try_new(iso).expect("bad ISO") }
    pub fn try_new(iso: &str) -> Result<Self, ParseCurrencyError> { /* ... */ }
}
```

*After:*
```rust
impl Currency {
    pub fn new(iso: &str) -> Result<Self, ParseCurrencyError> { /* ... */ }
}

// If the user wants an infallible construction from a validated source:
impl From<KnownCurrency> for Currency { /* infallible by type */ }
```

Use distinct input types, not distinct function names, to express the precondition difference.

---

## T7 — Move binding logic to Rust

**When:** A Python or WASM binding function contains logic, arithmetic, or multiple Rust calls.

**Example:**

*Before (Python binding):*
```rust
#[pyfunction]
fn compute_sharpe_from_df(df: &PyAny, rf: f64) -> PyResult<f64> {
    let returns: Vec<f64> = df.call_method0("to_list")?.extract()?;
    if returns.is_empty() { return Ok(0.0); }
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let var = returns.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / returns.len() as f64;
    let std = var.sqrt();
    if std == 0.0 { return Ok(0.0); }
    Ok((mean - rf) / std * 252f64.sqrt())
}
```

*After (Rust canonical):*
```rust
// in analytics/src/sharpe.rs
pub fn sharpe(returns: &[f64], rf: f64, annualization: f64) -> Option<f64> {
    if returns.is_empty() { return Some(0.0); }
    // ...standard sharpe...
}
```

*After (Python binding):*
```rust
#[pyfunction]
fn sharpe(returns: Vec<f64>, rf: f64, annualization: f64) -> PyResult<f64> {
    Ok(finstack_quant_analytics::sharpe(&returns, rf, annualization).unwrap_or(0.0))
}
```

Same refactor applied to WASM binding. `.pyi` updated. Parity contract updated.

---

## T8 — Unify error enums

**When:** A crate has multiple error types for the same semantic errors (e.g., `ParseError` and `ValidationError` that both represent "bad input").

**Not when:** The types represent genuinely different failure modes that callers handle differently.

**Example:**

*Before:*
```rust
pub enum ParseError { BadIso(String), BadDate(String), BadNumber(String) }
pub enum ValidationError { EmptyInput, NegativeRate, MismatchedLengths }
```

If both types are mapped the same way in bindings (both become `ValueError` in Python, both become `JsValue::from_str` in WASM), there's no caller that distinguishes them — they can be one type.

*After:*
```rust
pub enum Error {
    #[error("parse failed for {field}: {reason}")]
    Parse { field: String, reason: String },

    #[error("validation failed: {0}")]
    Validation(String),

    // plus #[source] chains for wrapped errors
}
```

Fewer variants, same information content, one mapping point in bindings.

---

## T9 — Shrink public surface (pub → pub(crate))

**When:** A `pub` item is not imported from outside its defining crate.

**Not when:** The item is re-exported at the crate root and consumed by external users (check the prelude and `lib.rs`).

**Example:** `pub fn internal_helper` in `statements/src/evaluator/forecast_eval.rs` is not used by any other crate.

*Fix:*
```rust
pub(crate) fn internal_helper(...) -> ... { /* ... */ }
```

Usually a safe, instant, Tier 2 refactor.

---

## T10 — Merge near-duplicates

**When:** Two functions/types/modules do essentially the same thing with minor variations.

**Procedure:**
1. Diff the two implementations side-by-side. Note every divergence.
2. For each divergence, decide: is it a real difference (parameterize), or noise (pick one)?
3. Build the merged version. Run tests for both old call-sites against the merged function.
4. If all green, replace call-sites one module at a time.

**Warning:** this tactic is the most likely to accidentally change numerical behavior. Before merging numerical code, run the golden test for both old paths, capture the outputs, then run the new path and diff.

---

## T11 — Demote wrapper types

**When:** A wrapper type adds no invariants or behavior beyond forwarding to an inner type.

*Before:*
```rust
pub struct CurveHandle {
    inner: Arc<DiscountCurve>,
}
impl CurveHandle {
    pub fn new(c: DiscountCurve) -> Self { Self { inner: Arc::new(c) } }
    pub fn discount(&self, t: f64) -> f64 { self.inner.discount(t) }
    pub fn forward(&self, t: f64) -> f64 { self.inner.forward(t) }
}
```

*After:*
Use `Arc<DiscountCurve>` directly at call-sites, or add `#[derive(Clone)]` to `DiscountCurve` if appropriate. Delete `CurveHandle`.

**Not when:** The wrapper is there specifically for FFI safety (`#[pyclass]` or `#[wasm_bindgen]`) — those are load-bearing for the binding layer, not simplifications to remove.

---

## T12 — Collapse config proliferation

**When:** A capability has multiple similar-looking config structs (`FooConfig`, `FooOptions`, `FooParams`).

**Procedure:**
1. Pick the most complete one.
2. For each field in the others that isn't in the winner, decide: real feature (add it), dead option (drop it), or alias (merge).
3. Migrate call-sites.
4. Delete the losers.

**Watch out for:** `RoundingConfig` vs `RoundingContext` in finstack-quant — they *look* like duplicates, but one is input and the other is output metadata. They are deliberately separate. See `financial-invariants.md`.

---

## T13 — Flatten nesting

**When:** Deeply nested `if let` / `match` that can be linear with early returns or `?`.

*Before:*
```rust
pub fn find_curve(id: &str, market: &Market) -> Option<Curve> {
    if let Some(section) = market.discount_curves.get(id) {
        if let Some(curve) = section.active() {
            if curve.is_valid() {
                return Some(curve.clone());
            }
        }
    }
    None
}
```

*After:*
```rust
pub fn find_curve(id: &str, market: &Market) -> Option<Curve> {
    let section = market.discount_curves.get(id)?;
    let curve = section.active()?;
    curve.is_valid().then(|| curve.clone())
}
```

Pure win: fewer lines, same behavior, easier to read.

---

## T14 — Prefer std over bespoke helpers

**When:** A crate-local helper reinvents something in `std` or `itertools` or a common dependency.

**Examples:**
- Custom `fn partition_by<T, F>(vec: Vec<T>, f: F) -> (Vec<T>, Vec<T>)` when `Iterator::partition` exists.
- Custom `fn group_by_sorted` when `itertools::Itertools::chunk_by` exists.
- Custom `fn zip_longest` when `itertools::EitherOrBoth` exists.

Delete the bespoke helper. Use the standard.

---

## T15 — Consolidate registrations

**When:** A crate has multiple registration points for the same kind of item (checks, scenarios, builders).

**Procedure:**
1. Identify the canonical registration entry point (usually in `mod.rs` or a `register.rs` at the crate root).
2. For each secondary registration point, route through the canonical one.
3. Delete the secondary paths.
4. Update tests that called secondary paths to call the canonical one.

**Finstack Quant-specific:** `statements/src/registry/mod.rs` + `statements/src/registry/dynamic.rs` — typically one of these should be the authority and the other should either be deleted or become a pure consumer.

---

## Tactics NOT to apply

Things that sound like simplifications but aren't:

- **"Introduce a trait to unify X and Y"**: that's abstraction, not simplification. If X and Y are actually the same thing, merge them (T10). If they're different, leave them.
- **"Write a macro to reduce boilerplate"**: macros are a tax on every reader. Only if the boilerplate is *proven* to be repetitive across many sites (5+) and the macro is simple.
- **"Add a builder so the constructor is simpler"**: builders don't simplify — they add a second pathway. Use them only when the struct has 7+ required arguments (AGENTS.md threshold for "too many args").
- **"Generalize T so the caller can pick"**: if the caller would always pick the same thing, don't generalize.
- **"Replace the enum with a trait object"**: enums + match are easier to read and exhaustively checked. Trait objects lose both.

---

## Ordering tactics in a slice

When you apply multiple tactics in one slice:

1. Delete first (T1) — fewer things to worry about in the following steps.
2. Inline/collapse next (T2, T3) — reduces surface area.
3. Merge near-duplicates (T10) — do this after deletion so you don't merge something you could have deleted.
4. Move binding logic to Rust (T7) — always last within a slice, because it may expose further simplifications.

After the slice, re-audit the affected area before planning the next slice. Simplifications compound, and what was Tier 3 before may now be Tier 1.
