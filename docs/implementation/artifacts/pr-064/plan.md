# PR-064 execution plan

Date: 2026-08-15
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-064-reliability-fuzz-security-review`
Intended baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-064. The closed PR-054
envelope is not reused. The PR-055–PR-063 envelopes are not reused.
PR-064 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8 or Phase 9, cut `0.1.0`
or `1.0.0`, or record G5 / G7 / G8.

## Execution envelope

Not authorized. Suggested text when the owner is ready:

```
Run PR-064; mode=integrated; target=main; local branch/commit/merge
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
write `G5-D-*`, `G7-D-*`, or `G8-D-*`. Do not start PR-055–PR-063
or PR-065+. Do not cut or publish `1.0.0` (PR-066). Do not invent
adopter usage, findings closeout, or an independent-review pass.

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
Phase 9 PR. If PR-062 (or another Phase 9 PR) already recorded
`PH9-E-entrance-*`, do not re-record it here.

### Phase 8 is `Done` and G7 is `Passed`

PR-055–PR-061 must be `Done`. `G7-D-*` must exist. The
Implementation Plan dependency is **PR-061 and a mature feature
set**: filesystem/shell batteries, remote protocol, WIT host,
observers, and the preview threat-model refresh must exist so
the review and fuzz surfaces are real. Keep one active logical
PR.

### PR-062 and PR-063 are `Done` unless the owner parallelizes

Implementation Plan lists PR-062, PR-063, then PR-064. Review
the frozen 1.0 contracts (PR-062), not a moving preview ABI.
Do not admit PR-064 while PR-062 or PR-063 is `Todo` unless the
owner explicitly authorizes parallel Phase 9 work in the same
sentence. PR-064 does not freeze contracts, ratify perf budgets,
or cut `1.0.0`.

### Other admission checks

- Threat Model §18 **is** triggered (independent review,
  advisories, plugin / filesystem / shell / protocol / secrets).
  Complete that review before merge. It is not `G8-D-*`.
- No Implementation Plan section 6.3 ADR trigger applies if the
  work adds no seventh port, no kernel I/O, and no new durable
  meaning. A finding that changes journal/protocol/WIT **meaning**
  still needs an ADR + fixtures in the same change.
- ADR-013 stays at-least-once. Do not claim exactly-once from
  crash-prefix extensions.
- NFR-SEC-001 and the TM §16 residual remain: native in-process
  Rust/Python/JS is trusted. This PR's explicit exclusion is
  "no guarantee against malicious native in-process extensions."
  Do not describe T1/T2 as sandboxed.
- Do not restore `tools/architecture/`. Do not invent
  `mise run schema-governance`.
- `mise run fuzz-smoke` and the PR-013 cargo-fuzz workspace are
  **absent** from current root `mise.toml` / the tree. Restoring
  them is this PR's job once admitted. Do not invent AFL,
  honggfuzz, or a second fuzzer family.

## Traceability

Implementation Plan PR-064 and §18.3 (independent review closes
before `1.0.0`); Phase 9 exit "Security and reliability reviews
complete" (this PR's evidence, not `G8-D-*`); PRD NFR-SEC-001–005
and NFR-REL-001–005; Engineering Standards §10 (fuzz, property,
fault/crash-prefix, security layers) and G8 "independent security
review"; Threat Model §13.2 G8 row, §13.3 independent-review
scope, §14 advisories; TDD §32.4 / §34.1 fuzz-smoke.
G8 is Phase 9's gate and is out of scope. PR-066 cuts `1.0.0`.
PR-065 owns ecosystem conformance and release engineering, not
this review.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-064-A01: No open critical/high finding blocks GA. Every
  Critical/High from the independent review is `Closed` or
  `Accepted` with severity, owner, compensating control, and a
  remediation date. `Open` Critical/High fails this PR. An
  `Accepted` Critical/High is the principal-change "explicitly
  accept" path; it is not an `Open` finding. Do not write
  `G8-D-*` when the register is clean.
- PR-064-A02: Fuzz corpora and regression cases are checked in
  where safe. Seed corpora are secret-free, size-bounded, and
  stored with the restored cargo-fuzz targets. A crash becomes a
  deterministic unit/proptest regression, not a raw dump with
  paths or payloads. Do not check in exploit PoCs.
- PR-064-A03: Crash/recovery coverage includes every effect
  category and lane operation. `EffectKind` is `{Model, Tool,
  Context, Middleware, Interaction, Timer}`. Public lane
  operations are create, acquire, reject-while-busy, concurrent
  sibling, and deterministic restore (PR-047). Extend
  `crates/finstack-ai-test/tests/crash_prefix.rs` (and lane
  tests) so each kind/operation has a named prefix that recovers
  to a `LegalRestore` class. The current 37-prefix matrix does
  **not** automatically satisfy this; prove the catalog.
