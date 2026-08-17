# PR-060 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-060-docs-security-release-artifacts`
Intended baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-060. The closed PR-054
envelope is not reused. The PR-055–PR-059 envelopes are not reused.
PR-060 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8, or record Phase 8
entrance.

## Execution envelope

Not authorized. Suggested text when the owner is ready:

```
Run PR-060; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, or from the
PR-055–PR-059 plans existing.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G5 inference, G7 inference. Do not write `G5-D-*` or
`G7-D-*`. Do not start PR-055–PR-059 or PR-061+. Do not cut or
publish `0.1.0`. Do not bump the lockstep workspace version off
`0.0.4`.

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

### PR-055 through PR-059 are `Done`

All five are planned only and still `Todo`. Implementation Plan
lists them before PR-060 ("Dependencies. All prior public
surfaces."). Keep one active logical PR. Do not admit PR-060 while
any predecessor is `Todo` unless the owner explicitly authorizes
parallel Phase 8 work in the same sentence.

PR-060's *code* dependencies are the public surfaces those PRs
add (Anthropic/Ollama, coding batteries, observers, remote server,
workflow adapters) plus everything already on `main`. Guides and
starters must describe the surfaces that exist at admit, not
invent missing batteries.

### Other admission checks

- ADR-024 stays Accepted / Partial until this PR lands the public
  RFC entry, contribution-link audit, and license-metadata sweep.
  Then it may move to Implemented / Verified for governance files
  only. That is not G7.
- Planning files under `docs/planning/` stay read-only. Do **not**
  edit the pre-implementation Threat Model in place. Publish the
  implemented-control review in the implementation/site layer.
  Changing TM design claims requires change control, not this PR.
- No Implementation Plan section 6.3 ADR trigger applies if the
  work adds no seventh port, no eighth middleware stage, no kernel
  I/O, no native dylib loader, and no stronger-than-at-least-once
  claim.
- Threat Model section 18 is triggered (build/release identities,
  provenance, public security process). Primary **TM-18**,
  **TM-04**, **TM-06**. Also **SEC-INV-012**. Complete the review
  before merge. A threat-model review is not an ADR and does not
  write `G7-D-*`.

## Traceability

Implementation Plan PR-060; Engineering Standards §6, §15 (G7
evidence list); Security and Threat Model §§12–14, §16 residual
register, TM-04 / TM-06 / TM-18; PRD distribution, risks, release
criteria §15.3–15.4, NFR-DX, NFR-SEC; Architecture §22 / ADR-024;
TDD §2 example set and milestone 8.
G7 is Phase 8's gate and is out of scope. PR-061 cuts `0.1.0`.

## Acceptance mapping

Six Implementation Plan bullets map 1:1 to A01–A06.

- PR-060-A01: Every public package has a tested quick start.
  Product-facing crates and bindings listed under Locked design
  each have a site guide or package README "Quick start" that a CI/doc-test or
  `tools/docs/quickstarts.py` (or equivalent mise task) executes
  offline. No live provider, Temporal cluster, or hosted collector.
  Kernel / runtime / protocol may point at the Rust SDK guide
  rather than a second constructor path.
- PR-060-A02: Starter repositories pin compatible versions and
  pass CI. In-tree starters under `examples/` and
  `plugins/templates/` pin workspace `0.0.4` (path or lockstep
  workspace deps). They are covered by existing `mise run ci` /
  focused tasks (`check-plugin-template`, browser/python smokes).
  Do **not** create hosted GitHub starter orgs. "Repository" here
  means a copy-out package, not a published remote.
- PR-060-A03: Security documentation clearly distinguishes native
  (T1), Python/JS callback (T2), process, and WASM/WIT (T3) trust
  levels, plus T0 kernel and T4 remote. One site page owns the
  matrix; every starter and guide that registers an extension
  links it. In-process code is never called a sandbox (TM-06).
- PR-060-A04: Release artifacts are reproducible from tagged
  source. This PR ships the rehearsal: two local staging runs
  from the same commit produce identical checksums/SBOMs for the
  artifacts the existing staging paths already know (Python
  wheel/sdist, npm tarball, crate metadata, WIT/plugin bits as
  already staged). Document that the **public tag** is PR-061.
  Do not `git tag`. Do not publish. "From tagged source" is the
  documented command sequence, proven here from HEAD.
- PR-060-A05: License, DCO, maintainer ownership, ADR, and public
  RFC links are reachable from every contribution entry point:
  root `README.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`,
  `SECURITY.md`, `docs/README.md`, `docs/site/README.md`, and
  each public-package README. Dual-license remains
  `MIT OR Apache-2.0`. DCO stays; no CLA.
- PR-060-A06: Every Threat Model control required at G7 links to
  passing evidence, an explicitly accepted residual risk, or a
  blocking issue. Publish
  `docs/implementation/threat-model-g7-review.md` covering
  SEC-INV-001–013, TM-01–TM-21, and Threat Model §§12–14 G7
  bullets. Rows that wait on PR-061 publish/tag say so as
  **blocking for G7**, not as this PR's pass. Do not write
  `G7-D-*`.

Principal changes that are not extra acceptance IDs, but are
required to prove the six bullets:

- Concept / Rust / Python / WASM / durability / provider /
  toolset / plugin / server / migration guides under `docs/site/`.
- Trust-level and security-deployment pages.
- Starter polish for the TDD example set plus WIT templates.
- RFC template and contribution-link audit.
- Local SBOM / checksum / provenance rehearsal.
- Markdown link checker and a small accessibility pass.
- Owner fresh-user walkthroughs (not fabricated external users).

## Locked design

### Layout

Follow Technical Design §2. Do not invent a competing docs root
or a `starters/` tree. Expand the existing site scaffold and the
existing example/template set.

```text
docs/site/README.md                         # public docs index (refresh; drop stale 0.0.2 claim)
docs/site/concept.md                        # NEW
docs/site/rust.md                           # NEW
docs/site/python.md                         # NEW
docs/site/wasm.md                           # NEW
docs/site/durability.md                     # NEW
docs/site/provider.md                       # NEW
docs/site/toolset.md                        # NEW
docs/site/plugin.md                         # NEW
docs/site/server.md                         # NEW
docs/site/migration.md                      # NEW
docs/site/security-trust-levels.md          # NEW; T0–T5
docs/site/security-deployment.md            # NEW; TM §15 gates
docs/site/architecture.md                   # refresh; keep mermaid
docs/site/provider-security.md              # refresh; link trust page

