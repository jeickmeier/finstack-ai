# Binding drift: Rust ↔ PyO3 ↔ wasm-bindgen

The triplet is **load-bearing**. Rust is canonical; Python and WASM must match in names and semantics, per `AGENTS.md`:

> Rust is the canonical API design. Type and function names in Python/WASM must match Rust exactly (exceptions only for documented host-language collisions).
> All logic stays in Rust crates; bindings do only type conversion, wrapper construction, error mapping.

Drift has two flavors:

1. **Structural drift** — names, shapes, or signatures differ.
2. **Logic drift** — binding code contains real logic that should live in Rust.

Both flavors are simplification opportunities. This reference tells you how to detect each and how to fix them without breaking parity.

---

## Map the triplet

For any Rust type or function `Foo` in `finstack-quant/<crate>/src/...`:

- **Python binding** lives under `finstack-quant-py/src/bindings/<crate>/` (mirrors Rust crate tree exactly).
- **WASM binding** lives under `finstack-quant-wasm/src/api/<crate_ns>/` where `<crate_ns>` is e.g. `core_ns`, `analytics`, `margin`, etc.
- **Python type stubs** are at `finstack-quant-py/finstack_quant/*.pyi` — derived from the binding code and the parity contract.
- **JS facade** is at `finstack-quant-wasm/index.js` — hand-written, not auto-generated from pkg/.
- **Parity contract** is at `finstack-quant-py/parity_contract.toml`.
- **Parity tests** are in `finstack-quant-py/tests/parity/`.

Before doing any binding-touching work, list these paths for the scope you're auditing. Put the list in the audit report.

---

## Structural drift — detection

### Types

1. Enumerate Rust `pub struct`/`pub enum` in the target module.
2. For each, check whether it has a counterpart:
   - `finstack-quant-py/src/bindings/<crate>/<module>.rs` — typically a `#[pyclass] pub struct PyFoo { pub(crate) inner: Foo }`
   - `finstack-quant-wasm/src/api/<crate_ns>/<module>.rs` — typically `#[wasm_bindgen] pub struct Foo { inner: RustFoo }` or similar
3. Flag each missing binding. Flag each binding without a Rust source. Flag name mismatches (e.g., Rust `VolSurface` but Python `VolatilitySurface`).

### Functions

1. Enumerate Rust `pub fn` in the module that are not on a struct (free functions) and public methods.
2. For each, check whether it's exposed to Python (as a free function in the submodule, or as a method on the binding struct) and to WASM.
3. Flag asymmetries.

### Fields / accessors

Rust convention uses `get_*` for accessors (per `AGENTS.md`). Check that Python and WASM follow the same convention — `get_discount()`, not `.discount` on the binding. A binding that exposes raw fields where Rust uses `get_*` is drifted.

### Metric keys

