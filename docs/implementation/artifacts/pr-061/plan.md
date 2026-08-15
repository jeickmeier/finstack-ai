# PR-061 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-061-public-preview-010`
Intended baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-061. The closed PR-054
envelope is not reused. The PR-055–PR-060 envelopes are not reused.
PR-061 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8, cut `0.1.0`, or record
G7.

## Execution envelope

Not authorized. Suggested text when the owner is ready to prepare
the local `0.1.0` candidate only:

```
Run PR-061; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. That envelope authorizes the version bump, suites, compatibility
policy, roadmap, and G7 readiness pack. It does **not** authorize
`G7-D-*`, a git tag, push, hosted PR/merge, or registry publish.

Do not infer authorization from this planning file, from `continue`,
from Phase 7 `Done`, from G6 `Passed`, or from the PR-055–PR-060
plans existing.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden unless a **later, separate sentence** names them: push,
hosted PR/merge, npm/pypi publish, crates.io publish, git tag,
GitHub release, G5 inference, G7 inference. Do not write `G5-D-*`
or `G7-D-*` from `implement the plan`. Do not start PR-055–PR-060
or PR-062+. Do not generate WIT `@1.0.0` worlds.

A05 and logical-PR `Done` wait on those separately named actions.
See Acceptance mapping.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 8 entrance is `Passed` (2/2)

Recorded by PR-055. Current state is `Passed` (2/2).

| Entrance bullet | Current state |
| --- | --- |
| Native preview, Python alpha, WASM alpha, durability beta, plugin alpha | **Satisfiable.** G3/G4/G5/G6 are named (`G5-D-durable-beta-a9568bd869b5`). Do not record `PH8-E-entrance-gates-*` until PR-055 is admitted. |
| Public API change backlog triaged | **Blocked.** `docs/implementation/public-api-change-backlog.md` does not exist. |

Do not infer G5 from Phase 6 `Done`. Do not write the backlog or
`G5-D-*` from this PR. If the owner says `implement the plan` while
entrance is still `0/2`, **stop**.

Phase 8 entrance, once Passed, is recorded by the first admitted
Phase 8 PR. Do not re-record `PH8-E-entrance-*` here.

### PR-055 through PR-060 are `Done`

All six are planned only and still `Todo`. Implementation Plan
lists them as dependencies ("PR-055 through PR-060 and all previous
gates"). Keep one active logical PR. Do not admit PR-061 while any
predecessor is `Todo` unless the owner explicitly authorizes
parallel Phase 8 work in the same sentence.

Implementation Plan §16 contingencies are binding: do **not** label
the tree `0.1.0` or prepare G7 if isolated WIT plugins, the remote
reference server, the third provider path, or shell hardening are
missing. Those gaps require a versioned PRD/plan amendment, not a
silent scope cut.

### Other admission checks

- G3, G4, and G6 are already named. G5 must be named before
  entrance. G7 stays `Not ready` until a separately named
  `G7-D-*` exists.
- ADR-035 stays experimental `@0.x` WIT. Do not freeze `@1.0.0`
  worlds (PR-062 / framework `1.0.0`). ADR-013 stays at-least-once;
  do not claim exactly-once.
- Preview-blocking rows on
  `docs/implementation/public-api-change-backlog.md` (created
  before entrance) must be `keep` or `change before preview`. A
  remaining `change before preview` row blocks this PR.
- No Implementation Plan section 6.3 ADR trigger applies if the
  work only publishes compatibility **scope** (not a new port,
  stage, kernel I/O, dylib loader, or stronger-than-at-least-once
  claim). Changing journal/WIT/remote compatibility **policy**
  itself still needs an ADR.
- Threat Model section 18 is triggered (release identities,
  registries, signing, provenance). Primary **TM-18**. Complete
  the review before merge. The review is not `G7-D-*`.

## Traceability

Implementation Plan PR-061 and Phase 8 exit; PRD §15 (all four
subsections) and public-preview scope in §2.1 / plan §2.1; TDD
§36; Engineering Standards §15 G7; Threat Model §13.2 G7 and
§12; ADR-020 (three activation modes); ADR-024; ADR-035.
G8 / PR-062+ are out of scope.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-061-A01: All PRD public-preview acceptance criteria pass.
  Walk PRD §15.1–15.4 and TDD §36 items 1–13 against evidence
  already on `main` plus PR-055–PR-060. Record a checklist in
  `docs/implementation/artifacts/pr-061/prd-15-tdd-36-review.txt`.
  A missing row is a blocker, not a skip. Live provider smokes
  stay `#[ignore]` unless a separate sentence names network use.
