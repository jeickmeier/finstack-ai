# finstack-ai-workflow-worker

Leased durable worker for `finstack-ai-workflow-local`. The kernel journal is
authoritative; the worker owns bounded adapter tables for wake hints, cron
fires, buffered responses, and response dead letters.

## Building a worker

`WorkerBuilder::new` takes the journal plus the two adapter tables every
worker needs: the wake index and the response inbox. Cron scheduling is
opt-in through `WorkerBuilder::cron`, which adds the schedule store, the
fire table, and the retention window for started fires:

```rust,no_run
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, MemoryWorkerStore, WakeIndexStore, WorkerBuilder,
};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let journal: Arc<dyn finstack_ai_runtime::ports::journal::JournalStore> = unimplemented!();
let store = Arc::new(MemoryWorkerStore::new());
let worker = WorkerBuilder::new(
    journal,
    Arc::clone(&store) as Arc<dyn WakeIndexStore>,
    Arc::clone(&store) as Arc<dyn InboxStore>,
)
// Optional: claim due schedules, bridge fires through registered
// `RunStarter`s, and purge started fires older than seven days.
.cron(
    Arc::new(MemoryCronStore::new()),
    Arc::clone(&store) as Arc<dyn FireStore>,
    Duration::from_hours(7 * 24),
)
.build()?;
# Ok(())
# }
```

A worker without `cron` never reads a cron or fire table; `TickReport`
keeps its `cron_fires` and `runs_started` counters at zero so hosts reading
the report are unaffected. Registering a `RunStarter` without `cron` fails
`build` with `cron_not_configured`. With `cron`, each tick purges one
`batch_limit`-bounded batch of started fires older than the retention window
after the bridge pass, so hosts no longer schedule `purge_started` themselves
(`purge_dead_letters` remains the host's responsibility).

`WorkerHandle::last_tick` returns a `watch::Receiver<Option<TickOutcome>>`
that the spawned loop replaces after every tick with the tick's `TickReport`,
the stable code of a tick-level failure if any, and the wall-clock instant it
ran — liveness without a logging dependency.

## Stock daemon

`finstack_workflow_worker` opens the worker, cron, and journal sqlite tables
on one file and opts into cron with a seven-day fire retention. It is gated
on the `daemon` feature so library consumers do not compile
`finstack-ai-store-sqlite`. Cargo skips the binary unless that feature is
enabled.

```sh
cargo run -p finstack-ai-workflow-worker --features daemon -- <sqlite-path>
```

## Safety contract

- Production session attachment uses operating-system entropy. Seeded
  attachment is an explicit test/reproducibility API.
- Each tick reads at most the configured `batch_limit` (default `64`) from a
  scheduler table, on a rotating cursor for both wake rows and unstarted
  fires so one stuck item cannot starve the rest. Inbox consumption uses an
  exact tenant/session/pending-id lookup rather than a whole-table scan.
- Dueness is evaluated by the store, not the tick: timer and runnable rows
  wait for `wake_at`; interaction and deferred rows wait out any retry
  backoff and are then due only with a buffered response in the inbox (an
  `EXISTS` subquery in sqlite, the in-memory inbox map otherwise) or, for an
  interaction, a passed deadline. A due row whose `workflow_kind` this worker
  has no `PortsFactory` for is skipped before any claim and left untouched
  for a worker that serves it. After a claim the inbox entry is re-read; if
  it was consumed in between, the lease is released without a resume.
- A worker built with `tenant_scope` loads only that tenant's rows
  (`WakeIndexStore::load_due_scoped`; sqlite pushes the restriction into the
  query).
- Worker construction rejects empty identities, zero limits/durations, and a
  drive timeout that can outlive its lease.
- A response key is immutable. Equal bytes are idempotent; different bytes or
  kinds return `inbox_conflict` and never overwrite the accepted response.
- Active inbox rows carry a digest. Consumption and dead-letter movement are
  digest-guarded so a stale worker cannot delete a newer command.
- Runtime ingress rejections and permanently malformed commands (an
  unknown locator, an invalid normalized command, a kernel decision
  rejection, an undecodable payload) leave the active inbox and remain
  available through `load_dead_letters` until an explicit bounded purge.
  Transient submission failures (store, boundary, event-delivery or
  non-revealing ingress faults) keep the entry and back the row off; after
  `max_response_attempts` (default `8`, measured on the wake row's
  `attempts`) the entry is dead-lettered with the last failure's code.
- Fire starts store a typed `SessionId` and compare-and-set from `Claimed` to
  `Started`. Fire idempotency keys use a domain-separated digest over
  length-prefixed identity fields.
- Every successful wake claim returns a fresh `WakeLease`, even when the same
  worker reacquires the row. Renewal, release, failure backoff, repark and delete
  atomically compare that acquisition identity. Initial publication passes `None`
  and cannot overwrite a leased row. Expired leases cannot be renewed.
- A worker that loses ownership performs no further journal work for that row.

## SQLite schema

`SqliteWorkerStore` owns `finstack_workflow_worker_schema` at version `3`.
It does not modify `PRAGMA user_version`, so it can share a file with the
journal. The DDL is idempotent: a version-3 database created before the
`(tenant_scope, pending_id)` wake index existed gains it on the next open
without a version bump. Historical unversioned worker tables are intentionally rejected with
`workflow_schema_reset_required`; create a fresh adapter database for this
breaking release.

Stop all workers before upgrading a version 1 or 2 adapter. Opening it with v3
adds the claim identity column, preserves hints/inbox/fire records, and clears
legacy leases in one transaction. Restart workers only after the upgrade;
running old and new workers against the same database is unsupported. Old
binaries reject the new schema when opened. The journal format is unchanged.

Custom `WakeIndexStore` implementations must return `Option<WakeLease>` from
`try_claim` and implement atomic fence checks on all owner mutations. Callers
pass the acquired lease to renewal/release/backoff and `Some(&lease)` to repark
or delete; initial publication uses `None`. There is no unfenced owner fallback.

Started fires and dead letters have bounded retention methods. A worker
built with `cron` purges started fires itself on the configured retention;
dead letters stay until the host calls `purge_dead_letters` from maintenance
work.

## HITL integration

`WorkerBuilder::interaction_lifecycle` accepts the narrow
`InteractionLifecycle` callback. The HITL crate implements it with
`HitlLifecycle`, allowing the worker to capture interactions created during a
re-park and to report authoritative `Accepted` or `Rejected` ingress outcomes
without a crate dependency cycle.

Worker ticks await synchronous adapter and interaction-lifecycle calls on a blocking
pool with at most 16 admitted calls across workers. Dropping an await cannot cancel
a database call; its capacity remains held until the call finishes, and wake writes
still require the acquisition fence. Lease renewal runs every one-third of the
lease duration, separately from session polling. Direct synchronous store and
delivery APIs remain the caller's responsibility to schedule off an async executor.
