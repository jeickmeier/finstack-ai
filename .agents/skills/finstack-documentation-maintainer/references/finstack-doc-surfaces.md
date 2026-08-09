# Finstack Quant Documentation Surfaces

Use this reference to choose the right verification depth for documentation changes.

## Canonical Inputs

- `AGENTS.md`: project structure, workflows, binding conventions, naming strategy, and quality gates.
- `finstack-quant-py/DOCS_STYLE.md`: Python documentation style and generated-doc expectations.
- `docs/REFERENCES.md`: canonical academic and market references for formulas and conventions.
- `finstack-quant-py/parity_contract.toml`: binding parity source of truth.
- `docs/superpowers/specs/`: design specs and implementation notes; preserve status/history where relevant.

## Derived Or Mirrored Docs

- Python `.pyi` stubs under `finstack-quant-py/finstack_quant/`
- PyO3 docstrings and module `__doc__` assignments
- WASM TypeScript declarations and JS facades
- Example notebooks under `finstack-quant-py/examples/notebooks/`
- README or crate-level docs that mirror public API names

## Verification Hints

- Public API docs: check Rust source, PyO3/WASM bindings, stubs, exports, examples, and parity tests.
- Command docs: confirm task names against `mise.toml` or `AGENTS.md`.
- Notebook docs: prefer `uv run python finstack-quant-py/examples/notebooks/run_all_notebooks.py` when scope justifies it.
- Financial formulas: cite `docs/REFERENCES.md` anchors or remove unsupported citations.
- Generated docs: update the source contract or generator where practical.
