# finstack-ai-workflow-worker — Design

**Date:** 2026-08-19
**Status:** Approved design, pre-implementation
**Path:** `extensions/workflow/finstack-ai-workflow-worker`

## 1. Problem

`finstack-ai-workflow-local` is an in-process library. A host still has to
attach, fire cron, and call `drive_until_wait`. Nothing wakes a parked
session after the process dies: cron fires are recorded but never start
runs, timers are only observed when a host attaches, and interaction /
external responses have nowhere durable to land. The missing product is a
leased worker daemon, per future-capabilities §10.2
(`docs/planning/05-finstack-ai-future-capabilities-design-validation.md:820-833`),
which places schedule definition, next-fire calculation, **ownership and
leasing**, and **missed-run policy** on the scheduler/adapter — never the
kernel — and requires scheduler-started runs to carry idempotency keys.

## 2. Decisions (made during brainstorming)

- **Deployment target:** single-host daemon against SQLite. Multi-worker
  claim semantics are still enforced (lease CAS + journal append CAS), but
  Postgres/HA is out of scope.
- **Missed cron fires:** coalesce — at most one catch-up fire per schedule
  on restart, then normal cadence. This is already the behavior of
  `LocalWorkflowDriver::fire_due` catch-up; the worker inherits it.
- **v1 wake sources:** Timer, Interaction, DeferredEffect, and cron fires.
- **Run construction:** host-registered factories. The worker is a
  library-with-binary; the host registers a `PortsFactory` per workflow
  kind and a `RunStarter` per cron schedule. The worker owns polling,
  leasing, and the clock; the host owns construction. No serialized
  port-config format.
- **Interaction/deferred delivery:** durable inbox table (§7), so
  responses arriving while everything is down survive to the next start.
- **Discovery:** adapter-owned wake index table (Approach A). Rejected:
  polling a host-registered roster (O(journal) recover per session per
  tick, roster dies with the process) and extending `JournalStore` with
  list/status queries (violates the deliberate no-lookup-side-channel
  design of `LoadRequest` and §10.6's verdict that scheduling stays
  outside the kernel).

## 3. Reality constraints (verified against the code)

- `JournalStore` has no list/query API; loads require a known
  `session_id` (`crates/finstack-ai-runtime/src/ports/journal.rs:55-140`).
  "Parked" is derived state: load + recover + `classify_wait`
  (`crates/finstack-ai-runtime/src/driver/workflow/mod.rs:166-212`).
  Hence the wake index is net-new and adapter-owned.
- The cron store already has the claim pattern to copy:
  `SqliteCronStore::try_claim` uses `BEGIN IMMEDIATE` + conditional
  UPDATE, winner = `changes() == 1`, with a two-process test asserting
  exactly one winner
  (`extensions/workflow/finstack-ai-workflow-local/src/store.rs:283-323`,
  test at `:396-430`). The trait default fails closed.
