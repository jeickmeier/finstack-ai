# browser-minimal

Experimental same-origin browser persistence demo for `@finstack/ai`.

IndexedDB is a trusted host JournalStore adapter. It is not JournalStore v1
and not crash-durable. Persistence remains experimental;
it does not meet NFR-REL-001. `health().detail` is
`js_indexeddb_experimental`. Reload restore is **inspect**, not
continue-the-run.

Trust class: JS host adapters are T2.
They are not isolated. Stored prompts and results are
T5.

Workspace manifests are staged at **2.0.0**. The latest local release tag is
`v1.0.0`; build this example from the repository until 2.0 is published.

## Quick start

The wasm harness serves this directory at `/examples/browser-minimal/` after
`mise run build-wasm -- release`. Interactive controls are labeled
**Run scripted session**, **Inspect last session**, and **Clear local data**.

## Risks

- Data is origin-scoped. Another site cannot read it, but every page on this
  origin can.
- Shared-device browsers keep the journal until the user clears site data or
  clicks **Clear local data**.
- Schema v3 is provisional. Upgrading an earlier provisional database discards
  snapshots and artifacts but preserves committed journal batches for replay.
  Opening a newer unsupported schema fails with `journal_schema_unsupported`.
- Deleting the database discards inspect history. That is not run cancellation.

## Topology

The production default hosts WASM, adapters, and IndexedDB inside a Dedicated
Worker. The UI talks only through `connectWorker`. Main-thread
`Agent.create({ store })` remains host-compatible.

This example does not use a Service Worker, SharedArrayBuffer, or COOP/COEP.
Do not embed provider credentials in the browser bundle; terminate secrets at a
trusted same-origin proxy.
