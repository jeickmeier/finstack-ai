# PR-062 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Active branch: `codex/pr-062-1.0-contract-freeze`
Baseline: local `main` at `1775eab425ef9d48f93df0f0248d77bfda706331`
Plan baseline: documentation pack v0.21 / PLAN-0.19 / Implementation Plan SHA-256
`86d2430860b12ab947638052b97ea2403c2e53b218a66977a4258defeb18fc3f`

This file is the execution contract for PR-062. The closed PR-054
envelope is not reused. The PR-055–PR-061 envelopes are not reused.
PR-062 is the only active logical PR. This planning file does not
cut `1.0.0` or record G8.

## Execution envelope

Authorized 2026-08-15; **PR-062 admitted**. Owner text:

```
Skip this entrance gate, I don't want this to be at public
registries yet. Move the phase 9
```

Interpreted as: versioned PLAN-0.19 amendment; Phase 9 entrance
`Passed` (2/2); admit PR-062 only; no crates.io / PyPI / npm
publish; no PR-063–PR-066 range; no `G8-D-*`.

```
Run PR-062; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, from Phase 8
plans existing, or from a phase name.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G5 inference, G7 inference, G8 inference. Do not
write `G5-D-*`, `G7-D-*`, or `G8-D-*`. Do not start PR-055–PR-061
or PR-063+. Do not cut or publish `1.0.0` (PR-066). Do not invent
external-adopter feedback.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 9 entrance is `Passed` (2/2)

Implementation Plan §17 entrance under PLAN-0.19. Recorded here.

| Entrance bullet | Current state |
| --- | --- |
| Tagged `v0.1.0` exists; public registries not required | **Passed** via `PH9-E-entrance-tag-b610b0ba93b5` |
| Preview telemetry, issue patterns, API pain points, and migration needs reviewed | **Passed** via `PH9-E-entrance-feedback-ee6999c59a12` |

Phase 8 is `Done` (entrance `2/2`, exit `4/4`). G7 is `Passed` via
`G7-D-public-preview-f7c7e70b9e04`. Do not re-record `PH9-E-entrance-*`.
Do not invent external adopters. Do not publish to crates.io / PyPI /
npm. Do not write `G8-D-*`.

### Phase 8 is `Done` and G7 is `Passed`

PR-055–PR-061 must be `Done`. `G7-D-*` must exist. PR-061's
preview compatibility policy and in-repo roadmap must exist.
Keep one active logical PR. Do not admit PR-062 while Phase 8
is `Todo` unless the owner explicitly authorizes parallel Phase 8
and Phase 9 work in the same sentence (that would still not
satisfy "used by external adopters").

### Other admission checks

- ADR-035 mapped delivery includes this PR. `@1.0.0` world
  generation is **this PR's job** once admitted. Resource-based
  streaming stays deferred. A superseding ADR is required only if
  post-plugin-alpha evidence shows the coarse `@0.x` contract
  cannot become `@1.0.0` without a new world shape.
- ADR-013 stays at-least-once. Do not claim exactly-once.
- No Implementation Plan section 6.3 ADR trigger applies if the
  freeze adds no seventh port, no eighth middleware stage, no
  kernel I/O, and no dylib loader. Changing journal/WIT/remote
  **meaning** still needs an ADR + fixtures in the same change.
- Threat Model section 18 is triggered (WIT compatibility policy,
  public-contract freeze). Primary **TM-08**. Complete the review
  before merge. That review is not `G8-D-*`.
- Independent first-party versioning is allowed only where preview
  feedback shows lockstep coupling harm. Default: core
  kernel/runtime/SDK/protocol/test stay lockstep through the
  `1.0.0` cut (PR-066).

## Traceability

Implementation Plan PR-062; Phase 9 entrance/exit (contracts
frozen is an exit bullet this PR evidences, not G8);
NFR-COMP-001–003; Engineering Standards §6; TDD §27.1 / §28.4 /
§36 item 10; ADR-035; PRD packaging (post-1.0 leaf decoupling
needs published engine-compatibility ranges).
G8 is Phase 9's gate and is out of scope. PR-066 cuts `1.0.0`.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-062-A01: Breaking-change tests detect incompatible
  public/schema changes. For every Engineering Standards §6.1
  family that is in the 1.0 freeze set, a deliberate incompatible
  mutation (unknown state-bearing field, renamed journal member,
  event-order swap, WIT world rename, public Rust/Python/JS
  signature break) fails CI. Reuse `fixtures/compatibility/` and
  the schema-change template. Do not invent
  `mise run schema-governance`. Prefer fixture/negative tests
  already in-tree; add `cargo-semver-checks` or equivalent only
  if it stays offline and MSRV-compatible. Silent semantic
  discard remains prohibited (NFR-COMP-003).
- PR-062-A02: Every `0.1.0` supported project has a documented
  `1.0.0` migration path. Supported means: in-tree starters plus
  any external adopter named in the Phase 9 entrance review.
  Paths live in `docs/site/migration.md` (updated) and
  per-surface notes (journal, AgentSpec, WIT guest retarget,
  remote protocol). A project that used only an **experimental**
  surface (IndexedDB, `@0.x` WIT after freeze, process session
  vocabulary if still later) is excluded from the permanent
  promise and is labeled as such.
- PR-062-A03: Deprecated APIs carry removal versions. Every
  `#[deprecated]` / Python warning / JS `@deprecated` that remains
  in the 1.0 surface names `since` and a removal version
  (`2.0.0` or a dated 1.x). New deprecations come from the
  preview-feedback review, not from invention. Do not delete
  preview APIs in this PR unless the 0.1.0 policy already said
  they were never in preview scope.
