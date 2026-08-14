# PR-034 execution plan

Date: 2026-08-13
Owner: me@jeickmeier.com
Branch: `codex/pr-034-js-host-adapters`
Baseline: local `main` at `fd9759c82ef3a0029264780f2d3b78af875d96a0`
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- PR-033 is `Done` at local `main` merge `1781b8d4841d573a3bc2d82b7fe5a6abce5df145`
  (closeout `fd9759c`). Both Phase 5 entrance criteria remain `Passed`.
- PR-034 is the only active logical PR. PR-035–PR-038 surfaces stay excluded.
- No Implementation Plan section 6.3 ADR trigger applies (no seventh port, no
  per-token callback, no kernel I/O).
- Threat Model section 18 is triggered because this PR adds a secret-bearing /
  credential-adjacent same-origin adapter and TM-05 controls. A focused
  security review is required before merge. JS adapters are trusted T2 (same
  class as Python callbacks). No new trust class, listener, IndexedDB, or
  `apiKey` field is introduced.
- ADR-019 becomes Accepted / In progress / Partial. Do not mark Implemented;
  PR-038 still owns publish, multi-browser, and G4.
- `DeferredBindingAdapter::wasm()` stays unavailable.

## Acceptance mapping

| Criterion | Planned proof |
| --- | --- |
| PR-034-A01 | Playwright constructs `JsModel` / `JsToolset` over scripted hosts. A test-only export drives one `Model::request` and one `Toolset::call` and asserts terminal DTOs (`text`+`completion_id`, `output`+`is_error`). Not Agent-run parity. |
| PR-034-A02 | Cancelling `AbortSignal` rejects the host promise / closes the stream; Rust maps to `js_host_cancelled` (`ErrorCategory::Cancellation`). No leaked pending `JsFuture`. |
| PR-034-A03 | Extra fields, missing `text`/`completion_id`, and non-object tool output map to `js_host_result_invalid`. Durable diagnostics omit raw JS exception text. |
| PR-034-A04 | A `ReadableStream` or async iterable of items is enough. One completed object remains valid. No per-token JS hook. |
| PR-034-A05 | Local mock SSE server (no live provider). Streaming + `AbortSignal` fixtures pass. Default URL is same-origin. README and examples contain no provider secret. `check.py` secret scan of `js/src` + README passes. |
| PR-034-A06 | `normalizePrebetaShape` matches Python kinds and Rust DTO validation. A test-only coordinator path applies the three normalized commands without a live Agent. Store APIs are labeled pre-beta. |
| PR-034-A07 | JS `HostJournalStore` is in-memory / scripted only. Docs state IndexedDB and crash durability are PR-037/PR-048. No SQLite. |

## Implementation slices

1. Record ownership, task rows, ADR-019 Partial, and this plan.
2. Shared host invoke/error mapping and `!Send` port wrappers for the six
   ports, clock, random, and artifacts. Native tests cover DTO/error paths.
3. Normalize `Promise | ReadableStream | AsyncIterable` and wire `AbortSignal`.
4. Tree-shakeable TypeScript `fetch` + SSE `@finstack/ai/adapters/openai-compatible`.
5. `normalizePrebetaShape` plus scripted coordinator command traces.
6. Browser conformance, graph/secret scan, TM-05/§18 review, and authorized
   local integration. No npm publish, hosted pull request, or G4.

## Security and compatibility disposition

TM-05: same-origin proxy, no credential field, no credential persistence
helper, scan generated glue and examples. CORS/network failures surface as
stable host diagnostics, not provider secrets.

Do not claim Phase 5 exit, G4, WASM conformance-adapter availability, or
IndexedDB durability. Do not mark ADR-019 Implemented. Do not treat scripted
port drives as Agent-run binding parity (PR-035).

## Temporary hosted execution envelope

The PR-032 Linux-only hosted envelope remains in force through PR-038.
Restoration and execution of deferred platform rows are user-owned.
Hosted pull-request creation/merge and package publication remain prohibited.
