# finstack-ai-workflow-worker

Leased durable worker for `finstack-ai-workflow-local`. The kernel journal is
authoritative; the worker owns bounded adapter tables for wake hints, cron
fires, buffered responses, and response dead letters.

## Stock daemon

`finstack_workflow_worker` opens the worker, cron, and journal sqlite tables
on one file. It is gated on the `daemon` feature so library consumers do not
compile `finstack-ai-store-sqlite`. Cargo skips the binary unless that
feature is enabled.

```sh
cargo run -p finstack-ai-workflow-worker --features daemon -- <sqlite-path>
```

## Safety contract

- Production session attachment uses operating-system entropy. Seeded
  attachment is an explicit test/reproducibility API.
- Each tick reads at most the configured `batch_limit` (default `64`) from a
  scheduler table. Inbox consumption uses an exact tenant/session/pending-id
  lookup rather than a whole-table scan.
- Worker construction rejects empty identities, zero limits/durations, and a
  drive timeout that can outlive its lease.
- A response key is immutable. Equal bytes are idempotent; different bytes or
  kinds return `inbox_conflict` and never overwrite the accepted response.
- Active inbox rows carry a digest. Consumption and dead-letter movement are
  digest-guarded so a stale worker cannot delete a newer command.
- Runtime ingress rejections and permanently malformed commands leave the
  active inbox and remain available through `load_dead_letters` until an
  explicit bounded purge.
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
journal. Historical unversioned worker tables are intentionally rejected with
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

Started fires and dead letters have bounded retention methods. Hosts choose
their retention window and call `purge_started` / `purge_dead_letters` from
maintenance work.

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