- PR-061-A02: Rust, Python, and WASM common traces are identical
  for shared features. Re-run the existing cross-binding
  conformance / golden-trace set (`mise run conformance` and the
  binding trace jobs already used for G4). Do not invent a second
  fixture language. Shared features only; IndexedDB experimental
  and WIT `@0.x` stay labeled.
- PR-061-A03: No forbidden kernel dependency or unbounded queue
  exists. `tools/wasm_package/check.py graph` plus
  `cargo tree -p finstack-ai-kernel` show no async runtime, HTTP,
  database, provider SDK, CLI, telemetry exporter, Python, or
  Wasmtime. Existing slow-consumer / bounded-queue tests still
  pass. Do not restore `tools/architecture/`.
- PR-061-A04: All three capability activation modes pass shared
  public-preview traces; model activation remains append-only at
  safe checkpoints. Reuse PR-012 / PR-022 / PR-032 / PR-038
  fixtures for `Always`, `Application`, and `Model`. A second
  model-activation at a safe checkpoint must not rewrite the
  stable prompt prefix (PR-055 already owns provider-diversity
  of that prefix). Do not reshape the Model port.
- PR-061-A05: Release `0.1.0` and gate G7 are approved. Split:
  1. This PR prepares lockstep `0.1.0` metadata, release notes,
     staged unpublished artifacts, the preview compatibility
     policy, the in-repo roadmap, Phase 8 exit review, and
     `g7-readiness-review.txt` with language
     `READY FOR NAMED DECISION`.
  2. Named `G7-D-*` is a **separate owner action** (same pattern
     as `G6-D-plugin-alpha-018aaea9aa00`).
  3. Git tag `v0.1.0`, registry publish (crates.io, PyPI, npm),
     and a GitHub release are **separately named external
     actions**. They are required to finish the principal-change
     "Publish …" list, not implied by the local envelope.

  `implement the plan` with `external actions=none` **stops**
  after step 1. Do not write `G7-D-*` in the implementation or
  evidence commit.

## Locked design

### What this PR changes

This is a release/consolidation PR, not a new battery. It may
fix preview-blocking API/schema rows from the backlog. It must
not add providers, a seventh port, `@1.0.0` WIT worlds, or
Phase 9 migration tooling.

```text
Cargo.toml / workspace.package.version          # 0.0.4 → 0.1.0
bindings/finstack-ai-python/pyproject.toml      # 0.1.0
bindings/finstack-ai-wasm/js/package.json       # 0.1.0
CHANGELOG.md                                    # [0.1.0] section
SECURITY.md                                     # supported versions: 0.1.0
docs/implementation/preview-compatibility-policy.md   # NEW; published scope
docs/implementation/public-preview-roadmap.md         # NEW; in-repo roadmap
docs/implementation/public-api-change-backlog.md      # close preview-blocking rows
docs/site/migration.md                          # point at the new policy
docs/implementation/artifacts/pr-061/           # suites, reviews, staging
```

WIT packages stay experimental **`finstack:ai-toolset@0.0.4`** and
**`finstack:ai-context@0.0.4`**. Document the framework-`0.1.0` ↔
WIT-`@0.0.4` mapping in the compatibility policy. Do not run
`@1.0.0` world generation.

