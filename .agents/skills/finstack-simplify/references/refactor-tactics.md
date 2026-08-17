# Refactor tactics: the concrete moves

Each tactic has: **when to use**, **when NOT to use**, and a **before/after** in finstack idiom.

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
// in runtime/src/start.rs
pub fn start_run(spec: &AgentSpec) -> Result<Run, Error> {
    start_run_impl(spec)
}

fn start_run_impl(spec: &AgentSpec) -> Result<Run, Error> {
    start_run_core(spec, StartRunOptions::default())
}

fn start_run_core(spec: &AgentSpec, options: StartRunOptions) -> Result<Run, Error> {
    // real work
}
```

*After:*
```rust
pub fn start_run(spec: &AgentSpec, options: StartRunOptions) -> Result<Run, Error> {
    // real work inlined
}
```

If call-sites always pass the default options, keep the signature but document the default expectation; don't hide it behind a wrapper.

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

**Not when:** Each constructor has a genuinely different input type and the difference encodes a precondition (e.g., `ErrorCode::from_str(&str)` vs `ErrorCode::from_validated(ValidatedCode)` — the latter can't fail because the input is pre-validated).

**Example:**

*Before:*
```rust
impl Agent {
    pub fn new(spec: AgentSpec) -> Self { /* panics on bad input */ }
    pub fn try_new(spec: AgentSpec) -> Result<Self, Error> { /* Result version */ }
    pub fn from_spec(spec: &AgentSpec) -> Self { /* calls new() */ }
    pub fn build(builder: AgentBuilder) -> Self { /* calls new() */ }
}
```

*After:*
```rust
impl Agent {
    pub fn new(spec: AgentSpec) -> Result<Self, Error> {
        // Validate input, construct.
    }
}

impl TryFrom<AgentSpec> for Agent {
    type Error = Error;
    fn try_from(spec: AgentSpec) -> Result<Self, Self::Error> {
        Self::new(spec)
    }
}
```

- One `new`, returns `Result`. The panicking variant is gone (per clippy rules in bindings, it's unusable anyway).
- `From`/`TryFrom` impls for conversion sources.
- `AgentBuilder` becomes an internal helper that ultimately calls `Agent::new`.

---

## T5 — Replace single-instantiation generic with concrete

**When:** A generic function or struct is only ever instantiated with one concrete type in the workspace (and is not exposed as a library extension point).

**Not when:** The generic is used at a binding boundary or explicitly documented as a library extension point.

**Example:**

*Before:*
```rust
pub fn apply_record<T: RecordLike>(ctx: &Context<T>, record: T) -> T { /* ... */ }
// Only ever called with T = Record.
```

*After:*
```rust
pub fn apply_record(ctx: &Context, record: Record) -> Record { /* ... */ }
```

Delete the `Numeric` trait if nothing else uses it. Update bindings — generics can't be exposed through PyO3 or wasm-bindgen anyway, so this usually *improves* the binding layer too.

---

## T6 — Collapse try_ / non-try pairs

**When:** A type has both `x()` and `try_x()`, where `x()` is just `try_x().expect(...)`.

**Example:**

*Before:*
```rust
impl ErrorCode {
    pub fn new(code: &str) -> Self { Self::try_new(code).expect("bad code") }
    pub fn try_new(code: &str) -> Result<Self, ErrorCodeError> { /* ... */ }
}
```

*After:*
```rust
impl ErrorCode {
    pub fn new(code: &str) -> Result<Self, ErrorCodeError> { /* ... */ }
}

// If the user wants an infallible construction from a validated source:
impl From<KnownErrorCode> for ErrorCode { /* infallible by type */ }
```

Use distinct input types, not distinct function names, to express the precondition difference.

---

## T7 — Move binding logic to Rust

**When:** A Python or WASM binding function contains logic, arithmetic, or multiple Rust calls.

**Example:**

*Before (Python binding):*
```rust
#[pyfunction]
fn start_run_from_dict(spec: &PyAny, timeout_ms: u64) -> PyResult<PyRun> {
    let agent_id: String = spec.get_item("agent_id")?.extract()?;
    if agent_id.is_empty() { return Err(py_err("missing agent_id")); }
    let timeout = Duration::from_millis(timeout_ms);
    let rust_spec = AgentSpec::parse(&agent_id)?;
    finstack_ai::start_run(&rust_spec, timeout).map(PyRun::from_inner).map_err(core_to_py)
}
```

*After (Rust canonical):*
```rust
// in crates/finstack-ai/src/run.rs
pub fn start_run(spec: &AgentSpec, options: StartRunOptions) -> Result<Run, Error> {
    if spec.agent_id().is_empty() { return Err(Error::missing_agent()); }
    // ...canonical start...
}
```

*After (Python binding):*
```rust
#[pyfunction]
fn start_run(spec: PyAgentSpec, options: PyStartRunOptions) -> PyResult<PyRun> {
    finstack_ai::start_run(&spec.inner, options.into_inner())
        .map(PyRun::from_inner)
        .map_err(core_to_py)
}
```

Same refactor applied to WASM binding. `.pyi` updated. Public-item inventory updated.

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
pub struct AgentHandle {
    inner: Arc<Agent>,
}
impl AgentHandle {
    pub fn new(agent: Agent) -> Self { Self { inner: Arc::new(agent) } }
    pub fn id(&self) -> AgentId { self.inner.id() }
    pub fn spec(&self) -> &AgentSpec { self.inner.spec() }
}
```

*After:*
Use `Arc<Agent>` directly at call-sites, or add `#[derive(Clone)]` to `Agent` if appropriate. Delete `AgentHandle`.

**Not when:** The wrapper is there specifically for FFI safety (`#[pyclass]` or `#[wasm_bindgen]`) — those are load-bearing for the binding layer, not simplifications to remove.

---

## T12 — Collapse config proliferation

**When:** A capability has multiple similar-looking config structs (`FooConfig`, `FooOptions`, `FooParams`).

**Procedure:**
1. Pick the most complete one.
2. For each field in the others that isn't in the winner, decide: real feature (add it), dead option (drop it), or alias (merge).
3. Migrate call-sites.
4. Delete the losers.

**Watch out for:** input config vs output metadata that *look* like duplicates. They can be deliberately separate. See `behavioral-invariants.md`.

---

## T13 — Flatten nesting

**When:** Deeply nested `if let` / `match` that can be linear with early returns or `?`.

*Before:*
```rust
pub fn find_session(id: &str, store: &SessionStore) -> Option<Session> {
    if let Some(section) = store.by_id.get(id) {
        if let Some(session) = section.active() {
            if session.is_open() {
                return Some(session.clone());
            }
        }
    }
    None
}
```

*After:*
```rust
pub fn find_session(id: &str, store: &SessionStore) -> Option<Session> {
    let section = store.by_id.get(id)?;
    let session = section.active()?;
    session.is_open().then(|| session.clone())
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
