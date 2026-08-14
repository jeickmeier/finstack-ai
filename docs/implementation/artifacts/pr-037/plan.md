# PR-037 execution plan

Date: 2026-08-13
Owner: me@jeickmeier.com
Branch: `codex/pr-037-indexeddb-demo`
Baseline: local `main` at `c946fc6c37d4b37ca05df9712ca6a75dbcb56f49`
  (PR-036 merge `ecf6d22962676ba0159ab6f31ba5040d0d4cd7f4`, closeout `c946fc6`)
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- PR-036 is `Done` at local `main` merge `ecf6d22962676ba0159ab6f31ba5040d0d4cd7f4`
  (closeout `c946fc6`). Both Phase 5 entrance criteria remain `Passed`.
- PR-037 is the only active logical PR. PR-038 surfaces stay excluded
  (multi-browser goldens, npm alpha, G4, SharedArrayBuffer).
- No Implementation Plan section 6.3 ADR trigger applies (no seventh port,
  no kernel I/O, no commit-order change, no SharedArrayBuffer).
- Threat Model section 18 review trigger applies: new browser persistence
  surface. Complete TM-05 / TM-20 / §8.3 / §8.6 controls and a binding-safety
  review before merge. Do not redesign the threat model.
- ADR-019, ADR-031, and ADR-036 stay Accepted / In progress / **Partial**.
  Do not mark Implemented. ADR-036 gains IndexedDB artifact-adapter evidence
  only; PR-056 remains.
- `DeferredBindingAdapter::wasm()` stays unavailable.
- `health().durable` stays false. Reload survival is inspect, not
  NFR-REL-001 crash durability. JournalStore v1 and durable-beta remain
  PR-039 / PR-048.

## Acceptance mapping

| Criterion | Planned proof |
| --- | --- |
| PR-037-A01 | Worker-hosted scripted run completes; reload; `inspectSession` returns `completed` and the same `resultText`; journal remains in IndexedDB. Not G4 / not crash-prefix. |
| PR-037-A02 | Hanging scripted model; reload or terminate mid-run; inspect returns `in_progress` or `cancelled` with `headSequence > 0`. Not the durable-beta matrix. Memory-backed worker still cannot resume. |
| PR-037-A03 | `expected_sequence` mismatch → Conflict; exact `batch_id` retry → original receipt; schema-version mismatch and byte-flipped batch → Integrity; ordered load; artifact put/get survives reload; oversized blob rejected. |
| PR-037-A04 | `examples/browser-minimal` imports only `@finstack/ai`, `@finstack/ai/worker`, and `@finstack/ai/adapters/*`. |
| PR-037-A05 | Package README, example README, and `health().detail` identify persistence as experimental until PR-048. Secret scan is clean. |

Existing PR-033–PR-036 Chromium tests (21) stay green.

## Implementation slices

1. Record ownership, task rows, and this plan.
2. Host journal ABI (`append` / `load` / `write_snapshot`) and
   `Agent.create({ store })` / `Agent.inspectSession`.
3. IndexedDB journal and artifact batteries, schema v1, worker inspect.
4. Chromium A01–A03 proofs and `examples/browser-minimal` (A04–A05).
5. Browser/graph/secret/TM-05/TM-20 validation and authorized local integration.

## Security and compatibility disposition

IndexedDB is a trusted T2 host JournalStore / artifact adapter on the owning
Dedicated Worker. It is origin-scoped, size-bounded, and deletable.
`durable` remains false. Inspect is not continue-the-run. Drop≠cancel still
holds. No Service Worker, multi-device sync, SharedArrayBuffer, COOP/COEP,
credential field, or seventh kernel port.

Do not claim Phase 5 exit, G4, WASM conformance-adapter availability,
JournalStore v1, crash durability, or ADR Implemented.

## Temporary hosted execution envelope

The PR-032 Linux-only hosted envelope remains in force through PR-038.
Hosted pull-request creation/merge and package publication remain prohibited.
