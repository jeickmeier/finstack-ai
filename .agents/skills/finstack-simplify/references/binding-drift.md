# Binding drift: Rust ↔ PyO3 ↔ wasm-bindgen

The triplet is **load-bearing**. Rust is canonical; Python and WASM must match in names and semantics, per `AGENTS.md`:

> Rust owns continuation, recovery, interaction, lineage, ordering, and error semantics. Python and JavaScript adapters perform coarse conversion and host calls only.

Drift has two flavors:

1. **Structural drift** — names, shapes, or signatures differ.
2. **Logic drift** — binding code contains real logic that should live in Rust.

Both flavors are simplification opportunities. This reference tells you how to detect each and how to fix them without breaking parity.

---

## Map the triplet

For any Rust type or function `Foo` in `crates/<crate>/src/...`:

- **Python binding** lives under `bindings/finstack-ai-python/`.
- **WASM binding** lives under `bindings/finstack-ai-wasm/`.
- **Python type stubs** are at `bindings/finstack-ai-python/python/finstack_ai/*.pyi`.
- **JS facade** is at `bindings/finstack-ai-wasm/js/src/` — hand-written over generated glue.
- **Public items** are checked by `uv run --no-project python scripts/compat/public_items.py --check`.
- **Conformance** is checked by `cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests`.

Before doing any binding-touching work, list these paths for the scope you're auditing. Put the list in the audit report.

---

## Structural drift — detection

### Types

1. Enumerate Rust `pub struct`/`pub enum` in the target module.
2. For each, check whether it has a counterpart:
   - Python: typically a `#[pyclass] pub struct PyFoo { pub(crate) inner: Foo }` or a re-export from `__init__.py`
   - WASM/JS: typically `#[wasm_bindgen] pub struct Foo { inner: RustFoo }` plus a JS facade type
3. Flag each missing binding. Flag each binding without a Rust source. Flag name mismatches (e.g., Rust `Session` but Python `Conversation`).

### Functions

1. Enumerate Rust `pub fn` in the module that are not on a struct (free functions) and public methods.
2. For each, check whether it's exposed to Python and to WASM.
3. Flag asymmetries.

### Fields / accessors

Check that Python and WASM follow the same accessor convention as Rust. A binding that exposes raw fields where Rust uses `get_*` is drifted.

### Stable codes and kinds

Error `code` strings and durable record/event kind names must be identical across bindings. If a binding constructs codes in a different format than Rust, that's drift.

---

## Name collision exceptions

Python and WASM host-language name collisions must be documented explicitly:

- Python must avoid builtins like `type`, `id`, `hash` (these are fine to shadow in Rust, not in Python).

Undocumented deviations ("we renamed `register` to `add` because it's cleaner") are drift to fix.

---

## Logic drift — detection

Binding code should read like this:

```rust
#[pyfunction]
fn start_run(params: StartRunArg) -> PyResult<PyRun> {
    finstack_ai::start_run(params.into_inner()).map_err(core_to_py)
}
```

Three jobs: extract → call → map error. Anything beyond that is logic drift.

**Red flags** in a binding function:

- `if` / `match` beyond trivial input normalization.
- Any arithmetic or policy decision.
- More than one call into a `finstack_*` crate.
- Construction of intermediate Rust types that could be done inside the Rust fn.
- Re-implementing validation that already exists in the Rust function.

When you find these, the refactor is:

1. Move the logic into a new (or existing) Rust function.
2. Reduce the binding back to the three-job shape.
3. Add a matching binding in the _other_ host language if one was missing.

**Do not** just clean up the Python binding and leave the WASM binding still holding logic. Triplets move together.

---

## Public items and conformance

`uv run --no-project python scripts/compat/public_items.py --check` is the source of truth for frozen public names. Treat it like an API contract.

During a refactor:

- If you delete a Rust public symbol, remove or update its public-item entry in the same slice.
- If you rename a Rust public symbol, rename the inventory entry.
- If conformance fixtures fail after your changes, stop. Either your refactor broke an invariant or the fixture is stale — figure out which before "fixing" the test.

```bash
uv run --no-project python scripts/compat/public_items.py --check
cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests
```

If you added a new canonical API, add it to the public-item inventory in the same slice when it is intended to be frozen.

---

## The .pyi stub layer

`bindings/finstack-ai-python/python/finstack_ai/*.pyi` is the IDE-facing surface. If you change binding shapes, regenerate or update the stubs in the same slice. Don't leave `.pyi` lying about types that no longer exist.

---

## Common drift patterns you'll see

### Pattern A — "Rust evolved, bindings didn't"

Rust added a new `Config` field. Python binding still constructs `Config` without it. Python users effectively get a silent default. WASM users too.

**Fix:** Thread the field through both bindings in one slice. Update `.pyi`. Update public items if frozen.

### Pattern B — "Binding evolved, Rust didn't"

Someone wanted a "convenience" Python helper: `from_yaml_file(path)`. They added it as a `#[pyfunction]` in the binding, reading the file and parsing YAML and calling the Rust constructor. Rust has no equivalent.

**Fix:** Move the helper to Rust (`pub fn from_yaml_file(path: &Path) -> Result<Foo, Error>`). Binding becomes a one-line call. Add the matching WASM binding.

### Pattern C — "Rust deleted something, binding kept a stub"

A Rust function was removed or renamed. The binding still has a function with the old name, now implemented inline or calling something unrelated.

**Fix:** Delete the binding stub. Update public items. Update `.pyi`. The user of the binding should update; that's what breaking changes are for.

### Pattern D — "Both sides evolved independently"

The worst case. Rust has `start_run(&spec)`, Python has `start_run(spec, timeout=None)`, WASM has `startRun(spec)`. Each has a different calling convention and the Python one accepts an extra arg that Rust doesn't.

**Fix:** Converge on the Rust signature. Update both bindings. Delete the extra Python arg (or add it to Rust if it's real). This is a medium-risk refactor and should go in its own slice with explicit user sign-off.

---

## Procedure for a binding-drift slice

1. **Read** the Rust source-of-truth for the scope. Write down its public shape.
2. **Read** both binding directories. Diff against the Rust shape.
3. **Categorize** each difference as: structural drift, logic drift, intentional (name collision), or unknown.
4. **Plan** the fix as part of the larger refactor slice — binding changes and their Rust sources go in the same commit.
5. **Implement** Rust-first, then Python binding, then WASM binding, then `.pyi`, then public items.
6. **Verify** in order: `mise run check-all && mise run test-all` → `mise run build-wasm -- release` → `mise run check-wasm` → `uv run --no-project python scripts/compat/public_items.py --check` → `cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests`.

**Do not batch multiple binding-drift slices into one commit.** Each drift repair is a discrete before/after; keeping them separate makes review tractable and rollback cheap.

---

## Sanity check before you call a binding slice "done"

- [ ] Rust public surface matches Python binding symbol-for-symbol (modulo Python naming and snake_case).
- [ ] Rust public surface matches WASM binding symbol-for-symbol (modulo documented JS naming conventions).
- [ ] No binding function exceeds ~20 lines unless it's doing a legitimate type-conversion batch.
- [ ] No binding function contains arithmetic or non-trivial control flow.
- [ ] Public-item inventory is in sync; `uv run --no-project python scripts/compat/public_items.py --check` passes.
- [ ] `.pyi` stubs type-check cleanly.
- [ ] JS facade exposes the new surface; no raw generated-glue leaks.
- [ ] `__all__` or package exports are set; no dynamic export discovery.

If any of the above are false, the slice is not done — regardless of what the test runner says.
