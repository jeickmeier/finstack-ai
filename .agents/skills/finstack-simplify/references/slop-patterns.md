# Slop patterns: the catalogue

Every category below is something this skill hunts for during Phase 1 (Audit). Each entry has: **what it looks like**, **how to detect it**, a **repo-specific example shape**, and **how to fix**. "Fix" always defaults to *delete*.

If you find a pattern that isn't in this catalogue, add it here in your audit report under "New pattern observed" with the same shape.

---

## 1. Parallel APIs (two+ ways to do the same thing)

**Looks like:** `compute_x()`, `compute_x_with_config()`, `compute_x_ex()`, `compute_x_v2()`, `compute_x_advanced()`, `compute_x_fast()`. Or: `Foo::new`, `Foo::from_parts`, `Foo::build`, `Foo::builder().build()` — three pathways to the same struct.

**Detect:**
- Grep for `_v2`, `_ex`, `_new`, `_advanced`, `_fast`, `_simple`, `_with_` as suffixes.
- Look for multiple `impl Foo { pub fn ... }` blocks with similar signatures and overlapping bodies.
- Any time you see a `match version { .. }` or `if legacy { .. }` inside a public function, suspect a parallel universe.

**Repo-specific shape:**
- Agent builders: `Agent::new` + `Agent::from_spec` + `build_agent()` free function — three entry points doing substantively the same job.
- Session construction: `Session::new` + `Session::from_handle` + `open_session()`.
- Multiple `start_run_*` functions that differ only in which subset of inputs they accept.

**Fix:** Pick the canonical entry point (usually the one with the richest signature + `Result` return). Collapse everything else into it. Convert the others to private helpers or delete outright. Update call-sites in the binding layer (`bindings/finstack-ai-python/`, `bindings/finstack-ai-wasm/`) in the **same slice** — don't leave orphans.

**Bias:** prefer the one that already takes a strongly-typed context object. Infallible shortcuts that hide a `.unwrap()` are traps; delete them.

---

## 2. Wrapper-only functions (thin forwarders)

**Looks like:**
```rust
pub fn start_run_default(spec: &AgentSpec) -> Result<Run, Error> {
    start_run_internal(spec, StartRunOptions::default())
}
```
…where `start_run_internal` is also public. Or: a method that calls another method and does nothing else. Or: a builder that only sets two fields and calls `.build()`.

**Detect:**
- One-line function bodies that delegate to another function in the same module.
- Functions whose body is `return other_fn(args)` with no transformation.
- Wrappers that "just provide a default" when the default is already `Default::default()` on the param type.

**Repo-specific shape:**
- `runtime::start_run_default(spec)` that just forwards to `runtime::start_run(spec, StartRunOptions::default())`.
- `sdk::open_session(id)` that forwards to `sdk::open_session_with_options(id, SessionOptions::default())`.

**Fix:** If the wrapper doesn't add semantic value (naming, type conversion, error translation, defaulting a non-trivial value), **delete it**. Inline into call-sites if needed, or make the inner function's default more convenient.

**Rule of survival:** a wrapper may stay if and only if you can write one sentence explaining the semantic value it adds that is NOT "renaming" or "reordering" or "providing an obvious default." Put that sentence in the refactor-diff note.

---

## 3. `try_*` shadow universe

**Looks like:** Every function has a `fn x() -> T` and a `fn try_x() -> Result<T, Error>`, and the non-`try_` version is just `try_x().unwrap()` or `try_x().expect(...)`.

**Detect:**
- Pairs of functions `x` and `try_x` in the same module.
- `fn x(...) -> T { Self::try_x(...).expect(...) }`.

**Repo-specific shape:** Constructors on primitives. `ErrorCode::new` panicking and `ErrorCode::try_new` returning `Result`, with `new` implemented via `try_new().unwrap()`. Both are public.

**Fix:** **Collapse to one.** The binding crate has `#![deny(clippy::unwrap_used)]` and `#![deny(clippy::panic)]` — the panicking sibling is already unusable from bindings. Delete it. Keep only the `Result`-returning one. If the user really wants an infallible version, give it a distinct name that reflects a genuinely infallible input type (e.g., a validated newtype), not a `try_`/`_` pair.

