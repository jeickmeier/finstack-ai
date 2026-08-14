# PR-035 execution plan

Date: 2026-08-13
Owner: me@jeickmeier.com
Branch: `codex/pr-035-js-handles`
Baseline: local `main` at `2d570bd22d59b631b9a8b65c119c9a6ac20a659b`
  (PR-034 merge `6bd1979c7ae01bd0cf497ba434abca3c9f72dea2`, closeout `2d570bd`)
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- PR-034 is `Done` at local `main` merge `6bd1979c7ae01bd0cf497ba434abca3c9f72dea2`
  (closeout `2d570bd`). Both Phase 5 entrance criteria remain `Passed`.
- PR-035 is the only active logical PR. PR-036–PR-038 surfaces stay excluded.
- No Implementation Plan section 6.3 ADR trigger applies (no seventh port, no
  per-token callback, no kernel I/O, no commit-order change).
- Threat Model section 18 is not redesigned. Binding-safety review of handle
  lifetime, detach-versus-cancel, and error redaction is required before merge
  (same class as PR-028). JS adapters remain trusted T2.
- ADR-019 stays Accepted / In progress / Partial. Do not mark Implemented.
- `DeferredBindingAdapter::wasm()` stays unavailable.

## Acceptance mapping

| Criterion | Planned proof |
| --- | --- |
| PR-035-A01 | Playwright `Agent.create` with scripted `JsModel`; `agent.run` returns text. Second test: one tool call, `JsToolset` completes, model finals. Rust `MemoryJournalStore`. Not G4. |
| PR-035-A02 | Getters do not serialize durable state. `toJson` / `toJsonBytes` / `toDict` / `events()` are the only snapshot paths. |
| PR-035-A03 | Multi-delta scripted model: batch count < event count; sequences monotonic; terminal event present. |
| PR-035-A04 | `closeEvents` / iterator drop leaves `result()` completable. Double `cancel()` is idempotent (`agent_run_cancelled` + session context). Drop does not cancel. |

## Implementation slices

1. Record ownership, task rows, and this plan.
2. wasm-host driver hook and sequential `HostRunOwner`.
3. Enable facade `Agent` on wasm-host with host clock/random.
4. wasm-bindgen + TypeScript Agent/Run/Result/EventBatch handles.
5. Chromium scripted Agent-run tests.
6. Browser/graph/secret/binding-safety validation and authorized local integration.

## Security and compatibility disposition

No new trust class, listener, IndexedDB, or credential field. Durable
diagnostics omit raw JS exception text. Default journal is process-local
`MemoryJournalStore` and is not crash-durable.

Do not claim Phase 5 exit, G4, WASM conformance-adapter availability,
workers, or IndexedDB durability. Do not mark ADR-019 Implemented.

## Temporary hosted execution envelope

The PR-032 Linux-only hosted envelope remains in force through PR-038.
Hosted pull-request creation/merge and package publication remain prohibited.
