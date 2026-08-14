# PR-033 execution plan

Date: 2026-08-13
Owner: me@jeickmeier.com
Branch: `codex/pr-033-wasm-package`
Baseline: local `main` at `f915cbfd3ed80446fcfa1dceeabf1f3491aeb774`
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- PR-013 and PR-026 are `Done`. G3 is `Passed`. The Phase 3 candidate
  binding surface is declared in `native-preview-binding-surface.md`.
- Both Phase 5 entrance criteria are recorded as passed before implementation:
  the declared Phase 3 binding surface, and the accepted browser/npm
  conventions in TDD §4 / §26.1, Architecture §14, ENG 7.3, and ADR-031.
- PR-033 is the only active logical PR. PR-034 and later work is excluded.
- No Implementation Plan section 6.3 ADR trigger applies.
- Security and Threat Model section 18 is triggered because this PR introduces
  an npm package identity and generated browser artifacts. A focused security
  review is required before merge. No new trust class, listener, persistence
  surface, or secret-bearing adapter is introduced.
- ADR-031 remains Accepted. Implementation evidence becomes Partial; worker
  topology stays PR-036.

## Acceptance mapping

| Criterion | Planned proof |
| --- | --- |
| PR-033-A01 | `mise run check-wasm` builds kernel, runtime `wasm-host`, facade `wasm-host`, the wasm crate, and the three leaf fixtures for `wasm32-unknown-unknown`. The selected graph contains no Tokio/native I/O, and the kernel has no wasm-bindgen. |
| PR-033-A02 | Native `Send + Sync` proofs reuse existing leaf fixtures and `journal_port_bounds`. New compile fixtures implement all six ports with `!Send` JS promise/stream proxies on wasm32 and `Send + Sync` handles on native. |
| PR-033-A03 | Headless Chromium loads `@finstack/ai`, calls `health` / `buildMetadata`, and runs the embedded no-op scripted trace through `CommitCoordinator` + `MemoryJournalStore`. `DeferredBindingAdapter::wasm()` stays unavailable. |
| PR-033-A04 | `mise run generate-wasm` is the only regen path. CI fails on a dirty generated tree. Two consecutive builds compare wasm and glue bytes. |
| PR-033-A05 | The thin wasm graph check plus workspace Clippy/tests prove kernel architecture still holds. The retired 22-test architecture suite is not claimed. |

## Implementation slices

1. Record Phase 5 entrance, ownership, and task rows.
2. Pin Node, `wasm32-unknown-unknown`, and `wasm-bindgen-cli` 0.2.127; add
   `check-wasm` / graph gate before bindgen code.
3. Add the host-driven local executor and six-port compile fixtures.
4. Add wasm-bindgen exports, `generate-wasm`, and the hand-authored
   `@finstack/ai` TypeScript facade.
5. Add Playwright no-op trace, `tsc`, and bundle-size reporting.
6. Run focused/aggregate validation, security review, and authorized local
   integration. No npm publish, hosted pull request, G4, or checkpoint claim.

## Security and compatibility disposition

The packaging and release-identity review trigger applies because PR-033
changes build artifacts and introduces the `@finstack/ai` npm identity. The
review covers exact dependency/CLI pins, generated-artifact ownership,
credential-free examples, glue scanning, and hosted Linux provenance. No
seventh port, kernel I/O, per-token callback, commit-order change, or stronger
effect guarantee is introduced.

Agent/Run/Result handles, JS host adapters, workers, IndexedDB, npm
publication, WASM conformance-adapter activation, and G4 remain excluded.

## Temporary hosted execution envelope

The PR-032 Linux-only hosted envelope remains in force through PR-038.
Restoration and execution of deferred platform rows are user-owned.
