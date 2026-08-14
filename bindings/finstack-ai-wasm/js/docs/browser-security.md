# Browser security and compatibility

This document is the PR-038 consolidation of browser security, CORS,
credentials, persistence, worker deployment, and compatibility for
`@finstack/ai`. It does not change kernel or port semantics.

## Same-origin proxy and credentials

Terminate provider secrets at a trusted same-origin proxy. The optional
`@finstack/ai/adapters/openai-compatible` battery defaults to `/finstack/openai`
and uses browser `fetch` plus SSE only. Do not embed provider credentials in
browser bundles, headers, examples, or worker scripts. The secret scan rejects
the provider credential field token used by typical OpenAI-compatible SDKs.

CORS failures from the fetch battery map to `js_host_failed`. Diagnose them at
the proxy, not by shipping a credential into the page.

## Content-Security-Policy

A production page that loads this package should allow:

- `script-src` for the application origin and the Dedicated Worker module
- `wasm-src` / `script-src` for the generated `finstack_ai_wasm_bg.wasm`
- `connect-src` for the same-origin proxy path only
- `worker-src` for the Dedicated Worker script

Do not enable `unsafe-eval` for this package. SharedArrayBuffer and
cross-origin isolation (`COOP`/`COEP`) are not required and are not shipped.

## Persistence

IndexedDB batteries on `@finstack/ai/adapters/indexeddb` are origin-scoped,
size-bounded, and deletable through `deleteIndexedDbStores()`. Schema v1 is
provisional. Persistence is experimental until PR-048 revalidates against
JournalStore v1. `health().durable` stays false. Reload restore is inspect, not
continue-the-run. Shared-device browsers share the origin database; treat the
journal as application data, not a secret store.

## Worker deployment

The production topology hosts WASM, host adapters, and optional IndexedDB
inside one Dedicated Worker. The UI thread talks only through `connectWorker`.
Do not post JS host objects across the worker boundary. Main-thread
`Agent.create` remains host-compatible. SharedArrayBuffer and threaded WASM
are a post-preview opt-in that requires a new ADR-031 reconsideration.

## Compatibility matrix

Applicable Agent-run goldens and the existing package harness run in Chromium,
Firefox, and WebKit through Playwright. Hosted Linux WebKit is “where
practical”; local Darwin WebKit remains required when hosted WebKit cannot
install. This is not a crash-durability or live-provider matrix.

## Explicit nonclaims

No Service Worker execution, multi-device sync, crash-prefix recovery, or
embedded provider secret is supported.
