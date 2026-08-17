# CI

Owner: `me@jeickmeier.com`

All executable checks are canonical mise tasks. Workflows must call
`mise run <task>` and must not reimplement policy in YAML.

## Tasks

Root [`mise.toml`](../../mise.toml) defines the required tasks:

| Task | Purpose |
| --- | --- |
| `format` | Write-mode `cargo fmt` and `ruff format` |
| `check` | `cargo fmt --check`, Clippy with warnings denied, `ruff format --check`, `ruff check`, and `mypy --strict` |
| `test` | `cargo test --workspace` and the Python test suite against an editable binding install |
| `coverage` | Diagnostic Rust, Python, and WASM reports under `target/coverage/` (not in `ci`) |
| `coverage-rust` | `cargo llvm-cov` workspace HTML + LCOV |
| `coverage-python` | pytest-cov HTML + XML for the Python binding |
| `coverage-wasm` | Scaffold artifact until wasm-bindgen-test coverage exists |
| `check-wasm` | Type-check the selected wasm-host graph and reject Tokio/native I/O |
| `generate-wasm` | Regenerate wasm-bindgen glue and the `@finstack/ai` TypeScript facade |
| `test-browser` | Type-check and run the headless Chromium, Firefox, and WebKit package harness |
| `stage-wasm` | Pack unpublished `@finstack/ai` artifacts and typecheck a clean install |
| `benchmark-wasm` | Record WASM/JS crossing warning measurements |
| `ci` | `check`, `test`, `check-wasm`, `supply-chain`, `docs-links`, `docs-license`, `check-guest-sdk`, `check-plugin-template`, and `check-plugin-lock` |
| `supply-chain` | cargo-deny across the workspace and fuzz manifest (Eng §9; TM-18) |
| `secret-scan` | gitleaks over the repository tree and history (SEC-INV-005; TM-04) |
| `secret-scan-canary` | Temporary-repo negatives proving gitleaks fails on assembled canaries |
| `fuzz-smoke` | Deterministic bounded cargo-fuzz smoke (nightly) |
| `kernel` / `runtime` / `python-binding` / `wasm-binding` | Narrow local crate gates. Not a substitute for `check` / `ci` |

## Workflows

| Workflow | Triggers | Purpose |
| --- | --- | --- |
| [`ci.yml`](../workflows/ci.yml) | every PR, `main` push, manual | Single Ubuntu job running `check`, `test`, `check-wasm`, `generate-wasm` (consecutive byte-identical rebuild), and `test-browser`. Dirty-tree vs committed Darwin glue stays a same-host `check.py dirty` obligation. |
| [`security.yml`](../workflows/security.yml) | every PR, `main` push, weekly, manual | `supply-chain`, `secret-scan`, and `secret-scan-canary`. |
| [`nightly.yml`](../workflows/nightly.yml) | daily, manual | `starter-rc` then `fuzz-smoke`. Does not publish. |
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

The following historical mise tasks no longer exist: `architecture`,
`schema-governance`, `test-schema-governance`, `docs`, `check-minimal`,
`test-miri`, `check-nightly`, `release-smoke`, `release-reproducible`,
`build-python-wheel-ci`, per-PR `test-pr0xx` wrappers, and the focused
`test-kernel` / `test-model` / `test-tool` / `test-events` /
`test-extensions` / `test-lifecycle` / `test-runtime` / `test-runtime-gate` /
`test-sdk` slices. Historical evidence under `docs/implementation/` records
the runs made while those gates were active.

Surviving replacements: `check`, `test`, `conformance`, `supply-chain`,
`docs-links`, `docs-quickstarts`, `secret-scan`, `secret-scan-canary`,
`fuzz-smoke`, `starter-rc`, and `release-rehearsal`. Do not invent a passing
result for a retired task.
