# PR-038 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-038-browser-conformance`
Baseline: local `main` at `e7049a6e80dd483d58fcae82659d0fe105a643ef`
  (PR-037 merge `6aafae740847c652f874f3448cb9710f24fd6393`, closeout `e7049a6`)
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- PR-037 is `Done` at local `main` merge `6aafae740847c652f874f3448cb9710f24fd6393`
  (closeout `e7049a6`). Both Phase 5 entrance criteria remain `Passed`.
- PR-038 is the only active logical PR. PR-039+ surfaces stay excluded
  (JournalStore v1, crash durability, WIT, SharedArrayBuffer).
- No Implementation Plan section 6.3 ADR trigger applies (no seventh port,
  no kernel I/O, no commit-order change, no SharedArrayBuffer).
- Threat Model section 18 review trigger applies: npm identity, signing,
  provenance, and browser security documentation. Complete TM-05 / TM-18
  and the G4 security row before merge. Do not redesign the threat model.
- ADR-007, ADR-008, ADR-019, ADR-020, and ADR-031 stay Accepted /
  In progress / **Partial** at admission.
- `DeferredBindingAdapter::wasm()` stays `Unavailable` in `cargo test`.
  Browser Playwright is the WASM parity evidence path, matching Python
  pytest after Phase 4.
- `health().durable` stays false. IndexedDB remains experimental until
  PR-048. JournalStore v1 remains PR-039.
- G4 is not inferred. This PR prepares Phase 5 exits and a readiness
  pack. `G4-D-*` is recorded only if the owner authorizes it.

## Acceptance mapping

| Criterion | Planned proof |
| --- | --- |
| PR-038-A01 | Playwright Agent-run goldens on Chromium, Firefox, and WebKit match Rust-owned record-kind prefixes for the applicable set. Existing 27 tests stay green. |
| PR-038-A02 | Benchmark report isolates startup, reducer, event-throughput, and WASM/JS crossing costs. Bundle-size JSON under `artifacts/pr-038/`. Not PR-063. |
| PR-038-A03 | Packed tarball installs in `examples/ts-alpha-install`; `tsc --noEmit` and a scripted `Agent.run` succeed. |
| PR-038-A04 | Always / Application / Model plus cache-stable prefix and fail-closed invalid activation. |
| PR-038-A05 | Staged signed npm `0.0.2` artifacts; checkpoint available / not cut. No publish. |
| PR-038-A06 | Phase 5 exit review plus G4 readiness pack. Named `G4-D-*` only if authorized. |

## Implementation slices

1. Record ownership, task rows, and this plan.
2. JS capability surface and `RunResult.trace` / `activeCapabilities`.
3. Applicable goldens and three Playwright engines.
4. Crossing-cost benchmarks and bundle-size report path.
5. Browser security docs and clean TypeScript install example.
6. npm staging (pack twice, checksums, SBOM, Sigstore workflow).
7. Validation, Phase 5 exits, G4 readiness, authorized local integration.

## Security and compatibility disposition

Browser secrets terminate at a trusted same-origin proxy. The pack contains
no provider credential fields. IndexedDB stays origin-scoped and experimental.
Workers stay single-threaded. Signing uses GitHub Actions OIDC; no npm token.
Capability selection grants no ambient authority.

Do not claim `npm publish`, a cut 0.0.2 checkpoint, G4, JournalStore v1,
crash durability, SharedArrayBuffer, or ADR-007 Implemented.

## Temporary hosted execution envelope

The PR-032 Linux-only hosted envelope remains in force through PR-038.
Hosted pull-request creation/merge and package publication remain prohibited.
Release artifacts may be staged without publication.
