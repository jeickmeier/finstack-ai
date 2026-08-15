# PR-065 execution plan

Date: 2026-08-15
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-065-ecosystem-conformance-release-eng`
Intended baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-065. The closed PR-054
envelope is not reused. The PR-055–PR-064 envelopes are not reused.
PR-065 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8 or Phase 9, cut `0.1.0`
or `1.0.0`, or record G5 / G7 / G8.

## Execution envelope

Not authorized. Suggested text when the owner is ready:

```
Run PR-065; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, from Phase 8
or Phase 9 plans existing, or from a phase name.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G5 inference, G7 inference, G8 inference. Do not
write `G5-D-*`, `G7-D-*`, or `G8-D-*`. Do not start PR-055–PR-064
or PR-066. Do not cut or publish `1.0.0`. Do not invent a plugin
marketplace, adopter usage, or a hosted nightly pass.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 9 entrance is `Passed` (2/2)

Implementation Plan §17 entrance. Current state is `0/2`.

| Entrance bullet | Current state |
| --- | --- |
| `0.1.0` used by external adopters | **Blocked.** Unpublished lockstep `0.1.0` exists and G7 is `Passed`. No external project depends on it. First-party starters do not count. |
| Preview telemetry, issue patterns, API pain points, and migration needs reviewed | **First-party review exists** at [`preview-feedback-review.md`](../../preview-feedback-review.md). That review is not external soak and does not make entrance `2/2`. |

Phase 8 is `Done` (entrance `2/2`, exit `4/4`). G7 is `Passed` via
`G7-D-public-preview-f7c7e70b9e04`. Do not infer Phase 9 entrance
from Phase 8 `Done`, from G7, or from the first-party review.

If the owner says `implement the plan` while Phase 9 entrance is
still `0/2`, **stop**. Do not fabricate adopter usage or preview
feedback. A local `0.1.0` candidate with no external use does
**not** satisfy bullet 1.

Phase 9 entrance, once Passed, is recorded by the first admitted
Phase 9 PR. If an earlier Phase 9 PR already recorded
`PH9-E-entrance-*`, do not re-record it here.

### Phase 8 is `Done` and G7 is `Passed`

PR-055–PR-061 must be `Done`. `G7-D-*` must exist. Starters and
the PR-060/PR-061 staging path must exist so downstream RC tests
and checksum recreation have real artifacts. Keep one active
logical PR.

### PR-062 through PR-064 are `Done` unless the owner parallelizes

Implementation Plan lists this PR's dependencies as
**PR-062 through PR-064**. Do not admit PR-065 while any of
those is `Todo` unless the owner explicitly authorizes parallel
Phase 9 work in the same sentence. PR-065 does not freeze
contracts, ratify perf budgets, conduct the independent security
review, or cut `1.0.0`.

### Other admission checks

- Threat Model §18 is triggered for release automation, signing,
  provenance, and rollback (TM-18). Complete that review. It is
  not `G8-D-*`.
- No Implementation Plan section 6.3 ADR trigger applies if the
  work adds no seventh port, no kernel I/O, and no new durable
  meaning. A marketplace, registry, or dylib loader would be an
  ADR; all three stay excluded.
- PR-062's `1.0-compatibility-policy.md` and
  `1.0-leaf-versioning.md` must exist (or this PR waits). Support
  windows reference those files; they do not replace them.
- Do not restore `tools/architecture/`. Do not invent
  `mise run schema-governance`. Do not add a Cargo `xtask`.
- `mise run conformance` is named in historical docs and is
  **absent** from current root `mise.toml`. Add a thin task only
  when a direct one-liner cannot express the published suites.
  Do not invent a second conformance harness.

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
  suite/engine version (workspace `0.1.0` at admit, or the
  frozen 1.0 suite id PR-062 published). A failure that says
  only "assert failed" is not enough.
- PR-065-A04: Rollback/hotfix rehearsal is completed. Stage a
  baseline artifact set and a patch-line set; document yank/
  retract per crates.io / PyPI / npm; prove a consumer can pin
  the previous checksum and that the hotfix stages with a new
  checksum. Do not yank, publish, or tag.