- PR-064-A04: Threat model and security advisories are updated.
  Write `docs/implementation/threat-model-g8-review.md` against
  TM-01–TM-21 and §13.3. Refresh `SECURITY.md` supported
  versions to the then-current preview line (`0.1.0` at admit).
  Publish an advisory index (empty is allowed if no CVE/GHSA
  exists). Do **not** edit
  `docs/planning/06-finstack-ai-security-threat-model.md` unless
  change control is triggered.

Principal changes that are not extra acceptance IDs, but are
required to prove the four bullets:

- Extended fuzz campaigns over parsers, records, events, remote
  frames, WIT inputs, and recovery sequences.
- Complete property/model checking for reducer **and** lane
  invariants.
- Conduct (or, if separately named, commission) an independent
  security review focused on plugins, filesystem/shell tools,
  protocols, and secret handling, covering the full TM §13.3
  list.
- Close or explicitly accept findings with severity and
  remediation dates.

## Locked design

### Layout

```text
fuzz/                                            # RESTORE isolated cargo-fuzz workspace (PR-013)
  fuzz_targets/record_replay_json.rs
  fuzz_targets/kernel_transition_json.rs
  fuzz_targets/run_event_json.rs
  fuzz_targets/message_json.rs
  fuzz_targets/remote_frame.rs                   # NEW; PR-058 framing
  fuzz_targets/wit_input.rs                      # NEW; WIT guest/host bytes
  fuzz_targets/recovery_sequence.rs              # NEW; journal prefix / restore
crates/finstack-ai-kernel/tests/model_only_reducer/properties.rs  # keep; extend
crates/finstack-ai-test/tests/crash_prefix.rs    # extend EffectKind + lane catalog
crates/finstack-ai-test/tests/lanes.rs           # lane property/model cases
docs/implementation/threat-model-g8-review.md    # NEW; do not edit planning TM
docs/implementation/artifacts/pr-064/findings.md # severity / status / dates
docs/implementation/artifacts/pr-064/independent-review.md
docs/security/advisories/README.md               # NEW index; empty rows OK
SECURITY.md                                      # supported versions at admit
```

Do not invent a second fuzzer, a TLA+/Alloy checker, or a second
docs root. Do not restore `tools/architecture/`. Do not invent
`mise run schema-governance`. Do not edit `docs/planning/*`.

### Fuzz (A02 + principal change 1)

Restore the PR-013 isolated cargo-fuzz workspace and the four
JSON targets (`record_replay_json`, `kernel_transition_json`,
`run_event_json`, `message_json`). Then add the missing mature
surfaces:

| Target | Input | Fail closed on |
| --- | --- | --- |
| Existing four | candidate-v1 JSON | panic / hang / OOM beyond RSS cap |
| `remote_frame` | 4-byte BE length + family-tagged CBOR | oversize, unknown family, downgrade |
| `wit_input` | WIT guest bytes / instantiate args | ambient WASI, limit bypass |
| `recovery_sequence` | journal prefix / snapshot+tail | silent accept of corrupt prefix |

Harness rules (ENG §10, TM-09/TM-10):

- Nightly-only `cargo-fuzz` is allowed for fuzz jobs. Consumers
  must not require nightly.
- Fixed-seed smoke in CI (PR-013 used seed 1, 256 runs, 5 s,
  1 MiB, 2 GiB RSS). Restore `mise run fuzz-smoke` as a thin
  task once the workspace exists.
- Extended campaigns (PR-013 hosted ten-minute style) are a
  **separately named** external action. The local envelope runs
  smoke plus a bounded local campaign and checks in safe
  corpora. Do not claim a hosted soak from a laptop run.
- Corpora are secret-free. Strip home paths. Reject inputs that
  embed credentials.
- A unique crash becomes a unit/proptest regression in the
  owning crate, then the corpus entry.

Do not add libAFL/honggfuzz. Do not fuzz live providers.

### Property / model checking (principal change 2)

Keep the existing fixed-seed kernel reference machine
(`properties.rs`, 256 cases, seed `0x5eed_0130_0000_0001`).
Complete lane invariants with the same proptest style:

- one active operation per named lane;
- concurrent sibling on a second lane;
- busy-lane reject does not cancel the sibling;
- restore yields a `LegalRestore` class;
- `LaneCreated` / `LaneMoved` fail closed on unknown
  state-bearing fields.

Do not add a new model-checker crate. Persist shrinks under
`proptest-regressions/`.

### Crash / recovery catalog (A03)

Extend `crash_prefix.rs` with an explicit inventory test:

```text
every EffectKind has ≥1 prefix
every public lane operation has ≥1 prefix
LegalRestore ∈ {Retryable, Suspended, Cancelled, Completed, Failed}
NFR-REL-002: valid state or explicit corruption — never silent loss
```

Reuse `LegalRestore` / `all_activated_record_bodies`. Do not
weaken queue bounds (NFR-REL-004). Do not claim exactly-once.

### Independent review (A01 + principal changes 3–4)