- PR-062-A04: Compatibility policy is approved by maintainers.
  This PR writes `docs/implementation/1.0-compatibility-policy.md`
  and a readiness note `READY FOR NAMED DECISION`. Named
  maintainer approval (record ID such as
  `COMP-1.0-D-<shortsha>`) is a **separate owner action**. It is
  not `G8-D-*`. `implement the plan` **stops** before writing
  that approval row.

Principal changes that are not extra acceptance IDs, but are
required to prove the four bullets:

- Freeze review of Rust / AgentSpec / errors / events / records /
  Python / JS / remote / WIT 1.0.
- Automated compatibility checks, deprecated aliases, migration
  commands, and fixture converters where promised.
- Document which leaf packages may evolve faster than the core.
- Independent versioning only where preview feedback justifies it.

## Locked design

### Layout

```text
plugins/finstack-ai-wit/wit/v1.0.0/         # NEW; @1.0.0 worlds
plugins/finstack-ai-wit/wit/v0.0.4/         # keep; host adapter
plugins/finstack-ai-guest-sdk/MIGRATION.md  # @0.x → @1.0.0
docs/implementation/1.0-compatibility-policy.md
docs/implementation/1.0-leaf-versioning.md
docs/site/migration.md                      # 0.1.0 → 1.0.0 paths
fixtures/compatibility/wit/v1.0.0/          # NEW
tools/migrate/                              # fixture/journal/spec converters
```

Do not invent `starters/` or a second docs root. Do not restore
`tools/architecture/`. Do not invent `mise run schema-governance`.
Do not edit `docs/planning/*`.

### What is frozen (1.0 promise)

Engineering Standards §6.1 families that leave experimental
status:

| Family | 1.0 disposition |
| --- | --- |
| Public Rust API (kernel/runtime/SDK) | Frozen SemVer; breaking = major |
| Python public modules/exceptions | Frozen; wheel matrix stays ADR-018 |
| JS/TS exports + WASM host ABI | Frozen; no SharedArrayBuffer requirement (ADR-031) |
| AgentSpec / BundleSpec / locks | Strict reject-unknown; additive fields need version/default |
| Journal records / snapshots | candidate-v1 becomes the 1.0 durable line; meaning breaks need ADR + migration |
| Runtime events (durable-derived) | Order and class frozen; transient progress stays non-replay-stable |
| Remote protocol | Inbound reject-unknown; version handshake |
| Process family | Handshake-only if PR-058 left session vocabulary later; do not freeze a session vocab that does not exist |
| WIT `finstack:ai-types/host/toolset/context@1.0.0` | Frozen exact worlds; coarse completion; **no** resource streaming |

Still **experimental** (no permanent promise; explicit exclusion):

- IndexedDB host journal
- Any surface the preview policy labeled experimental and preview
  feedback did not promote
- Observer isolation / WIT observer worlds (needs a later ADR)
- Process-plugin session commands if still later

### WIT `@1.0.0` (ADR-035)

This PR lifts the `@1.0.0` generation block in
`plugins/finstack-ai-wit` (`inventory.rs`, manifest parse,
`check-wit`). Add `wit/v1.0.0/` **beside** `wit/v0.0.4/`.

- Host supports both majors through explicit adapters (TDD §27.1).
- `@0.x` guests keep loading. `@1.0.0` guests use the new worlds.
- Unknown world or undeclared `@1.0.0` field still fails closed.
- One coarse completion. No progress batch. No resource streams.
- Regenerated bindgen via `mise run gen-wit` / `check-wit`. Do
  not hand-edit generated files.
- Guest templates and reference components get a documented
  retarget; do not silently rewrite published `@0.x` fixtures.

Do not bump lockstep crate/wheel/npm version to `1.0.0` here.
Workspace at admit should already be `0.1.0` from PR-061. Stay
on that line (or `1.0.0-dev` only if preview feedback already
chose it). The GA tag is PR-066.

### Breaking-change tests (A01)

For each frozen family, check in at least one negative fixture
that would have passed if unknown state-bearing data were
silently dropped. Wire them into existing
`cargo test -p finstack-ai-test` / family tests / `check-wit`.