First-party lockstep crates, the Python wheel, and `@finstack/ai`
move to `0.1.0`. That is the cut this PR owns.

### Preview-blocking backlog (principal change 1)

At admit, read
`docs/implementation/public-api-change-backlog.md`. For every
row whose `0.1.0` disposition is `change before preview`, land
the change in this PR or block. `defer past preview` rows become
roadmap issues (in-repo). `keep` rows need no code.

Do not use this PR to invent new public surface. Prefer the
smallest fix that makes the published scope honest.

### Published compatibility scope

Create `docs/implementation/preview-compatibility-policy.md`.
This is the document Engineering Standards / Phase 8 exit mean
by "published compatibility policy". It does **not** promise 1.0
SemVer (explicitly excluded).

Minimum contents:

- Supported families: public Rust API, journal/snapshot
  candidate-v1, runtime events, AgentSpec/locks, Python wheel,
  JS/WASM, remote protocol, process handshake, experimental WIT
  `@0.0.4`, experimental plugin lockfile, experimental IndexedDB.
- Promise: preview is supported for security fixes on the `0.1.0`
  line; breaking changes before `1.0.0` still require
  classification, changelog, and fixtures
  (`compatibility-governance.md`). Candidate/experimental labels
  do not permit silent breakage.
- Journal meaning breaks still need an ADR + migration.
- WIT `@1.0.0` and independent leaf versioning wait for PR-062.
- Deprecation process: changelog `Deprecated` section, replacement
  named in the same change, removal not before `1.0.0` unless the
  row was never in the published preview scope.
- How to report a compatibility defect (SECURITY.md vs public
  roadmap).

`compatibility-governance.md` stays the operational registry.
The new file is the adopter-facing preview promise. Do not edit
`docs/planning/*`.

### Release suites (principal change 2)

Run and record, offline where the task already is:

| Suite | Command / evidence |
| --- | --- |
| Check + unit | `mise run check` / `mise run test` |
| Conformance / shared traces | `mise run conformance` + binding goldens |
| Crash-prefix | existing `finstack-ai-test` crash tests |
| Plugin | `mise run check-plugin-wasm` / `check-plugin-template` / `check-plugin-lock` |
| Security / supply chain | existing deny / secret / graph checks |
| Benchmarks | `mise run benchmark-smoke`; attach existing Criterion metadata, no hidden network latency |
| Kernel graph | `check.py graph` + `cargo tree -p finstack-ai-kernel` |
| Staging rehearsal | PR-060 two-run checksum path, now at `0.1.0` names |

Hosted Linux/macOS/Windows matrix and live-provider smokes are
**not** in the local envelope. If the owner later names hosted
CI, record it as `PR-061-E-hosted-*`. Do not claim it from a
local run.

### Staging vs publish (principal change 3)

Local staging (in envelope):

- crates: `cargo package` lists / metadata at `0.1.0`
- wheels/sdist: existing `uv build` path
- npm: `mise run stage-wasm`
- WIT source/bindings as already published in-tree (`@0.0.4`)
- protocol fixtures already under `fixtures/` / `schemas/`
- SBOMs, SHA-256 checksums, provenance statement naming the
  commit and `mise.toml` pins
- `CHANGELOG.md` `[0.1.0]` release notes

Registry publish, `git tag v0.1.0`, and GitHub Release upload
wait for a sentence that names those external actions. Do not
`git tag` under `external actions=none`.

### Roadmap and deprecation (principal change 4)

Write `docs/implementation/public-preview-roadmap.md`:

- known preview limitations (experimental WIT, experimental
  IndexedDB, inspect-not-continue session open, at-least-once,
  no marketplace, no 1.0 freeze)
- deferred backlog rows
- Phase 9 themes (PR-062–PR-066) as links to the Implementation
  Plan, not as admitted work

Opening GitHub issues is an external action. The in-repo file
satisfies "open the public issue roadmap" under
`external actions=none`. A later sentence may mirror rows to
GitHub.

### Phase 8 exit and G7 readiness