- `CronFire` does not start runs ("Starting a run remains a caller
  action", `cron.rs:199`) and carries no idempotency key. The worker adds
  the fire→run bridge (§6).
- The session clock is `ExternalClock`, a manually advanced
  `Arc<AtomicI64>` — not wall time
  (`crates/finstack-ai-runtime/src/services/id_generation.rs:111-155`).
  The worker pumps it from `SystemClock` each tick; no API widening.
- `drive_until_wait` has a hard-coded 2 s timeout and performs a full
  journal recover per poll iteration (`driver/workflow/mod.rs:523-538`).
  The timeout becomes configurable (§8); recover cost is accepted for v1.
- TM-19 (`docs/planning/06-finstack-ai-security-threat-model.md:182`) is
  an identifier-swapping control, **not** a leasing requirement. It
  constrains the worker to take tenant scope from the acquired handle
  (enforced by `require_locator` on `resolve_interaction` /
  `complete_external`) and requires cross-tenant negative tests on every
  new store surface. Leasing itself is motivated by §10.2, not TM-19.
- Cron expressions are `every <n>ms|s|m` only (`IntervalSchedule`,
  `cron.rs:67-133`). Calendar/crontab expressions are explicitly out of
  scope for v1 and would be a new type in `finstack-ai-workflow-local`,
  not a new package.

## 4. Components

New workspace member `extensions/workflow/finstack-ai-workflow-worker`,
crate `finstack-ai-workflow-worker`. Dependencies:
`finstack-ai-workflow-local`, `finstack-ai-runtime`, `rusqlite`, `tokio`
(first tokio dependency among workflow extensions — it is the daemon).

1. **`WakeIndexStore` trait + `SqliteWakeStore`** — discovery index
   (§5). Trait methods default to fail-closed errors, mirroring
   `CronScheduleStore`.
2. **`park()` helper** — the only sanctioned write path into the index
   (§5).
3. **`WorkflowWorker`** — the tick loop (§9).
4. **`WorkerBuilder` / `WorkerHandle`** — host registration and
   lifecycle: `register_ports(kind, PortsFactory)`,
   `register_cron_starter(schedule_id, RunStarter)`, `poll_interval`,
   `lease_ttl`, `worker_id`; `spawn()` → `WorkerHandle` with graceful
   shutdown. An optional `[[bin]]` wraps the builder as a stock daemon.

## 5. Wake index

Table `finstack_workflow_worker_wake`:

| column | notes |
|---|---|
| `tenant_scope`, `session_id` | primary key |
| `lane_id`, `run_id` | to rebuild the `OperationLocator` |
| `workflow_kind` | selects the host's `PortsFactory` |
| `reason` | `timer` \| `interaction` \| `deferred` |
| `wake_at_unix_ms` | timer due time; NULL for interaction/deferred |
| `effect_or_interaction_id` | the pending id from `classify_wait` |
| `leased_by`, `lease_expires_at_unix_ms` | lease (§8) |
| `attempts` | failure backoff counter |

**The index is a hint, never authority.** The journal remains truth. On
claim, the worker attaches and re-runs `classify_wait`; if the row
disagrees with recovered kernel state, the row is corrected (or deleted)
and the recovered state wins. Metadata never grants authority — same
doctrine as `WriteMetadataRequest`.

**Write path:** `park(driver: &mut LocalWorkflowDriver, store,
workflow_kind: &str) -> WorkflowCheckpoint` (the host names the kind it
will later register a `PortsFactory` under) classifies the wait, upserts the wake row, calls
`persist_handoff()`, aborts the owner, and returns the checkpoint. Hosts
call `park()` instead of hand-rolling handoff so the index cannot go
stale at park time. Sessions parked without `park()` are simply invisible
to the worker — acceptable v1 semantics, documented in the README.

## 6. Cron→run bridge (idempotency)

Table `finstack_workflow_worker_fires`: PK `(tenant_scope, schedule_id,
fire_count)`, columns `status` (`claimed` | `started`), `session_id`
(NULL until started). `fire_count` is already CAS-maintained on the
schedule row, so `(schedule_id, fire_count)` is the idempotency key §10.2
requires.

Flow: claim the fire via the existing `try_claim`/`fire_due` path →
insert `claimed` row → invoke the registered `RunStarter` (which creates
the session/run through public APIs) → update row to `started` with the
new `session_id`. On startup, `claimed`-but-not-`started` rows are
re-driven; a crash between claim and start can neither lose nor
double-run a fire. A `RunStarter` must be registered for a schedule to
fire into a run; fires for unregistered schedules are logged and left
`claimed` (fail closed, visible).

## 7. Inbox (interaction / deferred delivery)

Table `finstack_workflow_worker_inbox`: PK `(tenant_scope, session_id,
effect_or_interaction_id)`, columns `kind` (`interaction` | `external`),
`payload` (serialized response/outcome), `received_at_unix_ms`.

Hosts call `worker.deliver_interaction(locator, interaction_id,
response)` / `deliver_external(locator, handle, outcome)`; both only
insert a row (durable immediately, no live worker required). The tick
loop joins inbox rows against wake rows; on a match it claims the wake
row, attaches, calls `resolve_interaction` / `complete_external` (both
TM-19-guarded by `require_locator`), drives, and deletes the consumed
inbox row in the same transaction as the wake-row rewrite. Unmatched
inbox rows (response arrived before the session parked, or for an unknown
session) are retained and retried each tick; the journal's own duplicate
settlement (§10.4: duplicate callbacks settle against the same
`EffectId`) makes redelivery safe.

## 8. Lease protocol

Copy of the cron claim shape, plus expiry:

```sql
BEGIN IMMEDIATE;
UPDATE finstack_workflow_worker_wake
SET leased_by = :worker_id,
    lease_expires_at_unix_ms = :now + :ttl
WHERE tenant_scope = :tenant AND session_id = :session
  AND (leased_by IS NULL OR lease_expires_at_unix_ms <= :now);
-- winner = changes() == 1
```

Long drives renew at `ttl/2` heartbeats. Crashed workers' leases lapse
and are claimed by any survivor. Defense in depth: even if two workers
drive the same run, journal append CAS on `expected_sequence`
(`extensions/stores/finstack-ai-store-sqlite/src/append.rs:82-90`) makes
the loser fail loudly rather than corrupt. On completion the wake row is
deleted (terminal phase) or rewritten via `park()` (new wait).

Runtime prerequisite: the hard-coded 2 s `drive_until_wait` timeout
becomes configurable (builder method on `WorkflowSession`, default
unchanged — all existing tests hold). Worker drives use a timeout below
`lease_ttl` so a lease cannot expire mid-drive without a heartbeat.

## 9. Tick loop

Every `poll_interval`:

1. **Pump the clock:** `external_clock.set(SystemClock::now())`.
2. **Fire due cron:** existing claim + `fire_due` (coalesced catch-up is
   built in).
3. **Bridge claimed fires to runs** (§6).
4. **Claim due wake rows:** timers with `wake_at <= now`, plus
   interaction/deferred rows having a matching inbox entry. For each:
   build ports via the `PortsFactory` for `workflow_kind`, attach,
   resolve/complete if applicable, `drive_until_wait`, then re-park or
   delete the row.
5. **Failure isolation:** a per-session error logs, increments
   `attempts`, and releases the lease with exponential backoff
   (`wake_at` pushed forward for timers; a backoff column consulted for
   inbox-driven rows). One poisoned session never stalls the loop or
   other tenants.

## 10. Error handling

- All `WakeIndexStore` / inbox / fires trait defaults fail closed
  (`..._unsupported` errors), matching `CronScheduleStore`.
- `WorkflowDriverError::DriveTimeout` → release lease with backoff, do
  not delete the row.
- Unknown `workflow_kind` (no registered factory) → log once per
  session, leave row unleased; visible, never silently dropped.
- Schema versioning: `PRAGMA user_version`-style guard consistent with
  the sqlite store's fail-closed unknown-version behavior.

## 11. Testing

Executable-spec style, mirroring
`extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/`:

- **Claim:** two-worker tests asserting exactly one winner for wake rows
  and fire rows (same shape as `store.rs:396-430`).
- **Lease expiry:** worker A claims and "crashes" (no heartbeat); worker
  B takes over after expiry and completes the resume.
- **Restart per wait reason:** park → drop everything → new process →
  worker resumes Timer, Interaction (via inbox), DeferredEffect (via
  inbox), each to completion.
- **Cron bridge:** fire → run started exactly once across a crash
  injected between claim and start; coalesced catch-up still holds.
- **Tenant negatives:** every new store method rejects cross-tenant
  access (TM-19 evidence for the new surface); locator mismatch on
  resolve/complete still fails via `require_locator`.
- **Loop tests:** tokio paused time; no real sleeps.
- **Index-is-a-hint:** a wake row disagreeing with journal state is
  corrected, and the journal outcome wins.

## 12. Out of scope (explicitly)

- Postgres store, HA, multi-host — the trait boundaries (`WakeIndexStore`
  etc.) are where that lands later.
- Calendar/crontab expressions, timezones, DST — future work inside
  `finstack-ai-workflow-local`.
- Kernel/`JournalStore` changes of any kind.
- Detached-run delivery targets, webhooks, retention (§10.3 territory).

## 13. Housekeeping absorbed into this work

- Make `drive_until_wait` timeout configurable
  (`crates/finstack-ai-runtime/src/driver/workflow/mod.rs:523`), default
  unchanged.
- Fix stale comment referencing the deleted Temporal sibling
  (`extensions/workflow/finstack-ai-workflow-local/src/lib.rs:5-6`).
