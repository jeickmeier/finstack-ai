# Finstack Quant Release Checklist

Use this as the repo-specific release checklist.

## Core Gates

- Format: `mise run all-fmt`
- Lint: `mise run all-lint`
- Tests: `mise run all-test`
- CI-equivalent: `mise run all-ci`
- Security/audit: `mise run all-audit` plus `cargo deny check` when needed

## Bindings

- Python build: `mise run python-build`
- Python release-profile build for runtime checks: `mise run python-build -- --release`
- Python lint/type/test: `mise run python-lint`, `mise run python-typecheck`, `mise run python-test`
- WASM build/lint/test: `mise run wasm-build`, `mise run wasm-lint`, `mise run wasm-test`
- Parity impact: check `finstack-quant-py/parity_contract.toml` and `finstack-quant-py/tests/parity`

## Examples

- Python notebooks: `uv run python finstack-quant-py/examples/notebooks/run_all_notebooks.py`
- Rust examples: use repo-specific example tasks if present in `mise.toml`; do not assume a `scripts/run-examples.sh` helper exists.

## Release Notes

Include:

- user-facing summary,
- breaking changes and migration snippets,
- new APIs,
- bug fixes,
- performance or numerical behavior changes,
- docs and examples updates,
- known limitations.
