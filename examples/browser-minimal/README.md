# browser-minimal

Experimental same-origin browser persistence demo for `@finstack/ai`.

IndexedDB is a trusted host JournalStore adapter. It is not JournalStore v1
and not crash-durable. Persistence remains experimental after PR-048;
it does not meet NFR-REL-001. `health().detail` is
`js_indexeddb_experimental`. Reload restore is **inspect**, not
continue-the-run.

## Risks

- Data is origin-scoped. Another site cannot read it, but every page on this
  origin can.
- Shared-device browsers keep the journal until the user clears site data or
  clicks **Clear local data**.
- Schema v1 is provisional. A later upgrade may refuse to open this database
  (`journal_schema_unsupported`). Schema version remains 1.
- Deleting the database discards inspect history. That is not run cancellation.

## Topology

The production default hosts WASM, adapters, and IndexedDB inside a Dedicated
Worker. The UI talks only through `connectWorker`. Main-thread
`Agent.create({ store })` remains host-compatible.

This example does not use a Service Worker, SharedArrayBuffer, or COOP/COEP.
Do not embed provider credentials in the browser bundle; terminate secrets at a
trusted same-origin proxy.

## Serve

The wasm harness serves this directory at `/examples/browser-minimal/` after
`mise run generate-wasm`.
