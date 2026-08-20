# finstack-ai-workflow-hitl

Human-in-the-loop router battery over `finstack-ai-workflow-worker`. It
durably captures every interaction a session parks on, exposes it as a
per-tenant inbox, and turns an authorized operator decision — or an
expiry policy's decision — into a resolution the worker buffers for the
next tick.

The inbox is a hint. The kernel journal stays authoritative: nothing in
this crate closes an interaction that the journal has not actually
settled, and a row can always be reconciled against the worker's wake
index if it drifts.

This crate is a T1 native mapping over `finstack-ai-workflow-worker`. It
is not isolated. See
[Technical Design §2](../../../docs/planning/03-finstack-ai-technical-design.md)
for ownership boundaries.

## Lifecycle

```text
Open ──resolve──▶ Delivered ──reconcile──▶ Closed
  │
  └──sweep (host ExpiryPolicy)──▶ Expired
```

- **`park`** — the safe entry point. It composes
  `finstack_ai_workflow_worker::park` with [`capture`], so the wake row is
  indexed before the inbox row, preserving the invariant `capture`'s own
  documentation depends on: a `sweep` racing a bare `capture` (skipping
  `park`) would find no wake row yet and reconcile the interaction to
  `Closed` before it was ever delivered. Call `capture` directly only if
  you have already upserted the wake row yourself.
- **`pending`** — `HitlRouter::pending(tenant_scope)` lists a tenant's
  `Open` rows, oldest first, for a host's inbox view.
- **`resolve`** — `HitlRouter::resolve(...)` authorizes the calling
  principal, builds an `InteractionResolutionCommand`, delivers it to the
  worker inbox, and only then flips the row to `Delivered`. Delivery
  happens before the status transition, so a rejected delivery leaves the
  row `Open` and retryable; if the store write after a successful delivery
  fails, the row stays `Open` even though the command is already buffered
  — the worker inbox's keyed upsert makes retrying `resolve` harmless.
  Once-only resolution is enforced by the kernel journal's settlement, not
  by this inbox: `HitlError::NotOpen` is a serial-use guard, not a
  concurrency guarantee.
- **`sweep`** — `HitlRouter::sweep(now)` runs in two passes over every
  `Open`/`Delivered` row across tenants. Reconcile runs first and wins:
  any active row with no matching `Interaction` wake row was already
  settled out of band (a tick consumed it), so it is closed rather than
  refused. Everything still `Open` and past its `expires_at` is then
  handed to the installed `ExpiryPolicy`.
