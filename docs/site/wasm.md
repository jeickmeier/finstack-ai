# WASM / JavaScript guide

Public package: `@finstack/ai`. It is staged, not on npm. Consume a packed
tarball after `mise run build-wasm -- release`, or pack with
`uv run --no-project python scripts/wasm_package/stage.py`.

Workspace version is **1.0.0** unpublished. The last public tag is
`v0.1.0`. IndexedDB stays experimental.

## Quick start

The production topology hosts the engine in a Dedicated Worker. See
[examples/browser-minimal](../../examples/browser-minimal/README.md) and
[examples/ts-alpha-install](../../examples/ts-alpha-install/README.md).

```ts
import { Agent, JsModel, init } from "@finstack/ai";
import { connectWorker, exposeWorkerHost } from "@finstack/ai/worker";

await init();
```

Do not embed provider keys in the bundle. Terminate secrets at a trusted
same-origin proxy.

IndexedDB on `@finstack/ai/adapters/indexeddb` is experimental. It is not
JournalStore v1 and is not crash-durable. Reload restore is inspect, not
continue-the-run.

Host callbacks are [T2](security-trust-levels.md). They inherit page
authority and are not isolated.

`Agent.openai()`, `Agent.anthropic()`, `Agent.ollama()`,
`Agent.gateway()`, and `Agent.e2bSandbox()` are the same Rust-owned
constructors as Python. wasm-host methods exist; fail-closed is a Rust
platform error (`agent_run_unsupported_plan`), not a missing method.
Do not read environment variables. `Agent.reResolve()` returns a new
lock from reconstructed catalogs; in-flight runs keep the previous
composition. Optional `approvalGrant` on `Agent.create` selects
`per_call` (default) or `informed_batch`. `Policy` remains a mandatory
approval floor on every catalog.

`Capability` stays instruction-only (`id`, `description`, `instructions`,
`activation`), matching Python. See [capabilities](capabilities.md).

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