Phase 8 exit bullets this PR must evidence, without writing
`G7-D-*`:

| Exit | Evidence home |
| --- | --- |
| Core use cases have first-party examples and batteries | PR-055–PR-060 + starters |
| Remote protocol and reference server are usable | PR-058 |
| Security, benchmark, SBOM, and compatibility reports are published | PR-060 rehearsal + this PR's policy/SBOM/notes (local "published" = in-tree; registries wait) |
| `0.1.0` public-preview acceptance criteria are met | A01–A04 checklist |

Write `phase8-exit-review.txt` and `g7-readiness-review.txt`
(`READY FOR NAMED DECISION`). Do not mark Phase 8 `Done` in the
implementation commit. Closeout after local merge may record
exit `Passed` (4/4) and Phase 8 `Done` only after that review
exists at the integrated commit. That is not G7.

G7 minimum evidence (plan §7, Engineering Standards §15, TM
§13.2): ecosystem batteries, server, workflow adapter, docs,
implemented-control threat-model review (PR-060 matrix),
SBOM/provenance, vulnerability process, security deployment
guide, support scope, starter validation. Cite those IDs in the
readiness pack. Residual: no 1.0 guarantee; no independent
review (G8); registries unpublished until named.

### Graph and exclusions

Kernel / default SDK / wasm graphs stay free of the same
forbidden leaves as PR-055–PR-060. Version bump must not add
dependencies.

Do not restore `tools/architecture/`.
Do not invent `mise run schema-governance`.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 8 entrance `Passed` (2/2) and
   PR-055–PR-060 `Done`; confirm no contingency gap; open
   `codex/pr-061-public-preview-010` from the then-current
   `main` tip. Mark PR-061 `In progress`. Do not re-record
   Phase 8 entrance.
2. Close preview-blocking backlog rows (A01 support).
3. Lockstep `0.1.0` bump + CHANGELOG + SECURITY supported
   versions (A05 step 1).
4. Preview compatibility policy + in-repo roadmap + deprecation
   process (principal changes 1 and 4).
5. Re-run shared traces, kernel graph, queue bounds, three
   activation modes (A02, A03, A04).
6. PRD §15 / TDD §36 checklist, staging rehearsal at `0.1.0`
   names, Phase 8 exit review, G7 readiness pack (A01, A05
   step 1). Stop before `G7-D-*`, tag, and publish.

## Explicit exclusions

No 1.0 compatibility guarantee. No `@1.0.0` WIT worlds. No
Phase 9 contract freeze, independent versioning of leaves, or
migration CLI (PR-062). No marketplace. No commercial support
portal. No G5 inference. No G7 inference from the readiness
pack. No tag, registry publish, or GitHub release under the
default envelope. No planning-file edits.

## Validation

- `cargo tree -p finstack-ai-kernel --locked` — no forbidden
  kernel deps
- `uv run --no-project python tools/wasm_package/check.py graph`
- `mise run conformance`
- focused activation-mode traces (Always / Application / Model)
- `mise run check-plugin-template` / `check-plugin-lock`
- `mise run benchmark-smoke`
- two local `0.1.0` staging runs; checksums byte-identical
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check` after the candidate is otherwise green
- grep: no `0.0.4` left on lockstep crate/wheel/npm version
  fields (WIT `@0.0.4` package names **must** remain)

Do not require `mise run ci` on a multi-OS hosted matrix,
Playwright beyond existing browser tasks, a Temporal server,
or Criterion perf budgets (those are PR-063).

## Suggested authorization sentences

When Phase 8 entrance is `Passed` (2/2), PR-055–PR-060 are
`Done`, and the owner is ready to prepare the candidate:

```
Run PR-061; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Later, separate sentences are required to record `G5-D-*` (before
any Phase 8 admit), triage the public API change backlog, admit
PR-055–PR-060, record `G7-D-*`, tag `v0.1.0`, or publish to
crates.io / PyPI / npm. Do not infer those from
`implement the plan`.
