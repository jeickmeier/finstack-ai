# Finstack Binding Parity Surfaces

Use this reference when a Rust public API is intended to appear in Python or WASM.

## Surfaces To Check

| Surface | Files |
| --- | --- |
| Canonical Rust API | `crates/*/src/**/*.rs` |
| Python bindings | `bindings/finstack-ai-python/**` |
| Python stubs | `bindings/finstack-ai-python/python/finstack_ai/**/*.pyi` |
| Python exports | `bindings/finstack-ai-python/python/finstack_ai/__init__.py` |
| WASM bindings | `bindings/finstack-ai-wasm/**` |
| JS facade | `bindings/finstack-ai-wasm/js/src/**` |
| Public items | `mise run check-public-items` |
| Conformance | `mise run conformance` |

## Required Invariants

- Rust names are canonical. Python should preserve `snake_case`; WASM should expose `camelCase` via `js_name`.
- Bindings use `pub(crate) inner: RustType` plus `from_inner()` for wrapper construction when that pattern is already established.
- Error mapping stays centralized. Stable error `code` strings and record/event kind names stay identical across bindings.
- Binding code does not implement domain decisions, validation, or lifecycle policy that belongs in Rust.
- `.pyi`, `__all__`, module registration, JS facade exports, and public-item inventory move with public API changes.

## Verification Defaults

Use the narrowest meaningful checks first:

- Python binding touched: rebuild the editable package, then targeted Python tests.
- WASM binding touched: `mise run generate-wasm` / `mise run check-wasm`, then targeted WASM tests if present.
- Public API renamed or moved: search stubs, exports, examples, and run `mise run check-public-items`.
- Rust behavior changed: run targeted Rust tests before binding conformance tests.
