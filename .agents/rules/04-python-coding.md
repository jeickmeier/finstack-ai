---
trigger: glob
description: Enforce Python binding quality, IntelliSense, and docstring rules for finstack-ai.
globs: "**/*.{py,pyi}"
---

# Python coding

Apply with `01-engineering-conformance.md` and `02-testing-and-delivery.md`. Bindings stay coarse; Rust owns semantics.

## Production-safe Python

- Follow the checked-in Python toolchain, formatter, linter, type-checker, packaging policy, and task commands once configured (Maturin/PyO3 for the native binding). Prefer `uv run` for local Python commands.
- Bindings are coarse adapters: expose handles, awaitables, and normalized commands. Do not reimplement continuation, recovery, interaction, lineage, or event-order semantics in Python.
- Annotate public modules, classes, methods, and functions with precise type hints. Prefer `typing`/`collections.abc` protocols over inheritance from C-extension base types; keep Pydantic and other extras optional and lazy where design requires.
- Map framework errors to the stable `FinstackError` hierarchy with `code`, identifiers, retryability, and safe details. Do not invent incompatible exception semantics or leak secrets in messages.
- Release the GIL for Rust-only construction, validation, scheduling, I/O, storage, and waits. Acquire it only for conversion and Python callable execution. Callbacks stay trusted, cancellable, timeout-bounded, and batched where design requires (for example observer delivery).
- Cache schemas, validators, and metadata at registration. Prohibit per-token or per-call regeneration on hot paths. Convert raw JSON to Python once per callback boundary.
- Prefer async APIs that mirror the Rust surface (`run`/`start`, async event iteration, explicit `cancel`). Dropping a run object must not silently cancel a durable run.
- Keep generated binding or packaging output reproducible and separate from hand-authored Python. Do not hand-edit generated artifacts; regenerate via the documented command and fail on dirty output.

## IntelliSense and IDE docs

Treat editor completion, hover docs, go-to-definition, and signature help as part of the public Python API. A binding change that breaks IDE discoverability is incomplete.

- Ship PEP 561 typing support: include `py.typed` in the installed package and package public `.pyi` stubs (or fully typed pure-Python modules) with the wheel/sdist so Pyright/Pylance and mypy see them without extra installs.
- Native/PyO3 extension modules must expose a typed facade IDEs can resolve. Prefer checked-in `.pyi` beside the import path, or thin typed pure-Python wrappers that re-export the extension. Do not leave public `#[pyclass]`/`#[pyfunction]` surfaces as untyped binary modules.
- Keep stub signatures identical to runtime: parameter names, defaults, positional/keyword kinds, overloads, `async`/`Awaitable`/`AsyncIterator` forms, context managers, and properties. Use `@typing.overload` for polymorphic constructors and helpers; avoid untyped `*args`/`**kwargs` on public APIs.
- Put Google-style docstrings on the symbols IDEs resolve—public stub methods/classes and pure-Python wrappers—not only in Rust `#[pyo3(...)]` attributes or internal comments. Hover text must show summary, parameters, return value, and raised errors for primary workflows.
- Maintain PyO3 `#[pyo3(text_signature = "...")]` (or equivalent) so runtime `help()`/`inspect.signature` stay aligned with stubs. When a docstring exists in both Rust and `.pyi`, keep them congruent; stubs remain the IDE source of truth for types and prose.
- Export a stable public surface through package `__init__` / `__all__`. Lazy provider submodules must still offer typed imports and stub coverage so autocomplete works before the submodule is imported for side effects.
- Prefer `Protocol`, `TypedDict`, `Literal`, `Enum`, `TypeAlias`, and precise generics over `Any`. Mark truly internal helpers with leading underscores and omit them from stubs/`__all__` unless tests require them.
- Validate IDE contracts in CI once tooling exists: type-check the public package against its stubs, and fail when stub/runtime signatures drift. Manual smoke-check that Pylance/Pyright hover shows docs for new public APIs before handoff.

## Docstrings

- Use PEP 257 one-line summaries; for public APIs prefer Google-style sections: `Args`, `Returns`, `Raises`, and `Examples` when useful. Module docstrings state the package or submodule role.
- Document exception types and stable `code` values callers should handle. Note GIL, threading/free-threading, cancellation, and timeout behavior when the API crosses into Rust or runs user callbacks.
- Keep docstrings aligned with type hints; do not restate obvious types. Document optional extras (for example Pydantic) and import/lazy-load constraints.
- Docstrings must live on stub or wrapper symbols so IDE hover and signature help show them, not only inside the extension binary.