**Exception:** It's fine to have `new(ValidatedInput) -> Self` (infallible because the input is pre-validated at the type level) alongside `try_new(&str) -> Result<Self, ParseError>` (fallible because the input is loose). That's not a shadow universe — it's two constructors with meaningfully different input types.

---

## 4. Duplicate implementations of the same computation

**Looks like:** Two functions in two modules that implement the same algorithm with minor variations — usually because someone copied and tweaked instead of refactoring.

**Detect:**
- Grep for distinctive constants or formula fragments with similar neighborhood.
- Two functions with the same output on test vectors but different code paths.
- Structural review: list all `apply_record`, `start_run`, `map_error` helpers in the workspace. If there's more than one of any of them, suspect duplication.

**Repo-specific shape:**
- `kernel::apply_record()` and `runtime::apply_record()` with subtly different handling of empty batches.
- Two error mappers in Python and WASM that reconstruct messages differently.
- Duplicate session-open helpers in SDK and bindings.

**Fix:** Pick the canonical implementation (usually the one with better tests and clearer docstring). Move it to the most natural crate (usually kernel or runtime). Delete the others. Update call-sites. Run the full conformance tests — if they diverge, investigate *which* one was right; don't paper over the difference.

**Caution:** sometimes what looks like duplication is actually a deliberate differentiation (e.g., native vs WASM host constraints). Confirm by reading docstrings and tests before collapsing. Cite the finding in the audit report.

---

## 5. Single-impl traits

**Looks like:**
```rust
pub trait SessionBuilder { fn build(&self) -> Session; }
impl SessionBuilder for LocalSessionBuilder { ... }
// ... and nothing else in the workspace implements SessionBuilder.
```

**Detect:**
- `grep -r "impl.*for" | awk` to count impls per trait. Any trait with exactly one impl is suspect.
- Traits defined in a module that also contains the only impl.

**Repo-specific shape:** "Pluggable" abstractions added speculatively during vibe-coding. `trait HostAdapter` with only `NativeHost` implementing it. `trait JournalWriter` with only `MemoryJournal` implementing it.

**Fix:** Collapse the trait away. Use the concrete type directly. If the user *plans* to add a second impl, defer the trait until the second impl exists — speculative traits are a tax on every reader until that day comes.

**Exception:** a trait is justified by *one* impl if it's used for mocking in tests AND the test-mock saves real effort. If the "mock" is trivial, inline the concrete type and test against it directly.

---

## 6. Single-instantiation generics

**Looks like:**
```rust
pub fn evaluate<T: RecordLike>(input: &[T]) -> T { ... }
// ...but `evaluate` is only ever called with T = Record.
```

**Detect:**
- Grep for callers of a generic function. If every call site uses the same concrete type, it's single-instantiation.
- PhantomData fields in structs, especially if the type parameter is never used in a meaningful way.

**Repo-specific shape:** `fn dispatch<P: Port>(p: P)` where only `NativePort` ever gets passed. `struct Cache<K, V>` where `K = String, V = RecordId` everywhere.

**Fix:** Delete the type parameter. Use the concrete type. Keep the generic only when you have two concrete instantiations *today* or when the caller (downstream Rust user, Python, or WASM) genuinely needs to pick. Note: PyO3 and wasm-bindgen can't expose generics anyway — any generic in code that feeds a binding is a strong delete signal.

---

## 7. Half-migrated APIs

**Looks like:** A module with both an "old" and a "new" way to do something because a refactor stalled. Comments like `// TODO: migrate to new API`, `// deprecated, use X instead`, or dead code branches behind feature flags that default off.

**Detect:**
- Grep for `TODO`, `FIXME`, `deprecated`, `legacy`, `old_`, `v1`.
- Look for two modules with similar names (`checks/` and `checks_v2/`, or `runner.rs` and `suite.rs`).
- `git log -- path/` showing a rename-in-progress.

