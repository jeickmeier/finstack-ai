# finstack-ai

Deterministic agent microkernel in Rust, with a runtime, SDK, and Python / WASM
bindings. The kernel owns semantic state, records, events, and effects. The
runtime owns ports and effect execution. Leaf providers, tools, stores, and
observers stay outward-facing.

Workspace version is **1.0.0**. Local tag `v1.0.0` exists. The last pushed
GitHub tag is `v0.1.0`. crates.io / PyPI / npm packages are not published.
Install from this repository until those registries publish.

## Quick start

| Language | Guide | Offline starter |
| --- | --- | --- |
| Rust | [docs/site/rust.md](docs/site/rust.md) | `cargo run -p finstack-ai-native-examples --bin minimal --offline --locked` |
| Python | [docs/site/python.md](docs/site/python.md) | `uv run --isolated --no-project --with-editable bindings/finstack-ai-python python examples/python-minimal/python-callback/main.py` |
| JavaScript / WASM | [docs/site/wasm.md](docs/site/wasm.md) | `mise run stage-wasm` then [examples/browser-minimal](examples/browser-minimal/README.md) |

Public guides: [docs/site/README.md](docs/site/README.md).
Concept: [docs/site/concept.md](docs/site/concept.md).
FAQ: [docs/site/faq.md](docs/site/faq.md).
Troubleshooting: [docs/site/troubleshooting.md](docs/site/troubleshooting.md).

## Developer bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tools and
checked-in tasks.

1. Install mise: <https://mise.jdx.dev/getting-started.html>
2. From the repository root: `mise install`

Common tasks:

- `mise run format` — write-mode Rust and Python formatter
- `mise run check` — formatting, Clippy, Ruff, and mypy
- `mise run test` — Rust workspace tests and Python tests
- `mise run kernel` / `runtime` / `python-binding` / `wasm-binding` — narrow crate gates (`check` / `ci` stay the workspace gate)
- `mise run coverage` — diagnostic Rust, Python, and WASM coverage reports
- `mise run ci` — `check` plus `test` (hosted CI)
- `mise run docs-links` — relative markdown links on the public set
- `mise run docs-quickstarts` — offline public-package starters

Prefer `mise run <task>` over ad-hoc wrappers. The full task list is
[`mise.toml`](mise.toml). Hosted CI lives under
[`.github/ci/README.md`](.github/ci/README.md).

The installable Python package lives under `bindings/finstack-ai-python`.
Build it from the root with `uv build --package finstack-ai` or
`uv build --project bindings/finstack-ai-python`.

## Workspace layout

See [Technical Design §2](docs/planning/03-finstack-ai-technical-design.md)
for the permanent repository shape. Core crates live under `crates/`,
bindings under `bindings/`, trusted native leaves under `extensions/`,
isolated WIT/Wasmtime packages under `plugins/`, and starters under
`examples/`.

Planning baseline: [docs/planning/README.md](docs/planning/README.md).
Delivery records: [docs/implementation/README.md](docs/implementation/README.md).

## License and governance

- Dual-licensed: [MIT](licenses/LICENSE-MIT) OR [Apache-2.0](licenses/LICENSE-APACHE)
- Contributions: [CONTRIBUTING.md](CONTRIBUTING.md) (DCO sign-off; no CLA)
- Code of Conduct: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- Maintainers: [GOVERNANCE.md](GOVERNANCE.md)
- Vulnerability reports: [SECURITY.md](SECURITY.md)
- Changelog: [CHANGELOG.md](CHANGELOG.md)
- Architecture decisions: [docs/implementation/adr-register.md](docs/implementation/adr-register.md)
- Public RFCs: [docs/rfcs/README.md](docs/rfcs/README.md)
- Trust levels: [docs/site/security-trust-levels.md](docs/site/security-trust-levels.md)
