# PR-066 execution plan

Date: 2026-08-15
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-066-1.0.0-ga`
Intended baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-066. The closed PR-054
envelope is not reused. The PR-055–PR-065 envelopes are not reused.
PR-066 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8 or Phase 9, cut `0.1.0`
or `1.0.0`, or record G5 / G7 / G8.

## Execution envelope

Not authorized. Suggested text when the owner is ready to prepare
the local `1.0.0` candidate only:

```
Run PR-066; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. That envelope authorizes the `1.0.0` version bump, docs,
staging, soak-evidence binding (when real), and the G8 readiness
pack. It does **not** authorize `G8-D-*`, a git tag, push, hosted
PR/merge, registry publish, GitHub Release, or announce.

Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, from Phase 8
or Phase 9 plans existing, or from a phase name.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden unless a **later, separate sentence** names them: push,
hosted PR/merge, npm/pypi publish, crates.io publish, git tag,
GitHub Release, announce, G5 inference, G7 inference, G8
inference. Do not write `G5-D-*`, `G7-D-*`, or `G8-D-*` from
`implement the plan`. Do not start PR-055–PR-065. Do not invent
adopter soak, marketplace, or a G8 pass.

A04 and logical-PR `Done` wait on those separately named actions.
See Acceptance mapping.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 9 entrance is `Passed` (2/2)

Implementation Plan §17 entrance. Current state is `0/2`.

| Entrance bullet | Current state |
| --- | --- |
| `0.1.0` used by external adopters | **Blocked.** Workspace is unpublished `0.0.4`. PR-061 has not cut `0.1.0`. No adopter evidence exists. |
| Preview telemetry, issue patterns, API pain points, and migration needs reviewed | **Blocked.** No preview-feedback review artifact exists. |

Phase 8 is `In progress` (entrance `2/2`, exit `0/4`). G5 and G7
are `Not ready`. Do not infer Phase 9 entrance from Phase 6/7
`Done`, from G6, or from the PR-055–PR-065 plans.

If the owner says `implement the plan` while Phase 9 entrance is
still `0/2`, **stop**. Do not fabricate adopter usage, RC soak,
or preview feedback. A local `1.0.0` candidate with no external
RC use does **not** satisfy principal change 1 or program
completion criterion 12.

Phase 9 entrance, once Passed, is recorded by the first admitted
Phase 9 PR. Do not re-record `PH9-E-entrance-*` here.

### Phase 8 is `Done` and prior gates are `Passed`

PR-055–PR-061 must be `Done`. `G5-D-*` and `G7-D-*` must exist.
G0–G4 and G6 are already `Passed`. Keep one active logical PR.

### PR-062 through PR-065 are `Done` unless the owner parallelizes

Implementation Plan lists this PR's dependencies as
**PR-062 through PR-065**. Do not admit PR-066 while any of
those is `Todo` unless the owner explicitly authorizes parallel
Phase 9 work in the same sentence. Required predecessor
artifacts at admit:

| Predecessor | Must exist |
| --- | --- |
| PR-062 | Frozen 1.0 contracts, WIT `@1.0.0`, `COMP-1.0-D-*` |
| PR-063 | Ratified NFR-PERF budgets or approved unexpired `EX-*` |
| PR-064 | Independent review closed; no `Open` Critical/High |
| PR-065 | Checksum recreation, starter RC tests, support windows, rollback rehearsal |

### Other admission checks

- Threat Model §18 **is** triggered (GA publish, credentials,
  announce). Complete that review. It is not `G8-D-*`.
- No Implementation Plan section 6.3 ADR trigger applies if the
  cut adds no seventh port, no kernel I/O, and no new durable
  meaning. An unreviewed public API change during soak restarts
  the soak and needs the PR-062 compatibility process.
- Exceptions register: Engineering Standards waivers must be
  `Closed` before G8. An NFR-PERF `EX-*` that the PRD/§18.2
  sentence already allows at G8 may remain `Approved` and
  unexpired. Do not invent either.
- Do not restore `tools/architecture/`. Do not invent
  `mise run schema-governance`. Do not add a Cargo `xtask`.
- Do not edit `docs/planning/*`.

## Traceability

