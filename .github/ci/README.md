# CI

Owner: `me@jeickmeier.com`

Hosted CI calls `mise run ci-all`. Do not reimplement that policy in YAML.
Workflows that are not the required CI gate may invoke a remaining language
task or a documented Python tool when no mise task exists.

## Tasks

Root [`mise.toml`](../../mise.toml) defines the required tasks:

| Task | Purpose |
| --- | --- |
| `install-all` | Pinned tools plus Rust, Python, and WASM environments |
| `ci-all` | Same required checks as [`.github/workflows/ci.yml`](../workflows/ci.yml) |
| `build-all` / `build-rust` / `build-python` / `build-wasm` | Build each language. Optional profile after `--` (default `dev`) |
| `check-all` / `check-rust` / `check-python` / `check-wasm` | Formatting, lint, and typecheck |
| `test-all` / `test-rust` / `test-python` / `test-wasm` | Language test suites |
| `coverage-all` / `coverage-rust` / `coverage-python` / `coverage-wasm` | Diagnostic coverage reports under `target/coverage/` (not in `ci-all`) |
| `bench-all` / `bench-rust` / `bench-python` / `bench-wasm` | Language benchmarks |

## Workflows

| Workflow | Triggers | Purpose |
| --- | --- | --- |
| [`ci.yml`](../workflows/ci.yml) | every PR, `main` push, manual | Single Ubuntu job running `ci-all`. Dirty-tree vs committed Darwin glue stays a same-host `check.py dirty` obligation. |
| [`nightly.yml`](../workflows/nightly.yml) | daily, manual | In-tree starter RC against staged artifacts. Does not publish. |
| [`npm-release-staging.yml`](../workflows/npm-release-staging.yml) | manual | Stage, sign, and upload unpublished `@finstack/ai` artifacts. Does not publish. |

Required checks intentionally have **no** `paths` / `paths-ignore` filters.

## Toolchain pins

| Channel | Pin | Cadence | Owner |
| --- | --- | --- | --- |
| stable | mise Rust `1.97.1` plus `wasm32-unknown-unknown` | every PR | `me@jeickmeier.com` |
| MSRV | workspace/mise `1.97.1` until an explicit MSRV ADR | every PR (intentionally coincides with stable) | `me@jeickmeier.com` |
| Node | mise Node `22.18.0` and `wasm-bindgen-cli` `0.2.127` | every PR | `me@jeickmeier.com` |

## Action and tool pins

- GitHub Actions are pinned by full commit SHA with a version comment.
- Contributor/CI tools are pinned in root [`mise.toml`](../../mise.toml).
- Update pins by changing `mise.toml` and the workflow SHA comments together.
- The `ci` job uses the pinned `Swatinem/rust-cache` action to reuse dependency
  build artifacts across runs, and sets `CARGO_INCREMENTAL=0` and
  `CARGO_PROFILE_DEV_DEBUG=line-tables-only` to cut link time. It exports
  `PYO3_PYTHON` so the PyO3 crate builds against the pinned interpreter.

## Retired automation

The following historical mise tasks no longer exist: `format`, `check`,
`test`, `ci`, `kernel`, `runtime`, `python-binding`, `wasm-binding`,
`coverage` (replaced by `coverage-all`),
`generate-wasm`, `test-browser`, `stage-wasm`, `benchmark`,
`benchmark-smoke`, `benchmark-wasm`, `supply-chain`, `secret-scan`,
`secret-scan-canary`, `fuzz-smoke`, `fuzz-local`, `conformance`,
`docs-links`, `docs-quickstarts`, `docs-notebooks`, `docs-license`,
`migrate`, `gen-wit`, `check-wit`, `gen-guest-sdk`, `check-guest-sdk`,
`gen-plugin-wasm`, `check-plugin-wasm`, `check-plugin-template`,
`gen-plugin-lock`, `check-plugin-lock`, `check-public-items`,
`check-size-budgets`, `measure-packaging-profile`, `starter-rc`,
`recreate-release`, `release-rehearsal`, `hotfix-rehearsal`,
`architecture`, `schema-governance`, `test-schema-governance`, `docs`,
`check-minimal`, `test-miri`, `check-nightly`, `release-smoke`,
`release-reproducible`, `build-python-wheel-ci`, per-PR `test-pr0xx`
wrappers, and the focused `test-kernel` / `test-model` / `test-tool` /
`test-events` / `test-extensions` / `test-lifecycle` / `test-runtime` /
`test-runtime-gate` / `test-sdk` slices. Historical evidence under
`docs/implementation/` records the runs made while those gates were
active.

Do not invent a passing result for a retired task.
