# finstack-ai-workflow-hitl

Human-in-the-loop router battery over `finstack-ai-workflow-worker`. It
durably captures an interaction a session parks on **through its own
`park`**, exposes it as a per-tenant inbox, and turns an authorized
operator decision into a resolution the worker buffers for the next tick.

Two things it is not: it does not capture interactions a session parks on
inside the worker's own tick (see [Limitations](#limitations)), and its
`ExpiryPolicy` hook does not decide what an expired run receives (see
[`ExpiryPolicy`](#expirypolicy--a-row-disposition-hook-not-a-run-outcome-hook)).

The inbox is a hint. The kernel journal stays authoritative: nothing in
this crate closes an interaction that the journal has not actually
settled, and a row can always be reconciled against the worker's wake
index if it drifts.

This crate is a trusted native mapping over `finstack-ai-workflow-worker` and
is not isolated. The kernel journal remains the semantic authority.

## Lifecycle

```text
Open ──resolve──▶ Delivered ──reconcile──▶ Closed
  │                                          ▲
  └──sweep (host ExpiryPolicy)──▶ Expired    │
                                             │
  (no policy installed: the worker's tick expires the interaction on the
   journal, and the next sweep reconciles the row here) ──────────────────┘
```

These are **row** states. They describe this inbox, not the run: the
journal's own settlement is the authority for what actually happened. In
particular `Expired` means "a policy authored an expiry for this row", not
"the run received that policy's payload" — see
[`ExpiryPolicy`](#expirypolicy--a-row-disposition-hook-not-a-run-outcome-hook).

- **`park`** — the recommended entry point. It composes
  `finstack_ai_workflow_worker::park` with [`capture`], so the wake row is
  indexed before the inbox row and a racing `sweep` never sees a wake-less
  row to reconcile. `capture` itself is self-healing on this point: it only
  runs for an interaction the journal still holds pending, so it resets a
  stale `Closed` row back to `Open` instead of carrying it forward (while
  still preserving `Delivered`/`Expired` settlements).
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

Tenant equality is a floor, not a sufficient check. Authorize against the
run's **accepted principal** where you can: that is the only principal the
runtime will admit (next section), so a resolution this battery authorizes
on tenant alone can still be rejected by the ingress on every tick.

### Resolve credentials — `Delivered` is not `accepted`

`resolve` delivers into the worker's inbox and marks the row `Delivered`.
That means **durably buffered**, and nothing more. The runtime's
interaction ingress admits a resolution only when its principal *and*
authorization evidence exactly equal the run's own `RunAccepted` security
context — the credentials the host presented when it accepted the run
(`authorization_matches`,
`crates/finstack-ai-runtime/src/driver/ingress/shared.rs`).

Deliver a mismatched principal or mismatched policy-version/decision-id
evidence and the failure is silent and permanent from this inbox's point
of view: every tick rejects the buffered command as `scope_mismatch`, the
worker never settles anything, the run stays parked on
`RunPhase::AwaitingInteraction`, and the row sits at `Delivered` forever —
out of `pending()`, so out of the operator's view. The worker's retry
backoff never gives up, so this does not self-heal.

Pass the accepted run's own principal and evidence through to `resolve`.
If a host cannot, it should not deliver at all — leave the row `Open`,
where an operator can still see it, and let the deadline expire the
interaction through the worker (below).

### `ExpiryPolicy` — a row-disposition hook, not a run-outcome hook

`HitlRouter::sweep` never invents a decision on a past-deadline row; it
asks the installed `ExpiryPolicy`, and `Ok(None)` leaves the row `Open`
for the next sweep. The shipped default, **`ApprovalExpiry`, declines
every row** — a sweep against a fresh `HitlRouter` expires nothing.

**What a policy can and cannot do.** A policy's authored payload reaches
the journal *as that payload* only if it is submitted **before** the
deadline. A sweep cannot arrange that: by construction it only hands a
row to the policy once `now >= expires_at`. The worker then submits the
buffered command with its own clock, which under a single shared clock —
what a real host has — is also `>= expires_at`. And the runtime's
interaction ingress is fail-closed on a late answer:
`interaction_settled_input`
(`crates/finstack-ai-runtime/src/driver/ingress/shared.rs`) rewrites any
resolution whose `submitted_at` is at or after the pending request's
`expires_at` into `InteractionSettled::Expired` before the reducer sees
it. The authored payload is discarded, and the journal records a plain
expiry with no resolution identity.

So an `ExpiryPolicy` controls **this router's row** — its status and its
`resolved_by` — and not the run's outcome. `tests/hitl/uc05.rs`
(`uc05_expiry_end_to_end`) runs exactly this on one clock and asserts it:
the policy delivers, the row becomes `Expired` with the policy's
principal recorded, and the journal's terminal outcome is nonetheless
`Expired` rather than `Denied`, with `resolution_identities` empty.

The API stays — it is a published surface, and the row annotation is
genuinely useful for a host's own inbox reporting. Just do not read
`Expired`/`resolved_by` as a claim about what the run received.

**What actually settles the run is the worker, credential-free.**
`finstack-ai-workflow-worker` persists the committed request's deadline on
`WakeRow::expires_at` at park time, and its tick claims a past-deadline
interaction row on the clock alone — attaching the session is what commits
the expiry, because the runtime applies `ExpireIfDue` inside every run
owner constructor; no input is submitted and no credentials are presented.
Each committed expiry is counted in `TickReport::sessions_expired`. An
unanswered interaction therefore expires whether or not a policy is
installed, and a declining `ApprovalExpiry` costs the run nothing. With no
policy installed the next sweep reconciles the row to `Closed`.

**Why declining is the right default.** A resolution reaches the journal
only through the ingress check above, which also requires the resolution's
principal and authorization evidence to exactly match the run's own
`RunAccepted` security context — the credentials the host presented when
it *accepted* the run. That is per-run data: not on the inbox row, not in
the committed `InteractionRequest` (the runtime's own approval request
sets `assignee_hint: None`), and not reachable from this crate's
synchronous `sweep`. An earlier `ApprovalExpiry` that refused under a
synthetic `("finstack.workflow.hitl", "expiry", Some(tenant))` principal
produced a resolution the ingress rejected as `scope_mismatch` on every
tick: the row was already stamped `Expired` and gone from `pending`,
while the run stalled on `RunPhase::AwaitingInteraction` forever with no
operator-visible trace. Declining is strictly safer than lying.

See the rustdoc on `ApprovalExpiry`, `ExpiryPolicy`, and
`HitlRouter::with_expiry_policy`, and
[the design spec §2.4](../../../docs/superpowers/specs/2026-08-20-workflow-hitl-router-design.md)
for the amended decision record.

## Limitations

- **Interactions the worker's tick parks on are not captured.** This
  battery captures at its own `park`. When the worker's tick resumes a
  session and that session parks on a *new* interaction, the worker's own
  `finstack_ai_workflow_worker::park` writes the wake row with no HITL
  capture, so the new interaction never appears in `pending()` and
  `resolve()` answers `HitlError::UnknownInteraction`. The run is not
  lost — it is parked and wake-indexed as usual — but it is invisible to
  this inbox. Workaround: the host delivers the resolution to the worker
  directly with `WorkflowWorker::deliver_interaction`, bypassing the
  router. A structural fix (capturing from the worker's re-park) is
  tracked separately.
- **`Delivered` means "durably buffered", not "accepted".** A resolution
  whose credentials the runtime's ingress refuses is rejected on every
  tick, forever, while the row reads `Delivered` — see
  [Resolve credentials](#resolve-credentials--delivered-is-not-accepted).

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
