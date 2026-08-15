# `@finstack/ai`

Preview browser package for the Rust-owned `finstack-ai` engine. The published
TypeScript surface is hand-authored; generated wasm-bindgen glue stays in
`generated/` and is not a public API.

## Quick start

Staged, not published. Trust class for host adapters:
[T2](../../../docs/site/security-trust-levels.md). Not isolated.
See [docs/site/wasm.md](../../../docs/site/wasm.md).

```ts
import { buildMetadata, health, init } from "@finstack/ai";

await init();
health(); // "ok"
buildMetadata();
```

## Install

This package is staged, not published. Consume a packed tarball from
`mise run stage-wasm` or the repository checkout after `mise run generate-wasm`.
See [browser security](docs/browser-security.md) and
[benchmarks](docs/benchmarks.md).

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
import { Agent, JsModel, init } from "@finstack/ai";

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
const agent = await Agent.create({ model });
const result = await agent.run("hello");
```

Compose `createOpenAICompatibleModel()` from
`@finstack/ai/adapters/openai-compatible` with `Agent.create({ model })`. There
is no `Agent.openaiCompatible` on the root barrel. Do not embed provider
credentials in browser bundles.

The production browser topology hosts the WASM engine in a Dedicated Worker.
Import `@finstack/ai/worker`, call `exposeWorkerHost` inside a module worker,
and consume transferable event-batch snapshots with `connectWorker` on the UI
thread. Construct `JsModel` / `JsToolset` inside the worker; do not post host
objects across the boundary. Main-thread `Agent.create` remains the documented
host-compatible mode with the same bounds. The default worker lag policy is
`drop-progress` (queue 32, 2s durable wait). SharedArrayBuffer and threaded
WASM are a post-preview opt-in that requires cross-origin isolation and a new
ADR-031 reconsideration.

```ts
import { Agent, JsModel, init } from "@finstack/ai";
import { connectWorker, exposeWorkerHost } from "@finstack/ai/worker";

// Dedicated Worker module
await init();
exposeWorkerHost({
  async create() {
    return Agent.create({
      model: new JsModel(
        {
          async request() {
            return { text: "ok", completion_id: "scripted-1" };
          },
        },
        { component: "app.model", provider: "scripted", model: "scripted-model" },
      ),
    });
  },
});

// UI thread
const worker = new Worker(new URL("./agent-worker.js", import.meta.url), {
  type: "module",
});
const client = await connectWorker(worker);
```

`normalizePrebetaShape` validates the three pre-beta command kinds against the
same Rust DTOs as the Python binding. It does not submit a live Agent.

Default `Agent.create` stays memory-backed. Opt into a host journal with
`Agent.create({ store })`. IndexedDB batteries live on
`@finstack/ai/adapters/indexeddb` and report `health().detail =
js_indexeddb_experimental`. Persistence remains experimental after PR-048;
it does not meet NFR-REL-001. `durable` stays false. Reload restore is
`Agent.inspectSession` / `WorkerClient.inspectSession`, not continue-the-run.
Call `deleteIndexedDbStores()` to drop origin-local data. This package does
not ship SQLite and does not claim crash durability.

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

## Public surface (PR-035 / PR-038)

- `init(): Promise<void>`
- `health(): string`
- `buildMetadata(): { version, engineVersion, implementation: "wasm", target: "wasm32-unknown-unknown" }`
- `Agent.create`, `Agent.start`, `Agent.run`, `Agent.inspectSession`
- `Agent.capabilityCatalog`, `Agent.compactCapabilityCatalog`
- `Run` (`session`, `events`, `result`, `cancel`, `closeEvents`)
- `RunResult.trace`, `RunResult.activeCapabilities`
- `Session`, `RunResult`, `Event`, `EventBatch`, `FinstackError`
- `Capability`, `CapabilityActivation`, `CapabilityCatalogItem`, `ActiveCapability`
- Host interfaces and `Js*` wrappers for model, toolset, context, middleware,
  observer, journal, clock, random, and artifacts
- `normalizePrebetaShape(kind, value)`
- `journalKnownAnswer(kind, value)`
- `@finstack/ai/adapters/openai-compatible`
- `@finstack/ai/adapters/indexeddb`
- `@finstack/ai/worker` (`connectWorker`, `exposeWorkerHost`, `WorkerRun`,
  `inspectSession`)

Default journal is the Rust in-memory store. JS `createMemoryJournalStore()`
remains a pre-beta health stub. IndexedDB persistence is experimental until
PR-048. npm artifacts may be staged; they are not published. G4 is a separate
named decision. Dropping a `Run` or `WorkerRun` detaches observation and does
not cancel. Applicable goldens run in Chromium, Firefox, and WebKit.

## Regenerate

```bash
mise run generate-wasm
```

That command is the only supported regeneration path. It writes `generated/`
and `dist/`. Same-host `python tools/wasm_package/check.py dirty` fails when
regeneration drifts from the committed tree. Hosted Ubuntu regenerates for
consecutive identity and browser tests; rustc/wasm-bindgen output is not
cross-OS identical.

## License

`MIT OR Apache-2.0`. Canonical texts live under [`../../../licenses/`](../../../licenses/)
and are copied next to this README.
[DCO](../../../CONTRIBUTING.md). [Maintainers](../../../GOVERNANCE.md).
[ADRs](../../../docs/implementation/adr-register.md).
[RFCs](../../../docs/rfcs/README.md).
