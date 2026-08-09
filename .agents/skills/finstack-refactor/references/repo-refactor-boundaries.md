# Repo refactor boundaries

Use this reference when deciding where code should live and what surfaces must remain aligned.

## Core ownership

Business logic, valuation rules, validation rules, pricing behavior, and domain invariants belong in the Rust core crates:

- `finstack-quant/core`
- `finstack-quant/valuations`
- `finstack-quant/statements`
- `finstack-quant/scenarios`
- `finstack-quant/portfolio`

Move logic into core when any of these are true:

- the rule should behave the same in Python and WASM
- the code makes a domain decision, not a binding or formatting decision
- the code is easier to test or reuse at the Rust layer
- the code is becoming duplicated across bindings or entrypoints

## Binding ownership

`finstack-quant-py` and `finstack-quant-wasm` should stay thin. They should primarily do:

- type conversion
- wrapper construction
- ergonomic adapters for host language use
- module registration and exports
- error translation

Keep code in the binding layer when it is truly binding-specific:

- Python text signatures, docstrings, or `__all__`
- flexible Python argument extraction
- Python module re-export wiring
- WASM-friendly serialization and export shape

Do not leave domain logic in bindings just because the binding currently owns the entrypoint.

## Public-surface boundaries

Treat these as public surfaces even if the Rust refactor is internal:

- Python extension exports in `finstack-quant-py/src/lib.rs`
- per-module `register()` wiring in `finstack-quant-py/src/**`
- Python package re-exports such as `finstack-quant-py/finstack_quant/valuations/__init__.py`
- manually maintained `.pyi` stubs under `finstack-quant-py/finstack_quant/`
- WASM bindings if the API shape is shared
- parity tests under `finstack-quant-py/tests/parity`

## Good boundary moves

- Move pricing or validation logic from a Python wrapper into a Rust core function.
- Extract data-shaping helpers inside a binding module while keeping the binding API stable.
- Split a large Rust module internally while preserving the existing exported function or type names.
- Introduce a params struct in core when it simplifies call sites across bindings.

## Bad boundary moves

- Add Python-only financial logic that cannot be shared with WASM.
- Hide a domain behavior change behind a refactor label.
- Move logic from core into bindings just to avoid touching Rust.
- Change public names or module layout casually without following the sync surfaces.

## Boundary check before editing

Ask these questions:

1. Is this rule host-language-specific or domain-specific?
2. Will Python and WASM need the same behavior?
3. Am I changing only structure, or also semantics?
4. Which re-export or stub surfaces mirror this code?
5. Can I keep the public surface stable while simplifying the internals?
