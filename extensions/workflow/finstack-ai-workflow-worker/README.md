# finstack-ai-workflow-worker

Leased worker for `finstack-ai-workflow-local`. Polls an adapter-owned wake
index and the local cron table, claims due work with CAS leases, and resumes
runs parked on Timer / Interaction / DeferredEffect waits.

- Sessions become visible to the worker only through `park()`.
- All tables are hints; the kernel journal is authoritative.
- Missed cron fires coalesce into one catch-up fire (inherited from the
  local cron adapter).
- Hosts register a `PortsFactory` per workflow kind and a `RunStarter` per
  cron schedule; the worker never invents ports.

## Stock daemon

The crate ships a `finstack_workflow_worker` binary:

```sh
finstack_workflow_worker <sqlite-path>
```

It opens the worker/cron/journal sqlite tables on one file, builds a
`WorkflowWorker` with no `PortsFactory` or `RunStarter` registered, and
spawns the tick loop until `ctrl-c`. With nothing registered it still fires
due cron schedules and corrects stale wake-index rows against the journal —
both phases need only the adapter tables. It cannot resume a run onto a
wait, because that requires host-owned ports (a model, tools, middleware);
runs needing ports resume only in embedding hosts that register their own
`PortsFactory`/`RunStarter` and call `WorkflowWorker::spawn` themselves.

## Embedding

Hosts that need runs to actually resume build their own worker instead of
using the stock binary:

```rust
let worker = Arc::new(
    WorkerBuilder::new(journal, cron, wake, fires, inbox)
        .register_ports("research", Arc::new(MyPortsFactory))
        .register_starter("nightly", Arc::new(MyRunStarter))
        .build(),
);
let handle = worker.spawn(Duration::from_millis(500));
// ... later, on shutdown:
handle.shutdown().await;
```

`register_ports` binds the model/tool/middleware ports used to resume one
workflow kind; `register_starter` binds the idempotent run-starter used to
bridge one cron schedule's claimed fires into a started run. `spawn` runs
`tick()` on `poll_interval`, pumping the clock from the system clock each
iteration; `shutdown` signals the loop and waits for its current tick to
finish before returning.

### What a registered `PortsFactory` does and does not buy

Registering ports is necessary to resume a run, but it does not make every
run reach its next wait under this worker alone. The worker fires timers,
applies inbox responses to the journal, and re-parks or completes runs whose
next wait is reachable without a facade decision. A run that is mid-flight in
the model/tool stage loop needs an externally submitted
`KernelInput::StageSettled { .. AfterModel | AfterToolBatch .., Continue }`,
produced by the application-level facade in the `finstack-ai` agent layer — a
crate this worker deliberately does not depend on. Those runs are not
advanced here: the response stays in the inbox, the wake row survives, and
the attempt is recorded as one counted failure with backoff, preserved for a
host that can drive them. Hosts that embed a facade see such runs resume to
completion; hosts that do not see them held safely, not lost.

## Schema

The worker's sqlite tables (`finstack_workflow_worker_wake`, `_fires`,
`_inbox`) are **versionless by doctrine**, matching the
`finstack_workflow_local_cron` precedent: they set no `PRAGMA user_version`
and are not part of the kernel journal's schema version. They are hints, so a
binary that does not understand a column can ignore it. Future changes must
be additive and nullable — new tables, or new nullable columns that mean
something sensible when absent — so old and new binaries can share one file
without a migration step. Existing columns are never repurposed or dropped.

## Guarantees

- **Lease CAS + journal append CAS.** A wake row is claimed with a
  compare-and-swap on `(leased_by, lease_expires_at)`; the underlying journal
  append is itself CAS'd on the expected sequence. Two workers racing the
  same row can never both drive it.
- **Fire-once catch-up.** A schedule with multiple missed fires coalesces
  into a single catch-up fire and advances past every missed occurrence —
  inherited from the local cron adapter, not re-derived here.
- **Idempotency keys `(tenant, schedule_id, fire_count)`.** Every claimed
  fire carries a key built from these three fields; a `RunStarter` receives
  the same key on retry and must treat it as the request's identity, so a
  redelivered fire can never start two runs.
- **Index-is-a-hint.** The wake index, fire table, and inbox are never
  authoritative — only the kernel journal is. A stale or poisoned adapter
  row is corrected (or isolated as one counted failure) the moment the
  worker actually claims and attaches the session; it can never desync the
  journal itself.
