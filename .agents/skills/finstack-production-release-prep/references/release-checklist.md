# Finstack Release Checklist

Use this as the repo-specific release checklist.

## Core Gates

- Format and lint: `mise run check-all`
- Tests: `mise run test-all` (or `mise run test-fast` while iterating)
- CI-equivalent: `mise run ci-all` (hosted CI runs `ci-rust` / `ci-python` / `ci-wasm` in parallel)
- Security/audit: `cargo-deny check`

## Bindings

- WASM graph: included in `mise run ci-all`
- WASM glue regeneration: `mise run build-wasm -- release` when bindings changed
- Browser tests: `mise run test-wasm` when the JS host changed
- Public items: `uv run --no-project python scripts/compat/public_items.py --check`
- Conformance: `cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests`

## Examples

- Rust examples: use repo-specific example tasks if present in `mise.toml`

## Release Notes

Include:

- user-facing summary,
- breaking changes and migration snippets,
- new APIs,
- bug fixes,
- performance or behavioral changes,
- docs and examples updates,
- known limitations.
