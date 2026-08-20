# Workflow HITL Router — Design

**Date:** 2026-08-20
**Status:** Approved for planning
**Path:** `extensions/workflow/finstack-ai-workflow-hitl`
**Plan:** `docs/superpowers/plans/2026-08-20-workflow-hitl-router.md`
**Depends on:** `docs/superpowers/specs/2026-08-19-workflow-worker-design.md` (worker plan Tasks 9–12 merged to `main`, including `WorkflowWorker::deliver_interaction`)
**Related:** PRD UC-05 (`docs/planning/01-finstack-ai-product-requirements.md:190-192`), future-capabilities HITL rows (`docs/planning/05-finstack-ai-future-capabilities-design-validation.md:314, 1476`), completion-ingress design (interaction tokens explicitly deferred there to this component)

## 1. Problem

UC-05 promises a run that "pauses on a typed human interaction such as an approval, choice, form, review, or correction; survives process restart; resumes after an authorized external response; and does not repeat completed effects." Today that promise is demonstrated, not shipped:

- `examples/durable-interaction` proves only the restart half: it parks on `WorkflowWait::Interaction`, reopens the store, and submits one hand-built `InteractionResolutionCommand`. It never lists what is pending, never checks who may resolve, never expires anything, and never drives the run to terminal.
- The workflow worker's wake index records *that* a session is parked on an interaction (`WakeReason::Interaction`, `pending_id`), but not *what* was asked: the `InteractionRequest` envelope — kind, prompt, response schema, `expires_at`, assignee hint — is dropped at park time. A host cannot render an approval queue from the wake index.
- `InteractionRequest.expires_at` is populated (the runtime's approval interactions set it to the run's effective deadline) but nothing in the tree ever fires it. An unanswered approval parks forever.
- Authorization is thin: the kernel enforces TM-19 locator identity and requires a `PrincipalRef` + `AuthorizationEvidence` on every resolution, but nothing checks that the *principal* is entitled to answer this interaction. Any caller who can reach `resolve_interaction` with the right locator can resolve anything.

The gap is a router battery: an interaction inbox with expiry timers and authorized resolve, layered on the worker. Explicitly **not** in scope: assignment UI, reminders, or SLOs.

## 2. Decision

Build `finstack-ai-workflow-hitl` as a publishable battery under `extensions/workflow/`, layered strictly on top of `finstack-ai-workflow-worker`:

1. **Capture at park time.** A `park` wrapper composes the worker's `park`: it classifies the wait first, delegates to `finstack_ai_workflow_worker::park`, and when the wait is `WorkflowWait::Interaction` writes one row to a new adapter-owned table (`finstack_workflow_hitl_inbox`) containing the locator coordinates and the full serialized `InteractionRequest`. The journal remains the authority; the HITL inbox is a queryable hint, exactly like the wake index.
2. **Resolve through the worker's durable inbox, never directly.** `HitlRouter::resolve` authorizes the principal, builds the `InteractionResolutionCommand`, and hands it to `WorkflowWorker::deliver_interaction` — an insert-only durable write. The worker's tick loop performs the actual kernel submission with its existing TM-19-guarded `resolve_interaction` path and duplicate-settlement safety. The router never bypasses that path, so resolutions delivered while everything is down survive, and redelivery stays idempotent.
3. **Authorization is a fail-closed hook.** A `ResolveAuthorizer` trait decides whether a `PrincipalRef` may resolve a given pending request. The default (`TenantAuthorizer`) requires the principal's tenant to equal the interaction's `tenant_scope`; hosts plug in real policy. Kernel-side locator and evidence checks remain the last line — the router adds principal-level policy, it does not replace kernel validation.
4. **Expiry is a sweep, not a timer service.** `HitlRouter::sweep(now)` finds open rows with `expires_at <= now` and asks an `ExpiryPolicy` for a resolution. The default policy answers only `InteractionKind::Approval` (a refusal payload `{"approved": false}` under a router-owned principal); other kinds stay parked unless the host supplies a policy. Expiry resolutions travel through the same worker inbox. The host calls `sweep` on whatever cadence drives the worker tick — the router owns no threads, no clocks, no wall time.
5. **Reconcile against the wake index.** Resolutions can legitimately bypass the router (a host calling `deliver_interaction` directly, or the run being cancelled). `sweep` therefore also closes any active row whose interaction wake row has disappeared. Row status is `Open → Delivered → Closed`, or `Open → Expired`, and reconcile is the only path to `Closed`.

## 3. Alternatives rejected

- **Extend the worker crate itself.** The worker's charter is leasing, cron, and resume — deliberately minimal. HITL inbox semantics (request envelopes, authorization, expiry policy) are optional battery territory; per the engineering standards, enabling the worker must not silently ship interaction-routing policy.
- **Kernel- or runtime-side expiry.** Future-capabilities doctrine places schedule/ownership/missed-run policy on the adapter, never the kernel. `expires_at` is semantic metadata; what expiry *means* (refuse? escalate? nothing?) is host policy, hence a battery hook.
- **Querying the journal for pending interactions.** `JournalStore` has no list/query API by design; deriving "pending" requires load + recover + `classify_wait` per session. The wake index exists precisely because scanning journals is not a listing mechanism; the HITL inbox extends that pattern with the request payload.
- **Direct `resolve_interaction` from the router.** Loses durability (a resolution accepted while the worker host is down would vanish) and duplicates the worker's attach/drive/park choreography. The worker inbox already provides exactly-once-ish delivery against journal settlement.

## 4. Data

Table `finstack_workflow_hitl_inbox`, PK `(tenant_scope, interaction_id)`:

| column | content |
|---|---|
| `tenant_scope`, `session_id`, `lane_id`, `run_id` | locator coordinates, canonical strings |
| `interaction_id` | canonical `InteractionId` string (PK member) |
| `kind` | interaction kind token (e.g. `approval`) for cheap filtering |
| `requested_at`, `expires_at` | unix ms; `expires_at` nullable |
| `request` | serialized `InteractionRequest` (serde_json bytes), deserialized at resolve/expiry time |
| `status` | `open` \| `delivered` \| `expired` \| `closed` |
| `resolved_by` | principal subject that resolved, nullable |
| `updated_at` | unix ms |

Store trait `HitlInboxStore` with `MemoryHitlStore` and `SqliteHitlStore` implementations, mirroring the worker's store shape. `SqliteHitlStore` may share a database file with `SqliteWorkerStore`; it owns only its table.

## 5. Non-goals (v1)

- Assignment UI, assignee routing, reminders, notification delivery, SLOs/aging metrics.
- Completion-ingress tokens for interactions (future token kind per the completion-ingress design).
- Python/JS bindings (Rust battery first; bindings follow the usual promotion path).
- Delegation semantics (`delegatable` is stored with the envelope, unused).

## 6. UC-05 acceptance

The battery's integration suite, plus an upgraded `examples/durable-interaction`, must show the full UC-05 sentence end to end: park on a typed approval → process death → restart → `pending()` lists the request → authorized resolve via the router → worker tick resumes → run reaches terminal with the tool executed exactly once. A second test shows the expiry path: unanswered approval crosses `expires_at`, `sweep` refuses it, tick resumes, the model sees the refusal outcome.