Per `AGENTS.md`, metric keys are fully qualified: `bucketed_dv01::USD-OIS::10y`, `cs01::ACME-HZD`, `pv01::usd_ois`. If a binding constructs keys in a different format than Rust, that's drift.

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
fn sharpe(returns: Vec<f64>, rf: f64) -> PyResult<f64> {
    finstack_quant_analytics::sharpe(&returns, rf).map_err(core_to_py)
}
```

Three jobs: extract → call → map error. Anything beyond that is logic drift.

**Red flags** in a binding function:

- `if` / `match` beyond trivial input normalization.
- Any arithmetic.
- More than one call into a `finstack_*` crate.
- Construction of intermediate Rust types that could be done inside the Rust fn.
- Re-implementing validation that already exists in the Rust function.

When you find these, the refactor is:

1. Move the logic into a new (or existing) Rust function.
2. Reduce the binding back to the three-job shape.
3. Add a matching binding in the _other_ host language if one was missing.

**Do not** just clean up the Python binding and leave the WASM binding still holding logic. Triplets move together.

---

## Parity contract and parity tests

`finstack-quant-py/parity_contract.toml` is the source of truth for what must be equal across Rust / Python / WASM. Treat it like an API contract file.

During a refactor:

- If you delete a Rust public symbol, remove its parity entry.
- If you rename a Rust public symbol, rename the parity entry.
- If the parity test suite fails after your changes, stop. Either your refactor broke an invariant or the parity entry is stale — figure out which before "fixing" the test.

Run the parity tests with:

```bash
uv run pytest finstack-quant-py/tests/parity -x
```

If you added a new canonical API, add it to the parity contract in the same slice.

---

## The .pyi stub layer

`finstack-quant-py/finstack_quant/*.pyi` is derived from the binding code and the parity contract. If you change binding shapes, regenerate or update the stubs in the same slice. Don't leave `.pyi` lying about types that no longer exist — type-checker consumers will catch it later and you'll own the bug.

---

## Common drift patterns you'll see

### Pattern A — "Rust evolved, bindings didn't"

Rust added a new `Config` field. Python binding still constructs `Config` without it. Python users effectively get a silent default. WASM users too.

**Fix:** Thread the field through both bindings in one slice. Update `.pyi`. Update parity.

### Pattern B — "Binding evolved, Rust didn't"

Someone wanted a "convenience" Python helper: `from_yaml_file(path)`. They added it as a `#[pyfunction]` in the binding, reading the file and parsing YAML and calling the Rust constructor. Rust has no equivalent.

**Fix:** Move the helper to Rust (`pub fn from_yaml_file(path: &Path) -> Result<Foo, Error>`). Binding becomes a one-line call. Add the matching WASM binding.

### Pattern C — "Rust deleted something, binding kept a stub"

A Rust function was removed or renamed. The binding still has a function with the old name, now implemented inline or calling something unrelated.

**Fix:** Delete the binding stub. Update parity. Update `.pyi`. The user of the binding should update; that's what breaking changes are for.

### Pattern D — "Both sides evolved independently"

The worst case. Rust has `compute_cs01(&bond, &curve)`, Python has `compute_cs01(bond, curve, hazard_curve=None)`, WASM has `cs01(bond, curve)`. Each has a different calling convention and the Python one accepts an extra arg that Rust doesn't.

**Fix:** Converge on the Rust signature. Update both bindings. Delete the extra Python arg (or add it to Rust if it's real). Update parity. This is a medium-risk refactor and should go in its own slice with explicit user sign-off.

---

## Procedure for a binding-drift slice

1. **Read** the Rust source-of-truth for the scope. Write down its public shape.
2. **Read** both binding directories. Diff against the Rust shape.
3. **Categorize** each difference as: structural drift, logic drift, intentional (name collision), or unknown.
4. **Plan** the fix as part of the larger refactor slice — binding changes and their Rust sources go in the same commit.
5. **Implement** Rust-first, then Python binding, then WASM binding, then `.pyi`, then parity contract.
6. **Verify** in order: `mise run rust-lint && mise run rust-test` → `mise run python-build` → `mise run python-lint && mise run python-test` → `mise run wasm-build` → `mise run wasm-lint && mise run wasm-test` → run parity tests.

**Do not batch multiple binding-drift slices into one commit.** Each drift repair is a discrete before/after; keeping them separate makes review tractable and rollback cheap.

---

## Sanity check before you call a binding slice "done"

- [ ] Rust public surface matches Python binding symbol-for-symbol (modulo Python naming like `get_*` and snake_case).
- [ ] Rust public surface matches WASM binding symbol-for-symbol (modulo documented JS naming conventions).
- [ ] No binding function exceeds ~20 lines unless it's doing a legitimate type-conversion batch.
- [ ] No binding function contains arithmetic or non-trivial control flow.
- [ ] `parity_contract.toml` is in sync; `mise run python-test` passes; parity tests pass.
- [ ] `.pyi` stubs type-check cleanly.
- [ ] `index.js` facade exposes the new surface; no raw pkg/ leaks.
- [ ] `__all__` is set in every Python submodule `register()`; no dynamic export discovery.

If any of the above are false, the slice is not done — regardless of what the test runner says.
