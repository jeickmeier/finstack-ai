# Binding-layer patterns

Use this reference when refactoring `finstack-quant-py` code or anything that changes Python-facing API shape.

## Thin-binding rule

The binding crate should not accumulate financial logic or policy decisions. The Python layer should mainly:

- extract Python inputs
- build or unwrap wrapper types
- call Rust core functions
- map core errors into Python exceptions
- register Python modules, docs, and exports

The module docs in `finstack-quant-py/src/lib.rs` already state this boundary explicitly.

## Wrapper pattern

Binding wrappers commonly expose the Rust value as an `inner` field:

```rust
pub struct PyThing {
    pub(crate) inner: Thing,
}
```

When refactoring wrapper constructors, match the local module's established naming instead of inventing a new variant. Some modules use `new`, while the repo guidance prefers `from_inner`; if you touch a wrapper, either keep the local name or normalize the whole local area in one pass.

Good wrapper refactors:

- move shared conversions into a local extraction helper
- keep wrapper construction near the boundary
- move domain decisions into core code before the wrapper is built

## Registration pattern

Python modules are typically assembled with a `register()` function that:

- creates the submodule
- sets `__doc__`
- adds classes and functions
- sets `__all__`
- attaches the submodule to its parent

Keep this pattern stable when splitting modules internally. Internal reorganization is usually cheaper than changing registration shape.

## Error mapping pattern

Prefer the centralized conversion functions in `finstack-quant-py/src/errors.rs`, especially `core_to_py`, rather than ad hoc mapping at each call site.

Good refactor:

```rust
core_operation(params).map_err(core_to_py)
```

Bad refactor:

```rust
core_operation(params).map_err(|e| PyValueError::new_err(e.to_string()))
```

The central mapping preserves the Python exception hierarchy and keeps behavior consistent.

## Export and re-export pattern

Two export surfaces matter:

- the PyO3 module tree in `finstack-quant-py/src/lib.rs`
- Python package re-export files such as `finstack-quant-py/finstack_quant/valuations/__init__.py`

A refactor may be internal in Rust but still require export updates if names or module layout move.

## Binding-specific examples from this repo

- `finstack-quant-py/src/lib.rs` registers core and package-level exports through explicit `register()` calls and `__all__` lists.
- `finstack-quant-py/src/bindings/core/currency.rs` shows the thin wrapper pattern, local extraction helper, and module registration flow.
- `finstack-quant-py/finstack_quant/valuations/__init__.py` shows the Python-side re-export surface that can drift if a Rust module layout changes.

## Common mistakes during refactor

- duplicating a conversion helper in multiple binding modules instead of extracting it locally or moving the rule to core
- changing a Python-visible name in Rust without updating Python package re-exports or `.pyi`
- mapping errors locally in a way that bypasses the established exception hierarchy
- leaving helper logic in bindings that should be shared with WASM
