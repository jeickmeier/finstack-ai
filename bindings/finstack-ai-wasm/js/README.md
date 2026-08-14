# `@finstack/ai`

Preview browser package for the Rust-owned `finstack-ai` engine. The published
TypeScript surface is hand-authored; generated wasm-bindgen glue stays in
`generated/` and is not a public API.

## Install

This package is not published. Consume it from the repository checkout after
`mise run generate-wasm`.

```ts
import { buildMetadata, health, init } from "@finstack/ai";

await init();
health(); // "ok"
buildMetadata();
```

`init` loads the generated module. `health` and `buildMetadata` do not create a
runtime, open a store, or spawn work.

## Host ABI

Trusted host objects remain the contract. Construct `JsModel`, `JsToolset`,
`JsContextProvider`, `JsMiddleware`, `JsObserver`, `JsJournalStore`, `JsClock`,
`JsRandomSource`, or `JsArtifactStore` around a host implementation. The wasm
crate wraps those objects as local port implementations and drives them with
the host-local executor.

Host methods return a completed object, a `ReadableStream`, or an async
iterable. No per-token JavaScript hook is required. Pass an `AbortSignal` on
the call options to cancel a pending host promise or close a stream.

```ts
import { JsModel, init } from "@finstack/ai";

await init();
const model = new JsModel(
  {
    async request(_draft, options) {
      options?.signal?.throwIfAborted();
      return { text: "ok", completion_id: "scripted-1" };
    },
  },
  { component: "app.model", provider: "scripted", model: "scripted-model" },
);
```

`normalizePrebetaShape` validates the three pre-beta command kinds against the
same Rust DTOs as the Python binding. It does not submit a live Agent.

Journal-store wrappers are scripted and in-memory only. They never claim crash
durability. IndexedDB persistence is later work; crash-durable browser storage
is later still. This package does not ship SQLite.

## Same-origin OpenAI-compatible battery

Import the tree-shakeable subpath, not the root barrel:

```ts
import { createOpenAICompatibleModel } from "@finstack/ai/adapters/openai-compatible";

const model = createOpenAICompatibleModel();
```

The default URL is the same-origin path `/finstack/openai`. The adapter is
TypeScript `fetch` plus SSE only. Do not embed provider credentials in browser
bundles, headers, or examples. Terminate secrets at a trusted same-origin
proxy. Optional application `headers` are not a credential helper.

## Public surface (PR-034)

- `init(): Promise<void>`
- `health(): string`
- `buildMetadata(): { version, engineVersion, implementation: "wasm", target: "wasm32-unknown-unknown" }`
- Host interfaces and `Js*` wrappers for model, toolset, context, middleware,
  observer, journal, clock, random, and artifacts
- `normalizePrebetaShape(kind, value)`
- `@finstack/ai/adapters/openai-compatible`

Agent, Run, Result, and event-batch handles remain later work, as do workers,
IndexedDB, npm publish, and G4.

## Regenerate

```bash
mise run generate-wasm
```

That command is the only supported regeneration path. It writes `generated/`
and `dist/`. CI regenerates and fails on a dirty tree.

## License

`MIT OR Apache-2.0`. Canonical texts live under [`../../../licenses/`](../../../licenses/)
and are copied next to this README.