Principal changes that are not extra acceptance IDs, but are
required to prove the four bullets:

- Publish provider/toolset/store/plugin conformance suites and
  a compatibility-badge **process** (not a store).
- Finalize multi-package release automation, signing,
  provenance, rollback, and hotfix procedures.
- Add nightly/canary artifact **definitions** and starter RC
  tests.
- Define support windows and maintenance-branch procedure.

## Locked design

### Layout

```text
docs/site/conformance.md                         # NEW; suites + badge process
docs/site/support.md                             # NEW; adopter-facing windows
docs/implementation/release-engineering.md       # NEW; automation / sign / rollback
docs/implementation/support-windows.md           # NEW; operational register
docs/implementation/artifacts/pr-065/            # checksum pairs, hotfix rehearsal
crates/finstack-ai-test/src/port_conformance.rs  # add suite version to failures
plugins/finstack-ai-plugin-host/src/conformance_tests.rs
examples/rust-minimal/                           # consume staged RC
examples/python-minimal/
examples/browser-minimal/
examples/durable-interaction/
examples/ts-alpha-install/
.github/workflows/nightly.yml                    # NEW or restore; do not dispatch
```

Do not invent `starters/`. Do not invent a marketplace, plugin
registry, or badge server. Do not restore `tools/architecture/`.
Do not invent `mise run schema-governance`. Do not add a Cargo
`xtask`. Do not edit `docs/planning/*`.

### Conformance suites and badges (A03 + principal change 1)

Reuse, then publish. Do not write a second harness.

| Suite | Existing surface | Publish as |
| --- | --- | --- |
| Provider / Model | `check_model_conformance` | docs + mise entry |
| Toolset | existing tool port helpers | same |
| Store | journal/store conformance | same |
| Plugin | `conformance_tests.rs` (G6 hostile + lockfile) | same |
| Binding traces | `finstack-ai-test` / Python / WASM goldens | already shared |

`PortConformanceFailure` today names `port` and `contract`. Add
`suite_version` (or equivalent) so A03 holds. Plugin failures
must name world + host/engine version.

Badge process (documentation only):

- A first- or third-party package may claim
  `finstack-ai <port> conformance <suite_version>` only after
  the published suite passes on a named commit.
- The claim lists the suite version and the contract IDs.
- A failed run prints the violated contract and version
  (A03). There is no hosted badge issuer and no commercial
  listing.

Do not add a compatibility marketplace. Do not fetch suites
from a registry.

### Release automation, signing, provenance (principal change 2)

`docs/implementation/release-engineering.md` is the operational
register (same role as `compatibility-governance.md`). It names
the exact local commands, already used by PR-060/PR-061:

- crates: `cargo package --list` / metadata
- wheels/sdist: existing `uv build` / maturin path
- npm: `mise run stage-wasm`
- WIT: in-tree packages + `check-wit`
- SBOM / SHA-256 / provenance statement (commit, `mise.toml`
  pins, "not published")
- `cargo deny` for advisories/licenses/sources (ENG §9)

Signing under this envelope:

- Required locally: checksums over staged bytes.
- Hosted keyless / Sigstore / OIDC (the existing
  `npm-release-staging.yml` `id-token: write` sketch) is a
  **separately named** external action. Document the procedure.
  Do not publish signatures to a public log from this PR.

Two-run checksum identity is A01. `SOURCE_DATE_EPOCH=0` stays
the reproducibility pin where WASM already uses it.

No Cargo xtask. Thin mise tasks only.

### Nightly / canary and starter RC tests (A02 + principal change 3)

- Add `.github/workflows/nightly.yml` (schedule +
  `workflow_dispatch`) that builds canary staging and runs the
  starter RC task. Do not dispatch it under
  `external actions=none`.
- Canary artifacts stay unpublished (`staging/canary/` or the
  existing staging directory with a canary label). They must
  not claim `1.0.0`.
- Starter RC task: install/build each in-tree starter against
  staged crates/wheel/npm; run the existing smoke (health /
  typecheck / scripted run). Fail if a starter still path-maps
  around the RC.
