# WASM / JavaScript guide

Public package: `@finstack/ai`. It is staged, not on npm. Consume a packed
tarball from `mise run stage-wasm` or the checkout after
`mise run generate-wasm`.

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

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../implementation/adr-register.md). [RFCs](../rfcs/README.md).
