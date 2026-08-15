# Preview feedback review (first-party)

Date: 2026-08-15
Reviewer: me@jeickmeier.com
Workspace: tagged lockstep `0.1.0` (`v0.1.0`). Registries remain unpublished.
Evidence: `PREVIEW-E-first-party-feedback-c706126bcc26`

This file reviews preview telemetry, issue patterns, API pain points,
and migration needs after G7. It is **first-party only**. It does
**not** record Phase 9 entrance, admit PR-062, or rewrite
[`docs/planning/`](../planning/).

## Disposition

| Phase 9 entrance bullet (Implementation Plan §17) | Result |
| --- | --- |
| `0.1.0` used by external adopters | **Not satisfied.** No named external project uses this candidate. |
| Preview telemetry, issue patterns, API pain points, and migration needs reviewed | **Reviewed here from first-party sources only.** This is not external soak. |

Phase 9 entrance stays **`0/2`**. Do not write `PH9-E-entrance-*`.
Do not treat first-party starters, `docs-quickstarts`, or this file as
external adopters. Treating first-party soak as bullet 1 requires a
versioned PRD / Implementation Plan amendment.

## Adopter search (2026-08-15)

Checked and empty for out-of-tree `0.1.0` use:

- `jeickmeier/finstack-ai` GitHub issues: none
- GitHub releases: none
- `git tag v0.1.0`: cut on this commit
- crates.io / PyPI / npm publish: blocked (no registry credentials)
- Hosted PRs `#1`–`#9`: maintainer-only
- Sibling GitHub repos under `jeickmeier` and local
  `/Users/jeickmeier/Projects/*`: no `finstack-ai`, `finstack_ai`, or
  `@finstack/ai` dependency outside this workspace

In-tree starters and examples (`examples/rust-minimal`,
`examples/python-minimal/*`, `examples/durable-interaction`,
`examples/browser-minimal`, `examples/ts-alpha-install`, plugin
templates) are first-party Phase 8 facts. They do not satisfy
“external adopters.”

## Telemetry

There is no preview telemetry pipeline and no adopter-origin events.

Available first-party signals:

- G7 fresh validation at `002b615bf75c194b7d00acd6be2a036bfd614482`
  ([`artifacts/pr-061/g7-fresh-validation.txt`](artifacts/pr-061/g7-fresh-validation.txt))
- Two-run rehearsal identity sha256
  `c41a0fdd5d4fd0592af64a2e475ecfd5258bda45993df46ad120efa6a4c14de0`
- `mise run docs-quickstarts` (rust minimal/service, durable-interaction,
  python-callback, rust-backed, python service)

Not claimed: hosted OS matrix, live provider smokes (`#[ignore]`),
product analytics, crash telemetry, or registry download counts.

## Issue patterns

GitHub has no adopter issues. In-repo patterns come from
[`public-preview-roadmap.md`](public-preview-roadmap.md),
[`public-api-change-backlog.md`](public-api-change-backlog.md), and
accepted G7 residuals.

| Pattern | Source | Phase 9 implication |
| --- | --- | --- |
| Tagged `0.1.0` without registry packages | This cut | External soak can start from the git tag; crates.io / PyPI / npm still need credentials |
| WIT package names stay `@0.0.4` while crate/wheel/npm fields are `0.1.0` | ADR-035; preview policy | Freeze must document the split; `@1.0.0` worlds are PR-062 work once admitted |
| IndexedDB experimental / non-durable | G5 residual; ADR-022 Partial | Do not advertise NFR-REL-001; keep inspect-not-continue |
| Delivery at-least-once | ADR-013 | Do not claim exactly-once in 1.0 freeze |
| Process protocol handshake-only | compatibility-governance | Session vocabulary stays later |
| No JS/WASM Anthropic adapter | backlog `defer past preview` | Keep Python/native Anthropic; do not invent a JS peer in freeze |
| SharedArrayBuffer deferred | ADR-031 | Stay post-preview |
| Wheel-from-sdist with maturin `locked=true` is not bit-identical | G7 residual | Release engineering (PR-063), not a contract-meaning change |
| Live provider smokes `#[ignore]` | preview policy | Do not infer production provider soak |
| Lockstep core versioning | G7 / native surface | Independent leaf versioning only where preview feedback shows coupling harm; none exists yet |
| No marketplace / dylib ABI / PostgreSQL | excluded scope | Keep excluded |

## API pain points (first-party)

These are maintainer observations from the preview surface and
starters, not adopter tickets.

1. **Version identity is split.** Adopters must learn that framework
   `0.1.0` maps to experimental WIT `finstack:ai-*@0.0.4`. A 1.0 freeze
   has to say this in one place or the split becomes a migration trap.
2. **Preview is tagged, not registry-published.** Path-dep, git-tag,
   and in-tree starters work. Docs that say `finstack-ai==0.1.0` or
   `@finstack/ai@0.1.0` describe version fields, not crates.io / PyPI
   / npm packages.
3. **Session restore is inspect-not-continue** on IndexedDB. Browser
   starters cannot be sold as durable resume.
4. **Binding peers are uneven.** Python has lazy Anthropic and Ollama;
   JS/WASM does not. ADR-022 stays `Partial`.
5. **Default SDK graph is intentionally thin.** Batteries, observers,
   server, and workflow adapters are optional leaves. Starters that
   pull the wrong crate look like missing APIs.
6. **Remote vs process vocabularies differ.** Remote session exists;
   process is handshake-only. Mixing them is an adopter footgun.
7. **Plugin discovery is lockfile-local.** No registry fetch. Guest
   authors need the in-tree templates, not a marketplace.

No first-party finding requires a seventh port, kernel I/O, a new
journal meaning, or exactly-once delivery.

## Migration needs (0.1.0 → 1.0.0)

Needed when Phase 9 is admitted; not started here.

| Need | Owner when admitted | Notes |
| --- | --- | --- |
| WIT `@0.0.4` → `@1.0.0` world generation | PR-062 | Blocked until entrance 2/2 |
| Documented path for every supported 0.1.0 project | PR-062 / PR-064 | Today the only “projects” are first-party starters |
| Deprecated aliases and removal versions | PR-062 | No adopter-driven deprecations yet |
| Independent leaf versioning | PR-062 | Default remains lockstep; no coupling-harm evidence |
| Named Criterion budgets vs 0.1.0 baselines | PR-063 | Idle-session 61,824-byte warning remains unratified |
| Hosted matrix, cargo-deny, gitleaks, Sigstore | PR-063 / G8 | G7 accepted residuals |
| Independent review | PR-065 / G8 | Not this review |

## What would satisfy entrance later

1. Name one or more **external** repositories or products that depend
   on this `0.1.0` candidate (path, git, or published package).
2. Keep or refresh this review with those projects’ issue patterns
   and migration needs.
3. Then the first admitted Phase 9 PR may record
   `PH9-E-entrance-adopters-*` and `PH9-E-entrance-feedback-*`.

Alternatively, a versioned PRD / Implementation Plan amendment may
change §17 if the owner wants first-party soak to count. Editing
registers cannot do that.

## Decision

Keep Phase 9 `Todo`, entrance `0/2`, PR-062–PR-066 `Todo`.
Do not admit PR-062. Do not write `G8-D-*`. Do not cut `1.0.0`.
