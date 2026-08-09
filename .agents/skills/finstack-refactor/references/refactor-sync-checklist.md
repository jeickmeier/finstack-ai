# Refactor sync checklist

Use this checklist before finishing any refactor that touches names, exports, module structure, or cross-language API shape.

| If you changed | Also inspect |
| --- | --- |
| Rust core public function or type | Python bindings, WASM bindings, `.pyi` stubs, docs, and parity tests if the surface is user-visible |
| Binding function signature or class constructor | matching `.pyi`, Python package exports, PyO3 `register()` wiring, text signatures, and parity tests |
| Binding module layout | `mod.rs`, `lib.rs` registration, Python `__init__.py` re-exports, and any stub package layout |
| Error or exception behavior | `finstack-quant-py/src/errors.rs`, Python-visible exception exports in `finstack-quant-py/src/lib.rs`, and tests/docs that assert exception classes or messages |
| Python-visible name | Rust export, Python re-export files, `.pyi`, examples, and parity tests |
| Accessor naming | adjacent APIs for `get_*` consistency across Rust, Python, and WASM surfaces |
| Metric key naming or identifiers | downstream tests, docs, notebooks, and any code that parses those keys |
| Large argument list converted to params struct | all callers, defaulting behavior, builder helpers if present, and docstrings/stubs |
| Internal module split with stable API | all re-exports so the public import path stays unchanged |

## Fast finishing pass

Before declaring the refactor done, state explicitly:

- what remained behaviorally invariant
- which external names stayed stable
- which mirrored surfaces were updated
- which validation the user should run if they want confirmation
