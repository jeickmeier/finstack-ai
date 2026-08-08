# finstack-ai

`finstack-ai` is a deterministic agent microkernel in Rust, with a runtime, SDK/facade, and Python/WASM bindings. The kernel owns semantic state, records, events, and effects; the runtime owns ports and effect execution; leaf providers, tools, stores, and observers stay outward-facing.

Project documentation is routed through [docs/README.md](docs/README.md).

- The [planning baseline](docs/planning/README.md) defines product scope, architecture, engineering rules, technical design, security obligations, sequencing, and acceptance criteria.
- The [implementation control set](docs/implementation/README.md) records delivery status, ownership, actual work, decisions, evidence, and exceptions while implementation is under way.

## License and governance

- Dual-licensed: [MIT](licenses/LICENSE-MIT) OR [Apache-2.0](licenses/LICENSE-APACHE)
- Contributions: [CONTRIBUTING.md](CONTRIBUTING.md) (DCO sign-off)
- Maintainers and process: [GOVERNANCE.md](GOVERNANCE.md)
- Vulnerability reports: [SECURITY.md](SECURITY.md)
- Changelog: [CHANGELOG.md](CHANGELOG.md)

## Developer bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tool installs and checked-in tasks.

1. Install mise if needed: <https://mise.jdx.dev/getting-started.html>
2. From the repo root, install pinned tools: `mise install`
3. Verify the toolchain: `mise run doctor`

Useful tasks after bootstrap:

- `mise run format` / `clippy` / `test` / `docs` — Rust merge gates
- `mise run check-minimal` — kernel-only and no-default-features graphs
- `mise run architecture` — dependency/feature boundary enforcement
- `mise run schema-governance` — ADR inventory, contract registry, and schema/fixture coupling
- `mise run supply-chain` / `secret-scan` / `secret-scan-canary` — Eng §8–9 / TM-04 / TM-18
- `mise run release-smoke` — private CI release binary packaging
- `mise run build-python` — Python binding package under `bindings/finstack-ai-python`
- `mise run ci` — local aggregate of the checks above

The installable Python package lives under `bindings/finstack-ai-python`, not the repository root. Prefer `mise run build-python`, or from the root: `uv build --package finstack-ai`.

Tool versions and tasks live in [`mise.toml`](mise.toml). Prefer `mise run <task>` over ad-hoc wrappers. Hosted CI workflows and the reserved matrix documentation live under [`.github/ci/README.md`](.github/ci/README.md).

## Workspace layout

See [Technical Design §2](docs/planning/03-finstack-ai-technical-design.md) for the permanent repository shape. Core crates live under `crates/`, bindings under `bindings/`, trusted native leaf batteries under `extensions/` (providers, toolsets, stores, observers), isolated WIT/Wasmtime packages under `plugins/`, and supporting trees under `examples/` and `fixtures/`.