- **`tick`** — not owned by this crate. Delivered resolutions sit in the
  worker's inbox until the host's own `WorkflowWorker::tick` (or
  `spawn`'s loop) applies them to the journal and resumes the run.

`resolve` is the only path from `Open`/`Delivered` forward under normal
operation; `reconcile` (inside `sweep`) is the only path to `Closed`.

## Stores

`HitlInboxStore` is the adapter-owned contract; two implementations ship:

- **`MemoryHitlStore`** — in-process, for tests and non-durable
  deployments.
- **`SqliteHitlStore`** — owns exactly one table,
  `finstack_workflow_hitl_inbox`, and can share its database file with
  `SqliteWorkerStore` (from `finstack-ai-workflow-worker`) or other
  adapter stores. Like the worker's own tables, it is deliberately
  **versionless**: no `PRAGMA user_version` guard, and not part of the
  kernel journal's schema version. It is a hint, so a binary that does
  not understand a column ignores it. Future changes to the table must be
  additive and nullable — new nullable columns only, never a repurposed
  or dropped one.

## Authorization and expiry hooks

Two host-replaceable hooks gate what the router will actually deliver.

### `ResolveAuthorizer`

Consulted on every `resolve` before delivery. The default,
`TenantAuthorizer`, requires the resolving principal's `tenant_scope` to
equal the row's tenant; an unscoped principal (`None`) is denied, because
the kernel treats `None` as "inherit", which this battery cannot verify.
Install a stricter authorizer with `HitlRouter::with_authorizer` — e.g.
one that also checks role or explicit interaction assignment.

### `ExpiryPolicy` — fail-closed by default

`HitlRouter::sweep` never invents a decision on a past-deadline row; it
asks the installed `ExpiryPolicy`, and `Ok(None)` leaves the row `Open`
for the next sweep. The shipped default, **`ApprovalExpiry`, declines
every row** — a sweep against a fresh `HitlRouter` expires nothing.

This is deliberate, not an oversight. A resolution only reaches the
kernel journal through the runtime's interaction ingress, which admits it
only when the resolution's principal and authorization evidence exactly
match the run's own `RunAccepted` security context — the credentials the
host presented when it *accepted* the run. That is per-run data. It is
not on the inbox row, not in the committed `InteractionRequest` (the
runtime's own approval request sets `assignee_hint: None`), and not
reachable from this crate's synchronous `sweep`. No principal this
battery authors can satisfy that check, so an earlier version of
`ApprovalExpiry` that refused under a synthetic
`("finstack.workflow.hitl", "expiry", Some(tenant))` principal produced a
resolution the ingress rejected as `scope_mismatch` on every tick: the
row was already stamped `Expired` and gone from `pending`, while the run
stalled on `RunPhase::AwaitingInteraction` forever with no operator-visible
trace. A policy that declines is strictly safer than one that lies about
what it delivered.

A host that itself accepted the run holds the credentials the ingress
requires, and can install a policy that presents them via
`HitlRouter::with_expiry_policy`. `tests/hitl/uc05.rs` proves such a
policy end to end, through a real tick, to a terminal run. See the
rustdoc on `ApprovalExpiry`, `ExpiryPolicy`, and
`HitlRouter::with_expiry_policy` for the exact ingress check, and
[the design spec §2.4](../../../docs/superpowers/specs/2026-08-20-workflow-hitl-router-design.md)
for the amended decision record.

**Recorded follow-up:** the credential-free path is a worker-driven
kernel `ExpireIfDue` input — the worker itself, not this synchronous
battery, would carry the authority to expire a run it is already ticking.
That is future work, not something this crate can retrofit today.

## Non-goals

This battery is a router, not an inbox product. It deliberately does not
provide:

- Assignment or assignee routing UI. `InteractionRequest`'s
  `assignee_hint` exists in the kernel type but this crate does not act
  on it beyond authorization.
- Reminders, notification transports, or any push/email/webhook delivery
  of pending interactions.
- SLO or aging metrics/dashboards.
- Completion-ingress interaction tokens. A future token kind, per
  [`docs/superpowers/specs/2026-08-20-completion-ingress-design.md`](../../../docs/superpowers/specs/2026-08-20-completion-ingress-design.md)
  §8, may let a token terminator call `resolve` directly; this router is
  the seam that design defers to, not an implementation of it.
- Delegation. `AuthorizationEvidence`'s delegation fields are passed
  through untouched; this crate neither issues nor validates them.

## Example

```rust
use std::sync::Arc;
use finstack_ai_workflow_hitl::{HitlRouter, MemoryHitlStore};

// `worker` and `wake` come from an already-built `WorkflowWorker` /
// `WakeIndexStore` (see `finstack-ai-workflow-worker`).
let store = Arc::new(MemoryHitlStore::new());
let router = HitlRouter::new(store, worker, wake);

// Host inbox view for one tenant:
let open = router.pending("tenant-a")?;

// Authorized operator resolution (delivers to the worker, then marks
// the row `Delivered`):
router.resolve(
    "tenant-a", interaction_id, "op-decision-1",
    principal, evidence, payload, None, now,
)?;

// Periodic maintenance: reconcile settled-out-of-band rows, then expire
// whatever the host's `ExpiryPolicy` (installed via
// `with_expiry_policy`) is willing to resolve. With no policy installed,
// this call reconciles but never expires.
let report = router.sweep(now)?;
```
