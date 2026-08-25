# finstack-ai

Deterministic agent microkernel in Rust, with a runtime, SDK, and Python / WASM
bindings. The kernel owns semantic state, records, events, and effects. The
runtime owns ports and effect execution. Leaf providers, tools, stores, and
observers stay outward-facing.

Workspace manifests are staged at **2.0.0**. No `v2.0.0` tag exists yet;
local tag `v1.0.0` remains the latest local release, and the last pushed
GitHub tag is `v0.1.0`. crates.io / PyPI / npm packages are not published.
Install from this repository until those registries publish.

## Quick start

## Developer bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tools and
checked-in tasks.

1. Install mise: <https://mise.jdx.dev/getting-started.html>
2. From the repository root: `mise run install-all`

Common tasks:

- `mise run ci-all` — sequential local equivalent of hosted CI
- `mise run ci-rust` / `ci-python` / `ci-wasm` — per-language required checks
- `mise run build-all` / `build-rust` / `build-python` / `build-wasm` — optional profile after `--`
- `mise run check-all` / `check-rust` / `check-python` / `check-wasm` — formatting, lint, and typecheck
- `mise run test-all` / `test-rust` / `test-python` / `test-wasm` — language test suites
- `mise run test-fast` — workspace nextest (not plugin-host), venv pytest, Chromium-only WASM
- `mise run coverage-all` / `coverage-rust` / `coverage-python` / `coverage-wasm` — diagnostic coverage reports
- `mise run bench-all` / `bench-rust` / `bench-python` / `bench-wasm` — language benchmarks

Prefer `mise run <task>` over ad-hoc wrappers. The full task list is
[`mise.toml`](mise.toml). Hosted CI lives under
[`.github/ci/README.md`](.github/ci/README.md).

The installable Python package lives under `bindings/finstack-ai-python`.
Build it from the root with `uv build --package finstack-ai` or
`uv build --project bindings/finstack-ai-python`.

`uv sync` at the repository root creates `.venv` with editable
`finstack-ai[pydantic]` plus notebook kernel packages. Register
`finstack-ai-notebooks` from that environment; see
[examples/python-notebooks/README.md](examples/python-notebooks/README.md).

## Workspace layout

Core crates live under `crates/`,
bindings under `bindings/`, trusted native leaves under `extensions/`,
isolated WIT/Wasmtime packages under `plugins/`, and starters under
`examples/`.

## License and governance

- Dual-licensed: [MIT](licenses/LICENSE-MIT) OR [Apache-2.0](licenses/LICENSE-APACHE)
- Contributions: [CONTRIBUTING.md](CONTRIBUTING.md) (DCO sign-off; no CLA)
- Code of Conduct: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- Maintainers: [GOVERNANCE.md](GOVERNANCE.md)
- Vulnerability reports: [SECURITY.md](SECURITY.md)
- Changelog: [CHANGELOG.md](CHANGELOG.md)