Public Rust: a `cargo public-api` / `cargo-semver-checks` baseline
taken from the tagged `v0.1.0` (must exist at admit) fails on a
deliberate signature break in CI. If those tools are not already
pinned, prefer a small checked-in rustdoc/public-item list diff
over adding a new toolchain.

### Migration tooling (A02)

Smallest promised surface:

- Offline converters under `tools/migrate/` (or a `mise` task)
  for journal diagnostic JSONL / AgentSpec JSON / WIT guest
  manifest `@0.x` → `@1.0.0`. Fail closed on unknown
  state-bearing fields.
- Fixture converters for `fixtures/compatibility/` version
  directories.
- Documented commands in `docs/site/migration.md`.
- Not a Cargo xtask. Not a hosted migration service.

### Deprecation (A03)

Inventory deprecated items from preview feedback + current
`#[deprecated]` / binding warnings. Each remaining item gets
`since` + removal version. Aliases stay until that version.
Do not add speculative deprecations.

### Leaf versioning (principal change 3–4)

Write `docs/implementation/1.0-leaf-versioning.md`:

- Core (`finstack-ai-kernel`, `finstack-ai-runtime`,
  `finstack-ai`, `finstack-ai-protocol`, `finstack-ai-test`,
  bindings) stay lockstep through `1.0.0`.
- First-party leaves (providers, tools, stores, observers,
  workflow, plugin-host) **may** take independent versions
  after GA only if the preview-feedback review names a coupling
  problem **and** the leaf publishes an engine-compatibility
  range. Default in this PR: keep lockstep; document the
  *policy*, do not split versions yet.
- Third-party packages were already independent.

Do not split the workspace into many versions in this PR
without that feedback evidence.

### Compatibility policy (A04)

`docs/implementation/1.0-compatibility-policy.md` is the
adopter-facing SemVer promise. It extends, not replaces,
`compatibility-governance.md` (owners/fixtures) and the PR-061
preview policy (historical 0.1.0 scope).

Minimum contents: freeze set, experimental exclusions, unknown-
field rules (TDD §28.4), ADR triggers for journal/event/WIT/
remote meaning, deprecation/removal rules, leaf engine ranges,
how breaking-change tests run.

Named approval is separate. Readiness language:
`READY FOR NAMED DECISION`.

### Graph

Kernel / default SDK / wasm graphs stay free of wasmtime,
providers, server, rustls, otel. WIT `@1.0.0` stays under
`plugins/`. Migration tools depend on protocol/kernel public
types only as needed; they are not a seventh port.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 9 entrance evidence exists (tagged
   `v0.1.0` + feedback review), Phase 8 `Done`, `G7-D-*` present;
   open `codex/pr-062-1.0-contract-freeze` from the then-current
   `main` tip. Record `PH9-E-entrance-*`. Mark PR-062
   `In progress`.
2. Freeze review + 1.0 policy draft + leaf-versioning policy
   (A04 support, A02 inventory).
3. WIT `@1.0.0` worlds beside `@0.x`, host dual-major adapters,
   lift generation block, fixtures (ADR-035).
4. Breaking-change negatives per frozen family (A01).
5. Migration converters + starter/adopter path docs (A02).
6. Deprecation inventory with removal versions (A03).
7. TM-08 review, readiness pack `READY FOR NAMED DECISION`.
   Stop before `COMP-1.0-D-*` and `G8-D-*`.

## Explicit exclusions

No permanent compatibility promise for explicitly experimental
packages. No resource-based WIT streaming. No observer WIT
world. No `1.0.0` tag or registry publish (PR-066). No
performance-budget enforcement (PR-063). No independent security
review (PR-064). No nightly/canary release engineering (PR-065).
No G5/G7/G8 inference. No fabricated adopter feedback. No
planning-file edits.

## Validation

- `mise run gen-wit` and `mise run check-wit` — `@1.0.0` present;
  `@0.x` fixtures still load
- `cargo test -p finstack-ai-wit --offline --locked`
- `cargo test -p finstack-ai-test --offline --locked` compatibility
  / journal / golden-trace families
- focused breaking-change negatives (must fail on the mutated
  fixture, pass on the frozen corpus)
- migration converter dry-run on 0.1.0 fixtures
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — still no wasmtime / providers / server
- `uv run --no-project python tools/wasm_package/check.py graph`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check` after the candidate is otherwise green

Do not require `mise run ci` on a multi-OS hosted matrix,
Playwright beyond existing browser tasks, or Criterion budgets.

## Suggested authorization sentence

When Phase 9 entrance is actually `Passed` (2/2), Phase 8 is
`Done`, `G7-D-*` exists, and the owner is ready:

```
Run PR-062; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`,
`G7-D-*`, Phase 8 work, `COMP-1.0-D-*`, `G8-D-*`, or cut
`1.0.0`. Do not infer those from `implement the plan`.
