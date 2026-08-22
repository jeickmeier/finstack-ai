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
- Wake leases are claimed, renewed, and explicitly released by holder. A
  worker that loses ownership performs no further journal work for that row.

## SQLite schema

`SqliteWorkerStore` owns `finstack_workflow_worker_schema` at version `1`.
It does not modify `PRAGMA user_version`, so it can share a file with the
journal. Historical unversioned worker tables are intentionally rejected with
`workflow_schema_reset_required`; create a fresh adapter database for this
breaking release.

Started fires and dead letters have bounded retention methods. Hosts choose
their retention window and call `purge_started` / `purge_dead_letters` from
maintenance work.

## HITL integration

`WorkerBuilder::interaction_lifecycle` accepts the narrow
`InteractionLifecycle` callback. The HITL crate implements it with
`HitlLifecycle`, allowing the worker to capture interactions created during a
re-park and to report authoritative `Accepted` or `Rejected` ingress outcomes
without a crate dependency cycle.
