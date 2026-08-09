# CI matrix (PR-003)

Owner: `me@jeickmeier.com`

All executable checks are canonical mise tasks. Workflows must call
`mise run <task>` and must not reimplement policy in YAML.

## Workflows

| Workflow | Triggers | Purpose |
| --- | --- | --- |
| [`ci.yml`](../workflows/ci.yml) | every PR, `main` push, manual | format, Clippy, workspace tests (including conformance), docs, minimal features, architecture, explicit WASM target checks, schema governance, benchmark compile checks, Python package smoke, release-smoke on Linux/macOS/Windows |
| [`security.yml`](../workflows/security.yml) | every PR, `main` push, Mondays 04:17 UTC, manual | cargo-deny (Eng §9 / TM-18), secret scan + canary negatives (SEC-INV-005 / TM-04) |
| [`nightly.yml`](../workflows/nightly.yml) | every PR, Sundays 05:37 UTC, manual | pinned `nightly-2026-08-01` compatibility only; fuzz remains documentation-reserved (no green placeholder job) |
| [`benchmark.yml`](../workflows/benchmark.yml) | Mondays 06:17 UTC, manual | Criterion benches + machine-readable metadata; artifact upload; **not** a required PR check |

## Toolchain channels (PR-003-A02)

| Channel | Pin | Cadence | Owner |
| --- | --- | --- | --- |
| stable | mise Rust `1.97.1` | every PR (`ci.yml` label `stable`) | `me@jeickmeier.com` |
| MSRV | workspace/mise `1.97.1` until an explicit MSRV ADR | every PR (`ci.yml` label `msrv`; intentionally coincides with stable) | `me@jeickmeier.com` |
| nightly | `nightly-2026-08-01` | every PR + weekly (`nightly.yml`) | `me@jeickmeier.com` |
| fuzz | reserved; no targets yet | docs-only until parser/fuzz targets land; intended weekly cadence then; `evidence_eligible = false` | `me@jeickmeier.com` |

## Path filters

Required checks intentionally have **no** `paths` / `paths-ignore` filters in
PR-003. Path-aware skipping may be added later only when it cannot skip
architecture, supply-chain, secret, or release-smoke evidence on code changes.

## Action and tool pins

- GitHub Actions are pinned by full commit SHA with a version comment.
- Contributor/CI tools are pinned in root [`mise.toml`](../../mise.toml).
- Update pins by changing `mise.toml` and the workflow SHA comments together;
  then run `mise run lint-workflows` and `mise run doctor`.
- The Rust matrix uses the pinned `Swatinem/rust-cache` action to reuse dependency
  build artifacts across runs. Its automatic key separates jobs and rustc
  host/toolchain identities and includes the Cargo graph and compiler
  environment; steps within one job continue to share the workspace `target/`
  directory directly.

## Release artifacts (PR-003-A03)

`mise run release-smoke` builds the private unpublished
`fixtures/ci/release-smoke` binary (`finstack-ai-ci-smoke`), executes it, and
stages:

- the platform binary under `target/ci-release/`
- `SHA256SUMS`
- `build-metadata.json` (commit, rustc, host triple, features)

Linux also runs `mise run release-reproducible` (two clean target dirs, digest
compare). Artifacts upload with 30-day retention. This is CI retention, not
publication to crates.io/PyPI/npm.

## Generated bindings and fixtures (PR-003-A04)

Current inventory:

| Surface | Status | Regeneration | Dirty-tree policy |
| --- | --- | --- | --- |
| Python binding package | hand-authored setuptools placeholder | `mise run build-python` | no generator tree yet |
| Browser WASM binding | hand-authored crate placeholder | `mise run check-wasm` | no wasm-bindgen glue yet |
| WIT / schema codegen | not present | deferred to owning PRs | when generators exist, CI must regenerate and fail on dirty output |
| Schema / ADR governance | hand-authored reserved roots + checker | `mise run schema-governance` | schema path changes without fixture updates fail GOV006 when a base SHA is available; reserved README-only dirs are not conformance evidence |
| Architecture fixtures | hand-authored case files | `mise run architecture` | fixtures must activate the claimed check |
| Security canary-redaction fixture | contract-only fragments | `mise run secret-scan-canary` (runtime temp repos) | `evidence_eligible = false` until a redactor consumes it |

