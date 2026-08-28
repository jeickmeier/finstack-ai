# browser-knowledge

The knowledge agent's embedded path: the agent running in a Dedicated
Worker with an IndexedDB journal and a **TypeScript-implemented retrieval
toolset** registered through the host adapter surface (`JsToolset`),
searching an in-page corpus of self-doc excerpts. Scripted model by
default — no provider, no credentials, no network.

Part of the three-surface knowledge agent; the parity matrix lives in
[`apps/finstack-knowledge/README.md`](../../apps/finstack-knowledge/README.md).
Parity deltas for this surface: the journal is IndexedDB (origin-local),
so cross-surface means **inspect/export only** — the CLI and Python
notebooks share a sqlite file this page cannot reach; memory is
host-backed rather than the native extension; confinement/net-guard does
not apply inside the browser sandbox.

Trust class: JS host adapters are T2 — trusted, not isolated. Do not embed
provider credentials in a browser bundle.

## Quick start

After `mise run build-wasm -- release`, the wasm harness serves this
directory at `/examples/browser-knowledge/`:

```bash
npm --prefix bindings/finstack-ai-wasm/js run build   # package dist/
npx tsc -p examples/browser-knowledge                  # example dist/
node bindings/finstack-ai-wasm/js/scripts/serve.mjs    # http://127.0.0.1:4173
```

Controls: **Ask** (question box → streamed event kinds → answer with doc-id
citations), **Run golden set** (replays
`apps/finstack-knowledge/fixtures/golden.json`, served at `./golden.json`,
against the scripted scenario and tabulates pass/fail), **Inspect last
session**, **Clear local data**.

## The retrieval seam

[`retrieval.mjs`](retrieval.mjs) is plain browser-executable ESM so one
module serves the worker, the page, and the Playwright conformance spec
(`bindings/finstack-ai-wasm/js/src/knowledge-golden.test.ts`): the corpus,
the `search_corpus` tool schema, deterministic term ranking, and the
`JsToolset` host adapter. The scripted model proves the seam end to end —
its answer template is filled from the tool result it sees in the
follow-up model request, not from the script itself.

## Conformance

The golden spec runs inside the existing `mise run test-wasm` scope and
makes the same two assertions as the Rust tests and the Python pytest:
`must_contain` appears in the answer, and the observed event kinds are a
superset of `event_kinds_expected`. An event kind observed on one surface
and absent from another is a failing test, not a note.