Implementation Plan PR-066, §17 exit, §18, §21 program
completion 1–12; Engineering Standards G8; Threat Model §13.2
G8 row; PRD §15 release acceptance; NFR-COMP / NFR-SEC /
NFR-PERF / NFR-REL as already evidenced by PR-062–PR-065.
G8 is this phase's gate. Writing `G8-D-*` is a **named owner
action**, not an inference from the local candidate.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-066-A01: All 1.0 acceptance, security, compatibility, and
  performance gates pass. Verify Implementation Plan §21 items
  1–11 against existing evidence (do not re-run the program).
  G8 evidence inputs: `COMP-1.0-D-*`, PR-063 budgets or `EX-*`,
  PR-064 review closeout, PR-065 rehearsal, G0–G7 decisions.
  Write a G8 readiness pack (`READY FOR NAMED DECISION`). Do
  not write `G8-D-*` from the local envelope.
- PR-066-A02: No critical release blocker remains open. Findings
  register has zero `Open` Critical/High. No `Expired`
  exception blocks G8. No unreviewed public API change landed
  during soak.
- PR-066-A03: Documentation and examples reference only
  released package versions. After the bump, public docs and
  in-tree starters install `1.0.0` (crates / wheel / npm / WIT
  `@1.0.0`). Migration prose may name `0.1.0` as the **from**
  version. Path-mapped unpublished workspace deps in adopter-
  facing docs fail this bullet.
- PR-066-A04: Release `1.0.0` and gate G8 are approved. This
  bullet is the named `G8-D-*` plus tag, publish, and announce.
  The local envelope **stops** before it. Logical-PR `Done`
  waits on A04.

Principal changes that are not extra acceptance IDs, but are
required to prove the four bullets:

- RC soak with **named** external adopters and no unreviewed
  API changes (also §21 item 12).
- Stage (local) then publish (separately named) crates, wheels,
  npm/WASM, WIT, server/client packages, fixtures, SBOMs,
  checksums, and benchmark/security reports.
- Publish the `1.0.0` migration guide, compatibility matrix,
  support policy, and roadmap (refresh PR-062/PR-065 pages to
  released versions).
- Tag and announce GA only after G8 is signed off.

## Locked design

### Envelope split (same shape as PR-061 A05)

| Step | Authorized by |
| --- | --- |
| `1.0.0` lockstep bump, CHANGELOG, docs, examples | Local envelope |
| Stage artifacts + two-run checksums (PR-065 path) | Local envelope |
| Bind real RC soak evidence if it already exists | Local envelope |
| G8 readiness pack `READY FOR NAMED DECISION` | Local envelope |
| `G8-D-*`, `git tag v1.0.0`, registry publish, GitHub Release, announce | Later named sentence |
| Phase 9 exit "1.0.0 artifacts released" | After A04 |

### Layout

```text
Cargo.toml / workspace.package.version          # 0.1.0 → 1.0.0
bindings/finstack-ai-python/pyproject.toml
bindings/finstack-ai-wasm/js/package.json
CHANGELOG.md                                    # [1.0.0]
SECURITY.md                                     # supported: 1.0.x
docs/site/migration.md                          # 0.1.0 → 1.0.0 released
docs/site/support.md                            # PR-065 windows, now GA
docs/implementation/1.0-compatibility-matrix.md # NEW; released families
docs/implementation/public-ga-roadmap.md        # NEW; post-1.0 in-repo only
docs/implementation/artifacts/pr-066/           # readiness, soak, staging
plugins/finstack-ai-wit/wit/v1.0.0/             # already from PR-062; ship
examples/*/                                     # install 1.0.0, not path maps
```

Do not invent `starters/` or a marketplace. Do not restore
`tools/architecture/`. Do not invent `mise run schema-governance`.
Do not add a Cargo `xtask`. Do not edit `docs/planning/*`.

### Version and packages

Lockstep first-party crates, the Python wheel, and `@finstack/ai`
move to `1.0.0`. WIT worlds ship as `finstack:ai-*@1.0.0` (PR-062
already generated them). Experimental surfaces (IndexedDB,
anything the 1.0 policy left experimental) stay labeled and get
**no** permanent promise.

Do not independently version leaves in this PR unless PR-062's
leaf-versioning policy plus preview feedback already split them.

### RC soak (principal change 1)

Bind a written soak note that names:

- external adopter projects (same honesty bar as Phase 9
  entrance; a local-only RC is not enough);
