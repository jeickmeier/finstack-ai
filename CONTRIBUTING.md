# Contributing

Thanks for your interest in `finstack-ai`.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tools and tasks.

1. Install mise: <https://mise.jdx.dev/getting-started.html>
2. From the repository root: `mise install`

Prefer `mise run <task>` over ad-hoc wrappers. The full task list is
[`mise.toml`](mise.toml). See [`README.md`](README.md) and
[`.github/ci/README.md`](.github/ci/README.md).

Before opening a pull request, run at least:

```bash
mise run ci
```

When the change touches public docs or starters, also run:

```bash
mise run docs-links
mise run docs-quickstarts
```

The hosted `ci.yml` workflow invokes the same task names. Do not reimplement
checks in workflow YAML.

Other tasks used by public docs:

- `mise run check` — formatting, Clippy, Ruff, mypy
- `mise run test` — Rust and Python tests
- `mise run kernel` / `runtime` / `python-binding` / `wasm-binding` — narrow crate gates; not a substitute for `check` / `ci`
- `mise run coverage` — diagnostic Rust, Python, and WASM coverage reports
- `mise run conformance` — published port and plugin suites
- `mise run generate-wasm` / `mise run stage-wasm` — JS/WASM package
- `mise run check-plugin-template` — plugin guest templates
- `mise run migrate` — 0.1.0 → 1.0.0 in-tree migration helpers

## Developer Certificate of Origin (DCO)

Every commit must be signed off under the [DCO](https://developercertificate.org/):

```text
Signed-off-by: Your Name <your.email@example.com>
```

Use `git commit -s` (or an equivalent that adds the trailer). The project does not use a CLA. See [`GOVERNANCE.md`](GOVERNANCE.md).

## Coding conventions

- Follow the [Engineering Standards](docs/planning/00-finstack-ai-engineering-standards.md).
- Preserve the workspace layout and dependency direction in the [Technical Design](docs/planning/03-finstack-ai-technical-design.md) sections 2–4.
- Agent-oriented repository rules live under [`.agents/rules/`](.agents/rules/) and [`AGENTS.md`](AGENTS.md).
- Public names follow the shared semantic vocabulary with idiomatic case per language.
- Do not add `rust-toolchain.toml` or a Cargo `xtask` crate; root `mise.toml` is the sole toolchain pin and task entrypoint.
- Keep the kernel deterministic, synchronous, and I/O-free.

## Architecture, security, and evidence

Before proposing substantial changes, read:

- [Engineering Standards](docs/planning/00-finstack-ai-engineering-standards.md) — including exception and waiver rules (section 14)
- [Security and Threat Model](docs/planning/06-finstack-ai-security-threat-model.md) — review triggers and controls
- [Implementation control set](docs/implementation/README.md) — delivery ledger, evidence register, exceptions register, ADR register
- [Public RFCs](docs/rfcs/README.md) — required in addition to an ADR for journal, event-order, WIT, and remote/process contract changes
- [Trust levels](docs/site/security-trust-levels.md) — T0–T5; in-process code is not a sandbox
- Dual-license texts: [MIT](licenses/LICENSE-MIT) OR [Apache-2.0](licenses/LICENSE-APACHE)

Pull requests should:

- stay within one logical PR from the [Implementation Plan](docs/planning/04-finstack-ai-implementation-plan.md) when practical;
- note architecture or threat-model review triggers when they apply;
- record real evidence in the evidence register rather than inventing completion; and
- link any ADR or time-bounded waiver required for a standards exception.

## Ownership

Contribution acceptance and releases are owned by the **finstack-ai maintainers** group. The current contribution-acceptance and release owner is `me@jeickmeier.com`. See [`GOVERNANCE.md`](GOVERNANCE.md) and [`SECURITY.md`](SECURITY.md).

## Pull requests

Use the repository pull request template. Keep commits short and imperative. Do not mix unrelated logical PRs for convenience.
