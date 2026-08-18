# Finstack Documentation Surfaces

Use this reference to choose the right verification depth for documentation changes.

## Canonical Inputs

- `AGENTS.md`: project structure, workflows, binding conventions, naming strategy, and quality gates.
- `.agents/rules/04-python-coding.md`: Python documentation style and IDE-facing stub expectations.
- `.agents/rules/05-typescript-coding.md`: TypeScript/JS documentation style.
- `docs/planning/`: design contract; read-only during normal coding.
- `docs/implementation/`: current work and proof.

## Derived Or Mirrored Docs

- Python `.pyi` stubs under `bindings/finstack-ai-python/python/finstack_ai/`
- PyO3 docstrings and module `__doc__` assignments
- WASM TypeScript declarations and JS facades under `bindings/finstack-ai-wasm/js/`
- Example crates and docs under `examples/` and `docs/`
- README or crate-level docs that mirror public API names

## Verification Hints

- Public API docs: check Rust source, PyO3/WASM bindings, stubs, exports, examples, and public-item inventory.
- Command docs: confirm task names against `mise.toml` or `AGENTS.md`.
- Example docs: prefer `uv run --no-project python tools/docs/quickstarts.py` when scope justifies it.
- Generated docs: update the source contract or generator where practical.
