# Finstack Release Checklist

Use this as the repo-specific release checklist.

## Core Gates

- Format and lint: `mise run check`
- Tests: `mise run test`
- CI-equivalent: `mise run ci`
- Security/audit: `mise run supply-chain`

## Bindings

- WASM graph: `mise run check-wasm`
- WASM glue regeneration: `mise run generate-wasm` when bindings changed
- Browser tests: `mise run test-browser` when the JS host changed
- Public items: `mise run check-public-items`
- Conformance: `mise run conformance`

## Examples

- Quickstarts: `mise run docs-quickstarts`
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