**Repo-specific shape:**
- `kernel/src/reducer/legacy.rs` alongside `reducer/apply/shapes.rs`.
- `registry/dynamic.rs` alongside `registry/mod.rs` with overlapping responsibilities.
- Anywhere a module imports from both a new and old path for the same capability.

**Fix:** Pick the side that's "newer and working." Migrate remaining call-sites. Delete the legacy side in its entirety in the same slice. Update bindings. Update parity contract if needed.

**Half-migrated APIs are the highest-value target** of this skill. They leave the codebase with two mental models, and every reader pays the cost.

---

## 8. Registry / builder / plugin sprawl

**Looks like:** Multiple registries, each with its own `register_*` convention, plus multiple builder types that all end with `.build()` but take different field-setter conventions.

**Detect:**
- Grep for `Registry`, `registry`, `register_`, `Builder`, `builder`.
- Look for `pub fn register_root(...)` in more than one place per crate.
- Builders that have more than one `pub fn build(...)` variant.

**Repo-specific shape:**
- `runtime/src/registry/mod.rs` + `runtime/src/registry/dynamic.rs` + an ad-hoc `HashMap<String, ...>` elsewhere.
- `AgentBuilder::build()` that coexists with `AgentBuilder::finalize()` and `AgentBuilder::into_agent()`.
- Registration via `sdk.register(...)` AND `runtime::register_builtin(...)` AND a proc-macro.

**Fix:** One registry per thing. One builder method name (`.build()` is the Rust convention; pick it and stick). Route all registration through a single point. Bindings should only expose the single point.

**Key heuristic:** if a crate has more than one `register_*` function at the module-root level, you have sprawl.

---

## 9. Error enum bloat

**Looks like:** An `Error` enum with 20+ variants, several of which are never constructed, several of which differ only in message text, and several of which are constructed from the same source error with slightly different context.

**Detect:**
- Grep for each variant's name. If a variant is only referenced in its own `impl Display`, it's probably unused.
- Variants with identical field shapes and similar names (e.g., `InvalidCurve`, `BadCurve`, `MalformedCurve`).
- Variants that are only used once and could be replaced with a `source()` chain.

**Repo-specific shape:** Per-crate `Error` enums in kernel, runtime, and protocol that each have their own `ParseError`, `ValidationError`, `InvalidInput`, etc., with no shared structure. Bindings then map them all to the same Python/JS error anyway.

**Fix:**
- Delete unused variants.
- Merge variants that differ only in message text into one variant with a context field.
- Consolidate under a single crate-level `Error` with well-chosen variants; use `thiserror::#[source]` for chaining instead of new variants per source type.
- Remember: bindings flatten all of these to strings eventually. Error-enum refinement past the point of binding flattening is pure tax.

---

## 10. Prelude and re-export bloat

**Looks like:** `prelude.rs` re-exporting 80 items, many of which are internal plumbing types that users never need. Or multiple `pub use crate::*` statements that fight each other.

**Detect:**
- Count items in each crate's `prelude` / `lib.rs` `pub use`.
- Look for `pub use foo::*` patterns (glob re-exports).
- Any re-export of a `*Builder` intermediate state (e.g., re-exporting `AgentBuilderStage1`) is a smell.

**Repo-specific shape:** A crate prelude exporting internal apply types used by no one outside the crate. SDK re-exporting an entire dependency's API.

**Fix:** Reduce the prelude to the types downstream users actually construct or match on. Remove internal-only re-exports. Cross-check by grepping every item in the prelude — if it's never imported outside the crate, demote it.

---

## 11. Dead code

**Looks like:** Unused imports, unreachable match arms, commented-out blocks, functions with no call-sites, modules that are `pub mod` but never used.

**Detect:**
- `cargo +nightly rustc -- -W dead_code` (nightly gives more).
- `cargo-udeps` or `cargo-machete` for unused dependencies.
- Grep for function names and see if there are any non-definition hits.

**Repo-specific shape:** Test utility functions left in `src/` instead of `tests/` or `mod tests`. Old host types never wired into the builder.

**Fix:** Delete. If it's "might be useful someday", it goes; `git` remembers.

---

## 12. Over-exposed internals

