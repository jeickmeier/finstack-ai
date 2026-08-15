# PR-065 execution plan

Date: 2026-08-15
Owner: me@jeickmeier.com
Branch: `codex/pr-065-ecosystem-conformance-release-eng`
Baseline: local `main` at `c2159c79a4aedea82bb50b19f387d88b8c271052`
Plan baseline: documentation pack v0.21 / PLAN-0.19 / Implementation Plan SHA-256
`86d2430860b12ab947638052b97ea2403c2e53b218a66977a4258defeb18fc3f`

This file is the execution contract for PR-065. The closed PR-054
through PR-064 envelopes are not reused. PR-065 is the only active
logical PR. This file does not cut `1.0.0` or record `G8-D-*`.

## Execution envelope

Authorized by owner `proceed to PR-065` (2026-08-15):

```
Run PR-065; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi/crates.io publish or
yank, tag, hosted nightly dispatch, Sigstore publication, G8
inference. Do not write `G8-D-*`. Do not start PR-066. Do not cut
or publish `1.0.0`. Do not invent a plugin marketplace or create
`release/1.0`.

## Admission

All admission checks were true at admit:

| Check | State |
| --- | --- |
| Phase 9 entrance | **Passed** (2/2) via `PH9-E-entrance-tag-b610b0ba93b5` and `PH9-E-entrance-feedback-ee6999c59a12`. Not re-recorded. |
| Phase 8 / G7 | **Done** / **Passed** via `G7-D-public-preview-f7c7e70b9e04`. |
| PR-062 through PR-064 | **Done**. |
| Compatibility policy files | `1.0-compatibility-policy.md` and `1.0-leaf-versioning.md` exist. |
| ADR / marketplace | No seventh port, kernel I/O, or marketplace. |

## Traceability

Implementation Plan PR-065; Phase 9 exit evidence toward
predictable releases (not `G8-D-*`); Engineering Standards §9
(SBOM, checksums, provenance, reproducible builds), §12
(support-policy docs), §13 (review); PRD §15 release acceptance
and §16 ecosystem/quality metrics; TDD §32.1 binding/plugin
conformance; `port_conformance.rs` / plugin host conformance;
PR-060/PR-061 staging rehearsal.
G8 is Phase 9's gate and is out of scope. PR-066 cuts `1.0.0`.
PR-064 owns independent security review, not this PR.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-065-A01: A tagged release can be recreated from source with
  matching checksums. Two clean local staging runs from the same
  commit produce byte-identical SHA-256 sets for crates lists,
  wheels/sdist, npm stage, WIT packages, and SBOMs. Document the
  GA tag procedure for PR-066. Do **not** `git tag` under this
  envelope. Recreate from the commit a tag would point at.
- PR-065-A02: Downstream starter projects test against release
  candidates automatically. In-tree starters
  (`examples/rust-minimal`, `examples/python-minimal`,
  `examples/browser-minimal`, `examples/durable-interaction`,
  `examples/ts-alpha-install`) consume **staged** RC artifacts,
  not path-mapped workspace crates, in a mise task. A nightly
  workflow file may call that task. Dispatching hosted nightly
  is a separately named external action.
- PR-065-A03: Conformance failures identify the violated
  contract **and version**. Extend
  `PortConformanceFailure` and the plugin suite so every
  failure names `port`/`world`, stable `contract` id, and
  suite/engine version (workspace `0.1.0` at admit). A failure
  that says only "assert failed" is not enough.
- PR-065-A04: Rollback/hotfix rehearsal is completed. Stage a
  baseline artifact set and a patch-line set; document yank/
  retract per crates.io / PyPI / npm; prove a consumer can pin
  the previous checksum and that the hotfix stages with a new
  checksum. Do not yank, publish, or tag.

## Locked design

See the admitted implementation. Layout:

```text
docs/site/conformance.md
docs/site/support.md
docs/implementation/release-engineering.md
docs/implementation/support-windows.md
docs/implementation/artifacts/pr-065/
crates/finstack-ai-test/src/port_conformance.rs
plugins/finstack-ai-plugin-host/src/conformance_tests.rs
.github/workflows/nightly.yml
```

Do not invent `starters/`, a marketplace, plugin registry, badge
server, Cargo `xtask`, or `mise run schema-governance`. Do not
edit `docs/planning/*`. Workspace version stays `0.1.0`.

## Tasks

| Task | Acceptance |
| --- | --- |
| PR-065-T-tracking-e9a7e4afbac9 | Admit / branch |
| PR-065-T-conformance-bc2810f64c15 | A03 |
| PR-065-T-release-69103083bb1c | A01 |
| PR-065-T-starters-85584a859399 | A02 |
| PR-065-T-support-fbd84e63aa63 | Support windows |
| PR-065-T-hotfix-9114921ac3ea | A04 + TM-18 |

## Explicit exclusions

No centralized commercial plugin marketplace. No plugin
registry download. No `1.0.0` cut or announce. No G8
inference. No `git tag`. No registry publish, yank, or hosted
signing. No fabricated nightly pass. No Cargo xtask. No second
conformance harness. No `starters/` tree. No LTS invention.

## Validation

- Port/plugin conformance runs that print contract + version
  on a deliberate failure
- Two-run staging checksum identity (A01)
- Starter RC task against staged artifacts (A02)
- Hotfix rehearsal SHA-256 pair (A04)
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check` after the candidate is otherwise green
