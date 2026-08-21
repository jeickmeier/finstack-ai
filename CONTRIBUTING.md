# Contributing

Thanks for your interest in `finstack-ai`.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tools and tasks.

1. Install mise: <https://mise.jdx.dev/getting-started.html>
2. From the repository root: `mise run install-all`

Prefer `mise run <task>` over ad-hoc wrappers. The full task list is
[`mise.toml`](mise.toml). See [`README.md`](README.md) and
[`.github/ci/README.md`](.github/ci/README.md).

Before opening a pull request, run at least:

```bash
mise run ci-all
```

The hosted `ci.yml` workflow runs `mise run ci-rust`, `ci-python`, and
`ci-wasm` in parallel. `mise run ci-all` is the sequential local equivalent.
Do not reimplement those task bodies in workflow YAML.

Tasks:

- `mise run install-all` — pinned tools plus Rust, Python, and WASM environments
- `mise run ci-all` — sequential local equivalent of hosted CI
- `mise run ci-rust` / `ci-python` / `ci-wasm` — per-language required checks
- `mise run runtime` — runtime minimal/native/WASM-host lint and focused tests
- `mise run check-repository-references` — reject links to removed historical planning artifacts
- `mise run build-all` / `build-rust` / `build-python` / `build-wasm` — optional profile after `--` (default `dev`)
- `mise run check-all` / `check-rust` / `check-python` / `check-wasm` — formatting, lint, and typecheck
- `mise run test-all` / `test-rust` / `test-python` / `test-wasm` — language test suites
- `mise run test-fast` — workspace nextest (not plugin-host), venv pytest, Chromium-only WASM
- `mise run coverage-all` / `coverage-rust` / `coverage-python` / `coverage-wasm` — diagnostic coverage reports
- `mise run bench-all` / `bench-rust` / `bench-python` / `bench-wasm` — language benchmarks

## Developer Certificate of Origin (DCO)

Every commit must be signed off under the [DCO](https://developercertificate.org/):

```text
Signed-off-by: Your Name <your.email@example.com>
```

Use `git commit -s` (or an equivalent that adds the trailer). The project does not use a CLA. See [`GOVERNANCE.md`](GOVERNANCE.md).

## Coding conventions

- Preserve the package ownership and dependency direction documented in [`AGENTS.md`](AGENTS.md).
- Agent-oriented repository rules live under [`.agents/rules/`](.agents/rules/) and [`AGENTS.md`](AGENTS.md).
- Public names follow the shared semantic vocabulary with idiomatic case per language.
- Do not add `rust-toolchain.toml` or a Cargo `xtask` crate; root `mise.toml` is the sole toolchain pin and task entrypoint.
- Keep the kernel deterministic, synchronous, and I/O-free.

## Architecture, security, and validation

Before proposing substantial changes, read [`AGENTS.md`](AGENTS.md), the
applicable [agent rules](.agents/rules/), [`SECURITY.md`](SECURITY.md), and the
dual-license texts: [MIT](licenses/LICENSE-MIT) OR
[Apache-2.0](licenses/LICENSE-APACHE).

Changes should remain one coherent behavior slice, identify architecture or
security review triggers, document exact validation results, and link any
current architecture decision or time-bounded waiver required for an exception.

## Ownership

Contribution acceptance and releases are owned by the **finstack-ai maintainers** group. The current contribution-acceptance and release owner is `me@jeickmeier.com`. See [`GOVERNANCE.md`](GOVERNANCE.md) and [`SECURITY.md`](SECURITY.md).

## Pull requests

Use the repository pull request template. Keep commits short and imperative. Do not mix unrelated behavior changes for convenience.
