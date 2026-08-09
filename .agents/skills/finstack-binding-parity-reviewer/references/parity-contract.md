# Finstack Quant Binding Parity Contract

Use this reference when a Rust public API is intended to appear in Python or WASM.

## Surfaces To Check

| Surface | Files |
| --- | --- |
| Canonical Rust API | `finstack-quant/*/src/**/*.rs` |
| Python bindings | `finstack-quant-py/src/bindings/**` |
| Python stubs | `finstack-quant-py/finstack_quant/**/*.pyi` |
| Python exports | `finstack-quant-py/finstack_quant/**/__init__.py`, PyO3 `register()` functions |
| WASM bindings | `finstack-quant-wasm/src/api/**` |
| JS facade | `finstack-quant-wasm/index.js`, `finstack-quant-wasm/exports/**` |
| Parity contract | `finstack-quant-py/parity_contract.toml` |
| Parity tests | `finstack-quant-py/tests/parity/**` |

## Required Invariants

- Rust names are canonical. Python should preserve `snake_case`; WASM should expose `camelCase` via `js_name`.
- Bindings use `pub(crate) inner: RustType` plus `from_inner()` for wrapper construction.
- Error mapping stays centralized: Python uses crate error mappers; WASM returns `JsValue` or the established JS error shape.
- Binding code does not implement pricing, risk, validation, scenario math, or portfolio aggregation.
- `.pyi`, `__all__`, module registration, JS facade exports, and parity contract entries move with public API changes.

## Verification Defaults

Use the narrowest meaningful checks first:

- Python binding touched: `mise run python-build`, then targeted Python/parity tests.
- WASM binding touched: `mise run wasm-build`, then targeted WASM tests if present.
- Public API renamed or moved: search stubs, exports, examples, notebooks, and parity contract.
- Rust behavior changed: run targeted Rust tests before binding parity tests.