- Experimental starters (IndexedDB) stay labeled experimental
  and are not a 1.0 badge surface.

### Support windows and maintenance branches (principal change 4)

`docs/implementation/support-windows.md` + `docs/site/support.md`:

| Line | Window (lock; do not invent LTS) |
| --- | --- |
| Current minor (`1.0.x` after PR-066) | Patches until the next minor |
| Previous minor | Security-only until the next minor ships or 90 days, whichever is shorter |
| `0.1.x` preview | Supported until `1.0.0` per the PR-061 preview policy; then security-only for 90 days |
| Historical snapshots | Not supported |

Maintenance-branch **procedure** (do not create or push
`release/1.0` in this PR):

1. At GA (PR-066), cut `release/1.0` from the tagged commit.
2. Hotfix: cherry-pick onto that branch, bump patch, restage,
   recreate checksums (A01/A04).
3. Do not merge unrelated Phase 9 work into a maintenance
   branch.

This PR writes the procedure and rehearses a **local** patch
line (A04). Remote branch creation is a separately named
external action.

### Rollback / hotfix rehearsal (A04)

Record under `docs/implementation/artifacts/pr-065/`:

1. Stage baseline set B; write `SHA256SUMS-B`.
2. Apply a no-op or docs-only patch on a local branch; stage
   set H; write `SHA256SUMS-H`.
3. Assert B ≠ H and that a documented consumer pin of B still
   verifies.
4. Document yank/retract commands per registry **without
   running them**.
5. Document "rollback" as: consumers stay on B; maintainers
   publish H as the next patch. Do not rewrite published
   bytes.

### Graph and version

No new release-orchestrator crate in kernel/runtime/default
SDK. Workspace version stays whatever PR-061 left (`0.1.0`);
do not bump to `1.0.0`. Staged canary labels may say
`0.1.0-canary` or `1.0.0-dev` only if preview feedback already
chose that; default is `0.1.0-canary`.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 9 entrance `Passed` (2/2), Phase 8
   `Done`, `G7-D-*` exists, PR-062–PR-064 `Done` unless
   parallelized; open
   `codex/pr-065-ecosystem-conformance-release-eng` from the
   then-current `main` tip. Mark PR-065 `In progress`. Do not
   re-record Phase 9 entrance if an earlier Phase 9 PR did.
2. Suite version on conformance failures; publish
   `docs/site/conformance.md` + badge process (A03).
3. `release-engineering.md`; two-run checksum recreation (A01).
4. Starter RC mise task + nightly workflow file (A02). Do not
   dispatch hosted nightly.
5. Support windows + maintenance-branch procedure.
6. Rollback/hotfix rehearsal notes (A04). TM-18 review. Stop
   before `G8-D-*` and before any tag.

## Explicit exclusions

No centralized commercial plugin marketplace. No plugin
registry download. No `1.0.0` cut or announce. No G8
inference. No `git tag`. No registry publish, yank, or hosted
signing. No fabricated nightly pass. No Cargo xtask. No second
conformance harness. No `starters/` tree. No LTS invention
beyond the table above.

## Validation

Use checked-in tasks and direct commands. Do not assume a
historical task still exists.

- Port/plugin conformance runs that print contract + version
  on a deliberate failure
- Two-run staging checksum identity (A01)
- Starter RC task against staged artifacts (A02)
- Hotfix rehearsal SHA-256 pair (A04)
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check` after the candidate is otherwise green

Do not require hosted nightly, Sigstore publication, Temporal,
or `mise run ci` on a multi-OS matrix unless separately named.

## Suggested authorization sentence

When Phase 9 entrance is `Passed` (2/2), Phase 8 is `Done`,
`G7-D-*` exists, PR-062 through PR-064 are `Done` unless
parallelized, and the owner is ready:

```
Run PR-065; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`,
`G7-D-*`, `G8-D-*`, dispatch hosted nightly, publish/sign/yank,
create remote maintenance branches, or cut `1.0.0`. Do not
infer those from `implement the plan`.