docs/rfcs/README.md                         # NEW public RFC entry
docs/rfcs/0000-template.md                  # NEW

docs/implementation/threat-model-g7-review.md   # NEW implemented-control matrix
docs/implementation/artifacts/pr-060/           # rehearsal notes, link-check, walkthroughs

examples/rust-minimal/                      # polish; do not add workflow to default deps
examples/python-minimal/                    # add service starter if missing
examples/browser-minimal/                   # polish worker quick start
examples/durable-interaction/               # document PR-059 fill; do not reimplement adapter
examples/ts-alpha-install/                  # keep as clean TS consumer
plugins/templates/                          # tested WIT starters (already present)
```

Do not add mdbook, Docusaurus, or a hosted docs deployment.
Do not restore `tools/architecture/`.
Do not invent `mise run schema-governance`.
Do not edit `docs/planning/*`.

### Guides

Each guide is short, secret-free, and points at a tested starter.
Workspace version in prose is **0.0.4 unpublished**; say that
`0.1.0` is the forthcoming preview cut (PR-061), not this PR.

| Guide | Must cover |
| --- | --- |
| concept | Kernel vs runtime vs SDK vs leaves; six ports; commit-before-effect; G-01 (no workflow/plugin required) |
| rust | `Agent::builder` / `start` / `run`; link `examples/rust-minimal` |
| python | Wheel install from a **staged** artifact; rust-backed vs callback; no Rust toolchain for users |
| wasm | `@finstack/ai` worker default; IndexedDB experimental; no provider keys in the bundle |
| durability | SQLite vs memory; inspect-not-continue `open_session`; interactions; at-least-once |
| provider | Separate crates; Python lazy extras; never put secrets in `AgentSpec` |
| toolset | `<100` lines excluding business logic (NFR-DX-002); calculator + filesystem + shell (PR-056) |
| plugin | Experimental `@0.0.4` WIT; deny-by-default; templates; `@1.0.0` worlds blocked until PR-062 |
| server | Loopback/Unix default; TLS 1.3+ off-loopback; `SecurityAuditGate`; PR-058 surface |
| migration | Point at `compatibility-governance.md`; preview compatibility **policy** is PR-061 |

`docs/site/README.md` becomes the public index and links the
planning/implementation authority chain without replacing it.
Root `README.md` and `docs/README.md` gain one link to the site
index.

### Starters (A01, A02)

Reuse what exists. Do not invent `examples/coding-agent/`,
`examples/workflow/`, or hosted template repos.

| Named starter | Home |
| --- | --- |
| Minimal agent | `examples/rust-minimal` `minimal` + `examples/python-minimal/rust-backed` + `examples/ts-alpha-install` |
| Coding agent | `examples/rust-minimal` `coding` (PR-056 batteries; public APIs only) |
| Python service | `examples/python-minimal/service/` **or** a `service` extra in that tree matching rust `service` (resolve once, health, one request). Do not add a new example root. |
| Browser worker | `examples/browser-minimal` |
| Durable interaction/approval | `examples/durable-interaction` (filled by PR-059; this PR owns the quick-start test and trust labels) |
| WIT plugin | `plugins/templates/toolset-plugin` and `context-plugin`; `mise run check-plugin-template` |

Pin compatible versions: workspace `0.0.4`, rustc 1.97.1, existing
Node/Python pins. Starters stay `publish = false` / private npm
where they already are.

Default `finstack-ai-native-examples` graph stays free of
workflow, server, rustls, otel, wasmtime, and Anthropic unless an
explicit example feature is selected. G-01 remains.

### Trust levels (A03)

`docs/site/security-trust-levels.md` is the single matrix:

| Class | Meaning | Starter label |
| --- | --- | --- |
| T0 | Kernel; I/O-free | not a user extension |
| T1 | Native Rust in-process | rust-minimal, native providers/tools |
| T2 | Python/JS callbacks | python-callback, JS host adapters |
| T3 | WIT/Wasmtime or process | plugin templates; process family is handshake-only until later |
| T4 | Remote principal | server clients; external completion |
| T5 | Content | prompts, retrieval, artifacts |

SECURITY.md, plugin README, python-callback README, and
browser-minimal README must link this page. TM-06: never describe
T1/T2 as isolated.

### Threat Model implemented-control review (A06)

Create `docs/implementation/threat-model-g7-review.md`.

Do not rewrite `docs/planning/06-finstack-ai-security-threat-model.md`.
That file stays the design-time contract (status
"Pre-implementation security baseline").

For each SEC-INV and TM-01–TM-21, one row:

| Control | Evidence ID or test path | Residual (TM §16) | G7 disposition |
| --- | --- | --- | --- |
| … | existing `PR-*-E-*` / test | link or `—` | `evidenced` / `accepted residual` / `blocking for G7` |

G7-required §13.2 bullets (updated threat model, external surface
review, SBOM/provenance, vulnerability process, security
deployment guide) map onto this PR's site pages + rehearsal +
SECURITY.md. Rows that require a public tag, registry publish, or
named `G7-D-*` stay **blocking for G7** and are PR-061's job.

Independent review (TM §13.3) is G8 / PR-065 territory. List it
as out of scope, not as passed.

### Release rehearsal (A04, TM-18)

Reuse existing staging — do not add a Cargo xtask:

- Python: existing `uv build` + maturin SBOM path (PR-032)
- npm: `mise run stage-wasm` / existing CycloneDX writer
- Rust: `cargo package --list` / `cargo deny` (already in CI)
- Checksums: SHA-256 over staged files
- Provenance: a generated statement that names commit SHA,
  toolchain pins from `mise.toml`, and "not published"

Two clean local staging runs from the same tree must match.
Record the commands in `docs/site/migration.md` or a short
`docs/implementation/release-rehearsal.md`.

Do not add Sigstore/OIDC in this PR unless the existing PR-032
staging path already does so locally. Do not require hosted
keyless signing (`external actions=none`).

Workspace version stays `0.0.4`. Staged artifacts may say
"preview rehearsal" but must not claim `0.1.0`.

### Governance, RFC, license (A05, ADR-024)

- Add `docs/rfcs/` with a template. Ecosystem-facing contract
  changes (journal, event order, WIT, remote protocol) require an
  RFC **in addition to** an ADR, as GOVERNANCE already says.
- Link RFC, ADR register, DCO, license texts, and maintainer
  table from every contribution entry point.
- Sweep public package metadata (`license`, `license-files`,
  npm `license`, Python `license`) for `MIT OR Apache-2.0`.
- Do not add SPDX headers to every source file. Verify headers
  only where they already exist.
- `SECURITY.md`: keep private reporting to `me@jeickmeier.com`.
  Supported-versions table may say "preview support begins when
  PR-061 tags `0.1.0`"; until then default-branch tip only. Do
  not invent a CVE portal or commercial support (explicitly
  excluded).

### Link checks, accessibility, fresh-user walkthroughs

- Add a small offline markdown relative-link checker under
  `tools/` and a `mise` task (name it when implementing; do not
  invent `schema-governance`). Check `README.md`,
  `CONTRIBUTING.md`, `GOVERNANCE.md`, `SECURITY.md`, `docs/`,
  `examples/**/*.md`, `plugins/**/*.md`.
- Accessibility: heading order in site pages; alt text on any
  images; `examples/browser-minimal` keeps labels on interactive
  controls. No third-party WCAG vendor.
- Fresh-user sessions: two owner walkthroughs recorded under
  `docs/implementation/artifacts/pr-060/` (Rust minimal + Python
  rust-backed). Script: clone/bootstrap → first successful
  offline run → note friction. Do not fabricate external
  interviewees. `external actions=none` forbids recruiting a
  hosted user-study panel.

### Graph and version

This PR is documentation, examples, templates, and staging
scripts. It must not add `temporalio`, `restate-sdk`, rustls,
opentelemetry, or wasmtime to kernel / default SDK /
`finstack-ai-native-examples`.

Do not bump to `0.1.0`. That is PR-061.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 8 entrance `Passed` (2/2) and
   PR-055–PR-059 `Done`; open
   `codex/pr-060-docs-security-release-artifacts` from the
   then-current `main` tip. Mark PR-060 `In progress`. Do not
   re-record Phase 8 entrance.
2. Site index + ten guides + trust/deployment pages (A01, A03).
3. Starter polish, Python service starter if missing, quick-start
   runner (A01, A02).
4. RFC template, contribution-link audit, license-metadata sweep
   (A05).
5. Threat-model G7 review matrix (A06).
6. Release rehearsal + two-run checksum identity (A04).
7. Link checker, accessibility pass, two owner walkthroughs,
   TM-18 review, candidate evidence. Stop before `G7-D-*`.

## Explicit exclusions

No commercial support portal or marketplace. No hosted docs
site, starter GitHub org, or user-study panel. No `0.1.0` cut,
tag, or registry publish (PR-061). No `G7-D-*`. No planning-file
rewrite of the Threat Model. No seventh port. No new provider,
server, workflow, or observer implementation. No `@1.0.0` WIT
worlds (PR-062). No independent security audit (G8). No CVE
assignment process beyond SECURITY.md.

## Validation

- `mise run docs-links` (or the name minted at implement) —
  no broken relative links on the audited set
- Quick-start runner / doc-tests for the public-package list
- `mise run check-plugin-template`
- focused rust-minimal / python-minimal / browser-minimal
  offline smokes already in CI
- Two local staging runs; checksum files byte-identical
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — still no workflow/server/otel/wasmtime/provider leaves
- `uv run --no-project python tools/wasm_package/check.py graph`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check` after the candidate is otherwise green

Do not require `mise run ci` on a multi-OS hosted matrix,
Playwright beyond existing browser tasks, a Temporal server,
or Criterion numbers.

## Suggested authorization sentence

When Phase 8 entrance is `Passed` (2/2), PR-055–PR-059 are
`Done`, and the owner is ready:

```
Run PR-060; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`, triage
the public API change backlog, admit PR-055–PR-059, cut `0.1.0`,
or record `G7-D-*`. Do not infer those from `implement the plan`.