TM §13.3 minimum scope (all required):

- journal/recovery integrity and external completion;
- remote framing, authentication hooks, and authorization
  context;
- interaction authorization and privileged tool dispatch;
- filesystem/shell and artifact boundaries;
- WIT/Wasmtime permissions, limits, signatures, and cache
  identity;
- Python/JavaScript callback lifecycle and browser credential
  guidance;
- secret/redaction behavior; and
- dependency/build/release provenance.

Implementation Plan focus (subset, not a reduction): plugins,
filesystem/shell tools, protocols, secret handling.

Under `external actions=none`, **conduct** a first-party
independent review: a written report whose reviewer is named
and is not the same person/session that implemented the
reviewed surfaces in this PR. **Commissioning** an external
firm is a separately named external action. A self-review by
the implementer alone does not satisfy "independent."

Findings live in
`docs/implementation/artifacts/pr-064/findings.md`:

| Field | Required |
| --- | --- |
| ID | `FIND-064-…` |
| Severity | Critical / High / Medium / Low (SECURITY.md rubric) |
| Surface | plugin / fs-shell / protocol / secret / other §13.3 |
| Status | Open / Accepted / Closed |
| Remediation date | date or named follow-up PR |
| Evidence | test path or review section |

A01 fails while any Critical/High is `Open`. `Accepted` needs
every exceptions-register field if it waives an Engineering
Standards rule; it cannot waive kernel I/O, a seventh port, or
unbounded queues.

Do not write exploit PoCs. Do not put secrets in the report.

### Threat model and advisories (A04)

- `docs/implementation/threat-model-g8-review.md`: one row per
  TM-01–TM-21 and each §13.3 bullet, with evidence and
  disposition (`evidenced` / `accepted residual` / `finding`
  linking `FIND-064-*`). Independent review is **this PR**, not
  PR-065 (the PR-060 plan's "PR-065" pointer is superseded).
- Do not rewrite the planning Threat Model. Status there stays
  the design-time baseline unless change control is opened.
- `SECURITY.md`: supported versions at admit (`0.1.0` plus
  default-branch tip). Keep private reporting to
  `me@jeickmeier.com`.
- `docs/security/advisories/README.md`: process + index. Empty
  index is valid. Do not invent GHSA/CVE IDs.

### Graph and version

No new heavy security-scanner crate in kernel/runtime/default
SDK. cargo-fuzz stays in the isolated `fuzz/` workspace.
Workspace version stays whatever PR-061 left (`0.1.0`); do not
bump to `1.0.0`.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 9 entrance `Passed` (2/2), Phase 8
   `Done`, `G7-D-*` exists, PR-061–PR-063 `Done` unless
   parallelized; open
   `codex/pr-064-reliability-fuzz-security-review` from the
   then-current `main` tip. Mark PR-064 `In progress`. Do not
   re-record Phase 9 entrance if an earlier Phase 9 PR did.
2. Restore cargo-fuzz + `mise run fuzz-smoke`; add remote / WIT
   / recovery targets; check in safe corpora (A02).
3. Lane property/model tests; reducer gaps only if a bench/review
   shows a missing invariant.
4. Crash-prefix catalog for every `EffectKind` and public lane
   operation (A03).
5. Independent review report (TM §13.3). File findings. Fix or
   explicitly accept Critical/High (A01).
6. `threat-model-g8-review.md`, `SECURITY.md`, advisory index
   (A04). Stop before `G8-D-*`.

## Explicit exclusions

No guarantee against malicious native in-process extensions.
No exactly-once claim. No `1.0.0` cut. No G8 inference. No
fabricated independent review or empty-pass findings register.
No planning-file Threat Model rewrite without change control.
No exploit PoCs or secret-bearing corpora. No second fuzzer
family. No hosted long-fuzz or external audit firm unless
separately named. No PR-065 marketplace or release-automation
work.

## Validation

Use checked-in tasks and direct commands. Do not assume a
historical task still exists until this PR restores it.

- `cargo test -p finstack-ai-test --test crash_prefix --test lanes --offline --locked`
- kernel property suite (`model_only_reducer` / lane proptest)
- `mise run fuzz-smoke` after the task is restored
- secret-free corpus listing + SHA-256 of checked-in seeds
- findings register: zero `Open` Critical/High
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `mise run check` after the candidate is otherwise green

Do not require a hosted ten-minute fuzz matrix, an external
audit firm, Temporal, or `mise run ci` on a multi-OS matrix
unless separately named.

## Suggested authorization sentence

When Phase 9 entrance is `Passed` (2/2), Phase 8 is `Done`,
`G7-D-*` exists, PR-061 (and PR-062/PR-063 unless
parallelized) are `Done`, and the owner is ready:

```
Run PR-064; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`,
`G7-D-*`, `G8-D-*`, commission an external audit, run hosted
long-fuzz, or cut `1.0.0`. Do not infer those from
`implement the plan`.
