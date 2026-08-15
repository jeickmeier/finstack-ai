# PR-066 execution plan

Date: 2026-08-15
Owner: me@jeickmeier.com
Branch: `codex/pr-066-1.0.0-ga`
Baseline: local `main` at `04962c743d2f03d59b74feb9c71f872cccfcc003`
Plan baseline: documentation pack v0.21 / PLAN-0.19 / Implementation Plan SHA-256
`86d2430860b12ab947638052b97ea2403c2e53b218a66977a4258defeb18fc3f`

This file is the execution contract for PR-066. The closed PR-054
through PR-065 envelopes are not reused. PR-066 is the only active
logical PR. This file does not record `PH9-E-entrance-*`, write
`G8-D-*`, cut `v1.0.0`, or publish registries.

## Execution envelope

Authorized by the owner sentence `Proceed to PR-066`, read as:

```
Run PR-066; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

That envelope authorizes the local `1.0.0` version bump, docs,
staging, soak-evidence binding (when real), and the G8 readiness
pack. It does **not** authorize `G8-D-*`, a git tag, push, hosted
PR/merge, registry publish, GitHub Release, or announce.

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden unless a **later, separate sentence** names them: push,
hosted PR/merge, npm/pypi publish, crates.io publish, git tag,
GitHub Release, announce, G8 inference. Do not write `G8-D-*`.
Do not start a successor PR. Do not invent adopter soak,
marketplace, or a G8 pass.

A04 and logical-PR `Done` wait on those separately named actions.

## Admission

All admission checks were true at admit. Phase 9 entrance stays
`Passed` (2/2) via `PH9-E-entrance-tag-b610b0ba93b5` and
`PH9-E-entrance-feedback-ee6999c59a12` and is not re-recorded.

| Check | State |
| --- | --- |
| Phase 9 entrance | Passed (2/2); do not remint `PH9-E-entrance-*` |
| Phase 8 Done; G0–G7 Passed | `G5-D-durable-beta-a9568bd869b5`; `G7-D-public-preview-f7c7e70b9e04` |
| PR-062–PR-065 Done | `COMP-1.0-D-contract-freeze-00b78667ecc4` and successor evidence |
| Open Critical/High | None |
| Exceptions register | Empty; no Expired `EX-*`; do not invent `EX-*` |
| Seventh port / kernel I/O / marketplace | None |
| TM-18 | Triggered for GA publish/credentials/announce; complete the review; not `G8-D-*` |

## Traceability

Implementation Plan PR-066, §17 exit, §18, §21 program
completion 1–12; Engineering Standards G8; Threat Model §13.2
G8 row; PRD §15 release acceptance; NFR-COMP / NFR-SEC /
NFR-PERF / NFR-REL as already evidenced by PR-062–PR-065.
G8 is this phase's gate. Writing `G8-D-*` is a **named owner
action**, not an inference from the local candidate.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-066-A01: Verify Implementation Plan §21 items 1–11 against
  existing evidence (do not re-run the program). Write
  `docs/implementation/artifacts/pr-066/g8-readiness-review.txt`
  as `READY FOR NAMED DECISION`. Do not write `G8-D-*`. Item 12
  (external preview users validated the `1.0.0` migration path)
  is a soak gap; do not claim A01 Passed while that gap remains.
- PR-066-A02: No critical release blocker remains open. Findings
  register has zero `Open` Critical/High. No `Expired`
  exception blocks G8. No unreviewed public API change landed
  during soak.
- PR-066-A03: After the bump, public docs and in-tree starters
  install `1.0.0` (crates / wheel / npm / WIT `@1.0.0`).
  Migration prose may name `0.1.0` as the **from** version.
  Path-mapped unpublished workspace deps in adopter-facing docs
  fail this bullet.
- PR-066-A04: Named `G8-D-*` plus tag, publish, and announce.
  This envelope **stops** before it. Logical-PR `Done` waits on
  A04.

Honest expected disposition after the local candidate:

| ID | Status |
| --- | --- |
| A01 | Pending (soak / §21.12 gap; pack written) |
| A02 | Passed if no Critical/High and no expired `EX-*` |
| A03 | Passed after bump + doc/example refresh |
| A04 | Pending |
| PR-066 | In progress even if locally merged; not Done |

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
SECURITY.md                                     # supported: 1.0.x candidate
docs/site/migration.md                          # 0.1.0 → 1.0.0
docs/site/support.md                            # PR-065 windows; in force at GA
docs/implementation/1.0-compatibility-matrix.md # NEW
docs/implementation/public-ga-roadmap.md        # NEW; post-1.0 in-repo only
docs/implementation/artifacts/pr-066/           # readiness, soak gap, staging
plugins/finstack-ai-wit/wit/v1.0.0/             # already from PR-062; ship
examples/*/                                     # install 1.0.0, not path maps
```