- RC artifact identity (commit / staged checksums);
- soak window actually observed (do **not** invent a 30-day
  requirement if the feedback review named a different one);
- "no unreviewed public API change" attestation.

If that note does not exist, prepare the candidate and **stop**
before claiming A01/A04. Do not fabricate adopters.

An API change during soak: classify under PR-062 policy, update
fixtures, restart soak. Silent breakage fails A02.

### Docs (principal change 3 + A03)

Refresh, do not fork a second docs root:

- `docs/site/migration.md` — released `0.1.0` → `1.0.0` commands
  (PR-062 tooling).
- Compatibility matrix — one table of frozen families vs
  experimental, with package versions `1.0.0` / WIT `@1.0.0`.
- Support policy — PR-065 windows, now in force for `1.0.x`.
- Roadmap — post-1.0 in-repo only: marketplace, channel catalog,
  and product UIs stay **excluded** (this PR's explicit
  exclusion). Do not promise them.

Grep adopter-facing docs/examples for leftover `0.0.4` /
unpublished path installs. `0.1.0` may remain only as a
migration source.

### Staging vs publish (principal change 2)

Local staging (in envelope), same commands as PR-065:

- crates, wheels/sdist, `mise run stage-wasm`, WIT, fixtures
- SBOMs, SHA-256, provenance (commit, `mise.toml` pins)
- benchmark/security reports already produced by PR-063/PR-064
- two-run checksum identity

Registry publish, `git tag v1.0.0`, GitHub Release upload, and
announce wait for a sentence that names those external actions.
Do not `git tag` under `external actions=none`.

### G8 readiness pack (A01, not A04)

`docs/implementation/artifacts/pr-066/g8-readiness-review.txt`:

- Phase 9 entrance 2/2 and PR-062–PR-065 `Done`
- `COMP-1.0-D-*`, PR-063 budgets/`EX-*`, PR-064 closeout,
  PR-065 rehearsal
- §21 items 1–11 evidenced; item 12 only if soak note exists
- exceptions disposition
- Result: `READY FOR NAMED DECISION`; `G8-D-*` is not recorded

### Graph

No new crate in kernel/runtime/default SDK. Default SDK stays
free of rustls/otel/wasmtime/network providers unless a feature
is selected.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 9 entrance `Passed` (2/2), Phase 8
   `Done`, `G5-D-*` / `G7-D-*` / `COMP-1.0-D-*` exist,
   PR-062–PR-065 `Done` unless parallelized; open
   `codex/pr-066-1.0.0-ga` from the then-current `main` tip.
   Mark PR-066 `In progress`. Do not re-record Phase 9 entrance.
2. Lockstep bump to `1.0.0`; CHANGELOG; SECURITY.md (A03).
3. Migration / matrix / support / roadmap at released versions
   (principal change 3).
4. Stage `1.0.0` artifacts; two-run checksums.
5. Bind soak evidence if real; otherwise record the gap and
   stop before A01/A04 claims.
6. G8 readiness pack. Stop before `G8-D-*`, tag, publish, and
   announce.

## Explicit exclusions

Post-1.0 marketplace, broad channel catalog, and product-
specific UIs. No fabricated soak or G8 pass. No `G8-D-*` from
`implement the plan`. No tag/publish/announce under
`external actions=none`. No seventh port. No planning-file
edits. No Cargo xtask. No new battery that belongs to
PR-055–PR-065.

## Validation

- Grep: no leftover current-version `0.0.4` / path-mapped
  installs in adopter-facing docs and examples
- Workspace/Python/JS versions are `1.0.0`; WIT `@1.0.0`
- Two-run staging checksum identity
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check` after the candidate is otherwise green
- Readiness pack cites existing PR-062–PR-065 evidence IDs

Do not require hosted publish, Sigstore, Temporal, or
`mise run ci` on a multi-OS matrix unless separately named.

## Suggested authorization sentences

When Phase 9 entrance is `Passed` (2/2), Phase 8 is `Done`,
prior gates including `G5-D-*` / `G7-D-*` / `COMP-1.0-D-*`
exist, PR-062 through PR-065 are `Done` unless parallelized,
and the owner is ready to prepare the candidate:

```
Run PR-066; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G8-D-*`,
`git tag v1.0.0`, publish crates/wheels/npm/WIT, upload a
GitHub Release, and announce GA. Do not infer those from
`implement the plan`.
