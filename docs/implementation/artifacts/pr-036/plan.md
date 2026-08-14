# PR-036 execution plan

Date: 2026-08-13
Owner: me@jeickmeier.com
Branch: `codex/pr-036-js-workers`
Baseline: local `main` at `b8ed9fc31e88e323a3192322f753059ca1069074`
  (PR-035 merge `a7d47fad5cc0757f03f6d56d9faebde97c693244`, closeout `865b492`)
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- PR-035 is `Done` at local `main` merge `a7d47fad5cc0757f03f6d56d9faebde97c693244`
  (closeout `865b492`). Both Phase 5 entrance criteria remain `Passed`.
- PR-036 is the only active logical PR. PR-037–PR-038 surfaces stay excluded.
- No Implementation Plan section 6.3 ADR trigger applies (no SharedArrayBuffer,
  no seventh port, no kernel I/O, no commit-order change).
- Threat Model section 18 is not redesigned. Binding-safety review of the
  worker protocol, terminate/unload, redaction, and §8.3 size-bounded
  handle-correlated transfers is required before merge (same class as PR-035).
  JS adapters remain trusted T2 and stay on the owning worker.
- ADR-031 stays Accepted / In progress / Partial. Do not mark Implemented.
- ADR-019 stays Accepted / In progress / Partial. Do not mark Implemented.
- `DeferredBindingAdapter::wasm()` stays unavailable.

## Acceptance mapping

| Criterion | Planned proof |
| --- | --- |
| PR-036-A01 | Worker-hosted 1000-delta stream; `#ui-tick` advances; batch count < event count; UI does not fetch `.wasm`. Not G4. |
| PR-036-A02 | `terminate()` mid-run is cancelled/recoverable; new worker starts fresh; page close/unload drops waiters; cancelled fetch aborts. |
| PR-036-A03 | Slow consumer keeps the outbound queue at capacity; `droppedProgress` or disconnect; terminal protected; `backpressure()` readable. |
| PR-036-A04 | Model-only and one-tool-cycle traces match on main-thread `Agent` and `connectWorker`. Existing 15 tests stay green. |

## Implementation slices

1. Record ownership, task rows, and this plan.
2. Worker protocol, `exposeWorkerHost`, `connectWorker`, and fixture worker.
3. Bounded transferable batches, ack, drop-progress, observable backpressure.
4. Chromium worker tests for A01–A04.
5. Browser/graph/secret/binding-safety validation and authorized local integration.

## Security and compatibility disposition

No new trust class, listener, IndexedDB, SharedArrayBuffer, or credential field.
Worker messages are size-bounded and carry `agentId` / `runId`. Durable
diagnostics omit raw JS exception text. Default journal is process-local
`MemoryJournalStore` and is not crash-durable. Worker restart cannot resume.

Do not claim Phase 5 exit, G4, WASM conformance-adapter availability,
IndexedDB durability, or ADR-031 Implemented.

## Temporary hosted execution envelope

The PR-032 Linux-only hosted envelope remains in force through PR-038.
Hosted pull-request creation/merge and package publication remain prohibited.