Do not invent `starters/` or a marketplace. Do not restore
`tools/architecture/`. Do not invent `mise run schema-governance`.
Do not add a Cargo `xtask`. Do not edit `docs/planning/*`.

### Version and packages

Lockstep first-party crates, the Python wheel, and `@finstack/ai`
move to `1.0.0`. WIT worlds ship as `finstack:ai-*@1.0.0` (PR-062
already generated them). Lift the plugin-crate `1.0.0` reservation
in `tools/wit_bindgen/generate.py` and
`plugins/finstack-ai-wit/src/inventory.rs`. Experimental surfaces
stay labeled and get **no** permanent promise.

Do not independently version leaves. Preview feedback found no
coupling harm.

### RC soak (principal change 1)

Bind a written soak note that names external adopter projects,
RC artifact identity, the observed soak window, and "no
unreviewed public API change". If that note cannot name external
adopters, record the gap and **stop** before claiming A01/A04.
Do not fabricate adopters. In-tree starters are not external
adopters.

### Docs (principal change 3 + A03)

- `docs/site/migration.md` — `0.1.0` → `1.0.0` commands
- Compatibility matrix — frozen families vs experimental
- Support policy — PR-065 windows; in force for `1.0.x` at GA
- Roadmap — post-1.0 in-repo only; marketplace, channel catalog,
  and product UIs stay **excluded**

Grep adopter-facing docs/examples for leftover current-version
`0.0.4` / unpublished path installs. `0.1.0` may remain only as
a migration source.

### Staging vs publish (principal change 2)

Local staging (in envelope), same commands as PR-065. Record
under `docs/implementation/artifacts/pr-066/`.
`SOURCE_DATE_EPOCH=0` stays the pin.

Registry publish, `git tag v1.0.0`, GitHub Release upload, and
announce wait for a sentence that names those external actions.

### G8 readiness pack (A01, not A04)

`docs/implementation/artifacts/pr-066/g8-readiness-review.txt`:

- Phase 9 entrance 2/2 and PR-062–PR-065 `Done`
- `COMP-1.0-D-*`, PR-063 budgets/`EX-*`, PR-064 closeout,
  PR-065 rehearsal
- §21 items 1–11 evidenced; item 12 only if soak note exists
- exceptions disposition
- Result: `READY FOR NAMED DECISION`; `G8-D-*` is not recorded

## Tasks

| ID | Owns |
| --- | --- |
| PR-066-T-tracking-c793124ecbe1 | Admit / branch |
| PR-066-T-bump-60516f6d166f | Lockstep `1.0.0` + CHANGELOG + SECURITY (A03) |
| PR-066-T-docs-3d50e77113c7 | Migration / matrix / support / roadmap |
| PR-066-T-stage-09c20345b29c | Stage `1.0.0` + two-run checksums |
| PR-066-T-soak-d471877bcf40 | Bind soak if real; otherwise record the gap |
| PR-066-T-g8-6fec55d4501a | G8 readiness pack; stop before `G8-D-*` / tag / publish |

## Explicit exclusions

Post-1.0 marketplace, broad channel catalog, and product-
specific UIs. No fabricated soak or G8 pass. No `G8-D-*` from
this envelope. No tag/publish/announce under
`external actions=none`. No seventh port. No planning-file
edits. No Cargo xtask.

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
