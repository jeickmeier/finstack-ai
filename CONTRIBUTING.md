# Contributing

Thanks for your interest in `finstack-ai`.

## Bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tools and tasks.

1. Install mise: <https://mise.jdx.dev/getting-started.html>
2. From the repository root: `mise install`

The repository defines four tasks: `format`, `check`, `test`, and `ci`. Prefer `mise run <task>` over ad-hoc wrappers. See [`mise.toml`](mise.toml), [`README.md`](README.md), and [`.github/ci/README.md`](.github/ci/README.md).

Before opening a pull request, run at least:

```bash
mise run ci
```

The hosted `ci.yml` workflow invokes the same task names. Do not reimplement checks in workflow YAML.

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

Pull requests should:

- stay within one logical PR from the [Implementation Plan](docs/planning/04-finstack-ai-implementation-plan.md) when practical;
- note architecture or threat-model review triggers when they apply;
- record real evidence in the evidence register rather than inventing completion; and
- link any ADR or time-bounded waiver required for a standards exception.

## Ownership

Contribution acceptance and releases are owned by the **finstack-ai maintainers** group. The current contribution-acceptance and release owner is `me@jeickmeier.com`. See [`GOVERNANCE.md`](GOVERNANCE.md) and [`SECURITY.md`](SECURITY.md).

## Pull requests

Use the repository pull request template. Keep commits short and imperative. Do not mix unrelated logical PRs for convenience.
