# finstack-ai

`finstack-ai` is a deterministic agent microkernel in Rust, with a runtime, SDK/facade, and Python/WASM bindings. The kernel owns semantic state, records, events, and effects; the runtime owns ports and effect execution; leaf providers, tools, stores, and observers stay outward-facing.

Project documentation is routed through [docs/README.md](docs/README.md).
The public guide index is [docs/site/README.md](docs/site/README.md).

- The [planning baseline](docs/planning/README.md) defines product scope, architecture, engineering rules, technical design, security obligations, sequencing, and acceptance criteria.
- The [implementation control set](docs/implementation/README.md) records delivery status, ownership, actual work, decisions, evidence, and exceptions while implementation is under way.

## License and governance

- Dual-licensed: [MIT](licenses/LICENSE-MIT) OR [Apache-2.0](licenses/LICENSE-APACHE)
- Contributions: [CONTRIBUTING.md](CONTRIBUTING.md) (DCO sign-off; no CLA)
- Maintainers and process: [GOVERNANCE.md](GOVERNANCE.md)
- Architecture decisions: [docs/implementation/adr-register.md](docs/implementation/adr-register.md)
- Public RFCs: [docs/rfcs/README.md](docs/rfcs/README.md)
- Trust levels: [docs/site/security-trust-levels.md](docs/site/security-trust-levels.md)
- Vulnerability reports: [SECURITY.md](SECURITY.md)
- Changelog: [CHANGELOG.md](CHANGELOG.md)

## Developer bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tool installs and checked-in tasks.

1. Install mise if needed: <https://mise.jdx.dev/getting-started.html>
2. From the repo root, install pinned tools: `mise install`

Tasks after bootstrap:

- `mise run format` — write-mode Rust and Python formatter
- `mise run check` — formatting, Clippy, Ruff, and mypy checks
- `mise run test` — Rust workspace tests and Python tests
- `mise run ci` — `check` plus `test`, the same thing hosted CI runs

The installable Python package lives under `bindings/finstack-ai-python`, not the repository root. Build it from the root with `uv build --package finstack-ai`.

Tool versions and tasks live in [`mise.toml`](mise.toml). Prefer `mise run <task>` over ad-hoc wrappers. Hosted CI workflows and the reserved matrix documentation live under [`.github/ci/README.md`](.github/ci/README.md).

## Workspace layout

See [Technical Design §2](docs/planning/03-finstack-ai-technical-design.md) for the permanent repository shape. Core crates live under `crates/`, bindings under `bindings/`, trusted native leaf batteries under `extensions/` (providers, toolsets, stores, observers), isolated WIT/Wasmtime packages under `plugins/`, and supporting trees under `examples/` and `fixtures/`.