**Looks like:** `pub struct` fields that should be `pub(crate)` or private. `pub fn` helpers that callers never touch. `pub mod internals` with the entire plumbing of the crate.

**Detect:**
- Any `pub struct Foo { pub x: ..., pub y: ..., pub z: ... }` that also has a builder. The fields probably shouldn't both be `pub` AND have a builder; pick one.
- Grep for each `pub fn` to see if any caller outside the defining crate uses it.

**Repo-specific shape:** Wrapper types in bindings (`pub(crate) inner: RustType`) that then expose `pub fn inner()` accessors. Internal error types re-exported at the crate root.

**Fix:** Tighten visibility to the minimum necessary. Public surface area is a contract; smaller contract = less to break.

---

## 13. Config proliferation

**Looks like:** Three config structs: `FooConfig`, `FooOptions`, `FooSettings` — each used by different entry points of the same capability.

**Detect:**
- Grep for `Config`, `Options`, `Settings`, `Params` suffixes on structs in the same module.
- Multiple `Default` impls that produce subtly different defaults.

**Repo-specific shape:** `RunConfig` vs `RunOptions` in runtime. `AgentSpec` vs `AgentSettings` for the same capability.

**Fix:** One config type per capability. Pick the most complete and delete the others. If sub-configs are needed for different phases, they should be composable (struct-of-substructs) not parallel.

**Exception:** Input config and output metadata that look similar can be deliberately separate. Don't collapse them without checking with the user. See `behavioral-invariants.md`.

---

## 14. Vibe-coded speculative abstractions

**Looks like:** Code written in anticipation of requirements that never materialized. Factories that produce one thing. Event buses with one publisher and one subscriber. Abstract base classes whose only purpose is to be inherited by a single concrete class.

**Detect:**
- Look for types named `*Factory`, `*Manager`, `*Coordinator`, `*Orchestrator`, `*Strategy`.
- Any abstraction added for "future flexibility" that has no current second user.
- Documentation that says "extensible" without showing what currently extends it.

**Repo-specific shape:** `HostFactory` that returns `NativeHost` and nothing else. `JournalStrategy` with one strategy implemented.

**Fix:** Inline the factory. Remove the abstract base. Kill the event bus. Re-introduce the abstraction the day a second user appears; until then, every reader is paying the cost.

---

## 15. Binding logic leakage

**Looks like:** A Python or WASM binding that does more than type conversion — e.g., validates input, computes something, or calls multiple Rust functions in sequence before returning.

**Detect:**
- In `bindings/finstack-ai-python/**/*.rs`, look for functions longer than ~20 lines, or with any arithmetic, or with any non-trivial control flow.
- In `bindings/finstack-ai-wasm/**/*.rs`, same test.
- Any binding that calls more than one Rust function is suspect.

**Repo-specific shape:** A Python binding that takes `spec: &PyAny`, extracts fields, constructs a Rust `AgentSpec`, *then* applies a policy inline — instead of calling a single Rust function.

**Fix:** Move the logic into a Rust function. Reduce the binding to: extract params → call the Rust fn → wrap the result → map errors. See `references/binding-drift.md` for the full procedure.

---

## Meta-pattern: "several small slops that add up"

Some of the worst offenders are combinations: a single-impl trait wraps a parallel-API wrapper that has a `try_*` shadow and lives in a half-migrated module. These don't each look terrible alone, but together they represent ~10x more code than one canonical function would.

When you find these, name them as a **slop cluster** in your audit report and collapse them in one coordinated slice — piecemeal fixes tend to leave half a cluster standing.

---

## How to triage findings in the audit

Rank each finding on two axes:

- **Impact** (how much it simplifies the codebase / how much reader confusion it removes): High / Med / Low.
- **Risk** (how likely a refactor is to break something): High / Med / Low.

Present findings sorted by **Impact desc, Risk asc** — i.e., lead with the highest-value, lowest-risk wins. These are the slices the user will almost always pick first.

"High impact, High risk" items go to the bottom of the audit under "Hazardous but valuable" — the user will decide whether to schedule those separately.