### Schema governance (PR-004)

Owner: `me@jeickmeier.com`

Canonical tasks:

- `mise run test-schema-governance`
- `mise run lint-schema-governance` / `format-schema-governance`
- `mise run schema-governance` (optional `--base <sha>` or `SCHEMA_GOVERNANCE_BASE`)

The checker enforces ADR-001–ADR-037 inventory/links (GOV001–GOV003), contract
registry completeness (GOV004), schema/fixture naming and coupling
(GOV005–GOV006), and PR-template impact sections (GOV007). Hosted
`schema-governance` uses `fetch-depth: 0` and the pull-request base SHA when
present; workflow_dispatch/manual runs without a base still execute static
checks. Reserved `schemas/` and `fixtures/compatibility/` directories are
governance scaffolding only until owning PRs add payloads.

When a generator lands, document its owning package command here and add a CI
step that regenerates then fails if `git status --porcelain` is non-empty.

## Reserved matrices (`evidence_eligible = false`)

These reservations are documentation-only. Do not add green placeholder jobs.

| Matrix | Reservation | Cadence (once activated) | Activation | Owner |
| --- | --- | --- | --- | --- |
| Python wheels | CPython 3.11–3.14 and 3.14t; manylinux x86_64/aarch64, macOS arm64, Windows x64 | every PR for smoke subset; scheduled full matrix | PR-027 | `me@jeickmeier.com` |
| Headless browser smoke | browser WASM conformance placeholders | every PR once browser package exists | PR-033–PR-036 | `me@jeickmeier.com` |
| Parser fuzz smoke | no targets yet; no workflow job until targets exist | weekly via `nightly.yml` once targets exist | PR introducing each parser | `me@jeickmeier.com` |
| Security-boundary suites | adversarial authorization/permission suites | every PR for owning surface | owning runtime/binding/plugin PRs | `me@jeickmeier.com` |
| Benchmark regression | activated as scheduled/manual artifact collection; not merge-blocking; no threshold enforcement | Mondays 06:17 UTC + `workflow_dispatch` via `benchmark.yml`; PR CI only compiles benches (`tools/benchmark/run.py compile`) | PR-005 | `me@jeickmeier.com` |

### Benchmark harness (PR-005)

Owner: `me@jeickmeier.com`

Canonical tasks:

- `mise run conformance` — golden-trace / conformance unit+integration tests
- `mise run test-benchmark` / `lint-benchmark` / `format-benchmark`
- `mise run benchmark-smoke` — compile + short Criterion run with metadata
- `mise run benchmark` — full non-blocking Criterion run with metadata under `target/benchmark/`
- `mise run coverage` / `coverage-rust` / `coverage-python` / `coverage-wasm` — diagnostic coverage reports under `target/coverage/` (uploaded by the Ubuntu `coverage` job; no percentage gate; WASM is a scaffold until real wasm tests exist)

The required Rust matrix runs conformance once through `mise run test`
(`cargo test --workspace`). The focused `mise run conformance` task remains
available for local development without rerunning the same package in the
aggregate CI sequence.

Metadata schema: `schemas/benchmark-report/v1/metadata.schema.json`.  
Artifacts include compiler, target, commit, feature set, and machine metadata
(PR-005-A03). Flamegraphs remain optional until regression thresholds exist.
`evidence_eligible` for merge gates remains false for performance budgets;
metadata shape evidence is eligible for PR-005-A03.

Placeholder jobs must not report passing evidence for unimplemented work.

## Local commands

```bash
mise install
mise run doctor
mise run ci
mise run check-nightly   # installs pinned nightly via rustup
```
