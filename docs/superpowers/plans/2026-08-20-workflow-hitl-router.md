# Workflow HITL Router Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `finstack-ai-workflow-hitl`, a battery that routes `WorkflowWait::Interaction` parks into a durable, queryable interaction inbox with expiry timers and authorized resolve — making UC-05 shippable machinery instead of an example.

**Architecture:** A leaf crate layered on `finstack-ai-workflow-worker`. Capture happens in a `park` wrapper that classifies the wait (`classify_wait`, `crates/finstack-ai-runtime/src/driver/workflow/mod.rs:155-212` on pre-worker main; offsets shift after the worker's Task 1 lands) before delegating to the worker's `park` (`extensions/workflow/finstack-ai-workflow-worker/src/park.rs:19-60`), then persists the full `InteractionRequest` envelope from `WorkflowWait::Interaction { interaction_id, request }` into a new `finstack_workflow_hitl_inbox` table. Resolve and expiry both feed `WorkflowWorker::deliver_interaction` (worker plan Task 10) — insert-only durable delivery; the worker tick performs the TM-19-guarded kernel submission. The journal stays authoritative; this inbox, like the wake index, is a hint reconciled on sweep.

**Tech Stack:** Rust; deps `finstack-ai-kernel`, `finstack-ai-runtime` (`default-features = false, features = ["native-tokio"]`), `finstack-ai-workflow-worker`, `serde`, `serde_json`, `thiserror`, plus the same SQLite dependency line `finstack-ai-workflow-worker/Cargo.toml` uses (copy it verbatim). Dev-deps: `finstack-ai-test`, `finstack-ai-store-sqlite`, `tokio` (`rt`, `macros`, `time`), `tempfile`.

**Spec:** `docs/superpowers/specs/2026-08-20-workflow-hitl-router-design.md`

## Global Constraints

- **Hard prerequisite:** the workflow-worker plan (`docs/superpowers/plans/2026-08-19-workflow-worker.md`) is fully merged to `main` — including Task 9 (`src/worker.rs`), Task 10 (`deliver_interaction` / `deliver_external`), and its public-api baselines. If any consumed signature below drifted during that merge, record the difference in a "Spec deviations" section at the top of this plan (pattern: `docs/superpowers/plans/2026-08-20-tool-policy-middleware.md`) and adapt; do not fork the worker's API.
- Copy the crate lint header verbatim from `extensions/workflow/finstack-ai-workflow-worker/src/lib.rs` (`#![forbid(unsafe_code)]`, clippy `unwrap_used`/`expect_used`/`panic`/`unreachable` denies, `warn(missing_docs)`, test re-allows).
- Error codes are `&'static str` snake_case, exposed via `pub const fn code(&self) -> &'static str`, matching `WorkerError` (`src/error.rs`).
- No wall clock, no threads, no `SystemTime::now()`: every entry point takes `now: Timestamp` (or reuses the worker's `ExternalClock`). Loop tests use tokio paused time — no real sleeps.
- The journal is authority; the HITL inbox is a hint. Never write kernel state from this crate; the only kernel-facing writes go through `WorkflowWorker::deliver_interaction`.
- Fail closed: authorization denials, unknown interactions, and store errors return `Err`, never a silent no-op.
- Run checks with workspace binaries and check exit codes explicitly: `cargo test -p finstack-ai-workflow-hitl`, `cargo clippy -p finstack-ai-workflow-hitl --all-targets -- -D warnings` — never a summarizing proxy (`tsc`/`eslint`-style wrappers are known to report false success).
- Commit after every green cycle; short imperative Conventional Commit subjects.
- Out of scope (do not add): assignment/assignee routing, reminders, notifications, SLOs, bindings, completion-ingress tokens, delegation.

## Current-state map (read before starting)

| Fact | Where |
|---|---|
| `WorkflowWait::Interaction { interaction_id: InteractionId, request: InteractionRequest }` carries the committed envelope by value | `crates/finstack-ai-runtime/src/driver/workflow/mod.rs:36-69` |
| `classify_wait(&KernelState) -> Option<WorkflowWait>`; interaction precedence is second after terminal | same file, `:155-212` |
| `WorkflowCheckpoint` public fields: `tenant_scope: Arc<str>`, `session_id: SessionId`, `lane_id: LaneId`, `run_id: RunId`, `last_applied_seq`, `external_handles` | same file, `:84-100` |
| Worker `park(session, wake, workflow_kind) -> Result<WorkflowCheckpoint, WorkerError>`; interaction rows upsert `wake_at: None`, `pending_id = interaction_id.to_canonical_string()` | `extensions/workflow/finstack-ai-workflow-worker/src/park.rs:19-60` |
| Worker inbox: `deliver_interaction(&InteractionResolutionCommand, received_at)` inserts an `InboxRow`; tick joins inbox × wake, resolves, deletes row transactionally | worker plan Task 10 (`:1372-1391`) + spec §7 |
| `WakeIndexStore::load_tenant(&str) -> Vec<WakeRow>`; `WakeRow.reason: WakeReason`, `WakeRow.pending_id: Arc<str>` | worker `src/wake.rs:52-110` |
| Resolution construction pattern (principal, evidence, payload, note) | `examples/durable-interaction/src/main.rs:469-484` |
| Runtime approval interactions set `expires_at` = run effective deadline and `kind = InteractionKind::Approval` | `crates/finstack-ai-runtime/src/exec/settlement/interaction.rs:88-145` |
| Approval is a profile of interaction, not a separate record family; `classify_wait`/`resolve_interaction` are kind-agnostic — approval HITL comes for free | `crates/finstack-ai-kernel/src/effects/interaction.rs:24-41` |
| Battery packaging = publishable Cargo.toml, README, docs/site row, public-api baseline | `docs/superpowers/plans/2026-08-20-verify-battery-promotion.md` Task 6 |
| End-to-end interaction fixture that drives to completion (reuse its shape) | `extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/restart.rs:143-318` |

---

### Task 1: Crate scaffold and `HitlError`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/Cargo.toml`
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/lib.rs`
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/error.rs`
- Modify: `Cargo.toml` (workspace members alphabetical next to `finstack-ai-workflow-worker`; add `finstack-ai-workflow-hitl = { path = "extensions/workflow/finstack-ai-workflow-hitl", version = "1.0.0" }` to `[workspace.dependencies]`)
- Test: `extensions/workflow/finstack-ai-workflow-hitl/src/error.rs` (unit tests in-module, matching the worker's style)

**Interfaces:**
- Consumes: `finstack_ai_workflow_worker::WorkerError`.
- Produces:

```rust
/// Errors surfaced by the HITL router battery.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HitlError {
    /// Backing store rejected an operation.
    #[error("hitl store unavailable: {code}")]
    StoreUnavailable { code: &'static str },
    /// Persisted row failed validation on read.
    #[error("hitl store integrity: {code}")]
    StoreIntegrity { code: &'static str },
    /// No inbox row for the requested interaction.
    #[error("unknown interaction")]
    UnknownInteraction,
    /// Row exists but is not resolvable (already delivered/expired/closed).
    #[error("interaction is not open")]
    NotOpen,
    /// Authorizer refused the principal.
    #[error("resolution not authorized: {code}")]
    Unauthorized { code: &'static str },
    /// Resolution inputs failed kernel construction.
    #[error("invalid resolution: {code}")]
    InvalidResolution { code: &'static str },
    /// Worker delivery failed.
    #[error(transparent)]
    Worker(#[from] finstack_ai_workflow_worker::WorkerError),
}

impl HitlError {
    #[must_use]
    pub const fn code(&self) -> &'static str; // passthrough | "unknown_interaction" | "not_open" | worker.code()
}
```

- [ ] **Step 1: Write the failing test** — in `src/error.rs`, `#[cfg(test)]` asserting `HitlError::UnknownInteraction.code() == "unknown_interaction"`, `NotOpen.code() == "not_open"`, `Unauthorized { code: "tenant_mismatch" }.code() == "tenant_mismatch"`, and `HitlError::from(WorkerError::NotParked).code() == "not_parked"`.
- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-hitl error` → expect a compile failure (crate/type absent).
- [ ] **Step 3: Implement** — Cargo.toml (workspace metadata inheritance, `readme = "README.md"`, no `publish = false` — this is a battery from day one; `[lints] workspace = true`), lint-headed `lib.rs` with `mod error; pub use error::HitlError;`, the enum and `code()` above. Register workspace member + dependency.
- [ ] **Step 4: Run to verify pass** — `cargo test -p finstack-ai-workflow-hitl` and `cargo clippy -p finstack-ai-workflow-hitl --all-targets -- -D warnings`, both exit 0.
- [ ] **Step 5: Commit** — `git commit -m "feat(workflow): scaffold finstack-ai-workflow-hitl battery"`

---

### Task 2: `InteractionRow`, `HitlInboxStore`, `MemoryHitlStore`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/row.rs`
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/store.rs`
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/memory.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-hitl/src/lib.rs` (module wiring + re-exports)
- Test: `extensions/workflow/finstack-ai-workflow-hitl/tests/hitl/main.rs`, `tests/hitl/store.rs`

**Interfaces:**
- Consumes: `SessionId`, `LaneId`, `RunId`, `Timestamp` from `finstack-ai-kernel` (same import paths the worker's `wake.rs` uses).
- Produces:

```rust
/// Lifecycle of an inbox row. Canonical tokens: "open" | "delivered" | "expired" | "closed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionStatus { Open, Delivered, Expired, Closed }
impl InteractionStatus {
    #[must_use] pub const fn as_str(self) -> &'static str;
    pub fn parse(value: &str) -> Result<Self, HitlError>; // StoreIntegrity { code: "interaction_status" }
}

/// One pending or settled interaction, keyed by (tenant_scope, interaction_id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionRow {
    pub tenant_scope: Arc<str>,
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: RunId,
    /// Canonical `InteractionId` string; equals the wake row's `pending_id`.
    pub interaction_id: Arc<str>,
    /// Interaction kind token (e.g. "approval") for filtering without deserializing.
    pub kind: Arc<str>,
    pub requested_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    /// serde_json bytes of the committed `InteractionRequest` envelope.
    pub request: Arc<[u8]>,
    pub status: InteractionStatus,
    /// Subject of the resolving principal, once delivered/expired.
    pub resolved_by: Option<Arc<str>>,
    pub updated_at: Timestamp,
}

/// Adapter-owned HITL inbox. A hint; the journal is authority.
pub trait HitlInboxStore: Send + Sync {
    fn upsert(&self, row: &InteractionRow) -> Result<(), HitlError>;
    fn load(&self, tenant_scope: &str, interaction_id: &str)
        -> Result<Option<InteractionRow>, HitlError>;
    /// Open rows for one tenant, ordered by requested_at then interaction_id.
    fn load_open(&self, tenant_scope: &str) -> Result<Vec<InteractionRow>, HitlError>;
    /// Every Open or Delivered row across tenants (sweep input), same ordering.
    fn load_active(&self) -> Result<Vec<InteractionRow>, HitlError>;
    fn set_status(&self, tenant_scope: &str, interaction_id: &str, status: InteractionStatus,
        resolved_by: Option<&str>, updated_at: Timestamp) -> Result<(), HitlError>;
}

pub struct MemoryHitlStore { /* Mutex<BTreeMap<(Arc<str>, Arc<str>), InteractionRow>> */ }
impl MemoryHitlStore { #[must_use] pub fn new() -> Self; }
impl HitlInboxStore for MemoryHitlStore { /* all five */ }
```

`set_status` on a missing row returns `Err(HitlError::UnknownInteraction)`. `upsert` replaces an existing row wholesale (redelivery-safe, mirrors the worker inbox contract).

- [ ] **Step 1: Write the failing test** — `tests/hitl/store.rs` against `MemoryHitlStore`: upsert two tenants × two rows; `load_open` returns only that tenant's `Open` rows in `requested_at` order; `load` round-trips every field; `set_status(.., Delivered, Some("alice"), ts)` is visible on reload and drops the row from `load_open` but keeps it in `load_active`; `set_status` on an unknown id errors `unknown_interaction`; `InteractionStatus::parse("bogus")` errors `interaction_status`. Build row fixtures with kernel id constructors exactly as `finstack-ai-workflow-worker/tests` builds `WakeRow` fixtures (copy that helper style).
- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-hitl store` → compile failure.
- [ ] **Step 3: Implement** — row.rs, store.rs, memory.rs per the signatures; wire modules and re-export `HitlInboxStore, InteractionRow, InteractionStatus, MemoryHitlStore` from lib.rs.
- [ ] **Step 4: Run to verify pass** — `cargo test -p finstack-ai-workflow-hitl` and clippy, exit 0.
- [ ] **Step 5: Commit** — `git commit -m "feat(workflow): hitl inbox row, store trait, memory store"`

---

### Task 3: `SqliteHitlStore`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/sqlite.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-hitl/src/lib.rs`, `Cargo.toml` (SQLite dep)
- Test: `extensions/workflow/finstack-ai-workflow-hitl/tests/hitl/sqlite.rs`

**Interfaces:**
- Consumes: the trait and row from Task 2.
- Produces:

```rust
/// SQLite-backed inbox. May share a database file with SqliteWorkerStore; owns only its table.
pub struct SqliteHitlStore { /* connection guarded like SqliteWorkerStore */ }
impl SqliteHitlStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HitlError>;
    #[must_use] pub fn path(&self) -> &Path;
}
impl HitlInboxStore for SqliteHitlStore { /* all five */ }
```

Table (created on `open`, mirroring the worker's DDL conventions in `src/sqlite.rs` — copy its pragma/busy-timeout setup verbatim):

```sql
CREATE TABLE IF NOT EXISTS finstack_workflow_hitl_inbox (
    tenant_scope       TEXT    NOT NULL,
    session_id         TEXT    NOT NULL,
    lane_id            TEXT    NOT NULL,
    run_id             TEXT    NOT NULL,
    interaction_id     TEXT    NOT NULL,
    kind               TEXT    NOT NULL,
    requested_at_unix_ms  INTEGER NOT NULL,
    expires_at_unix_ms    INTEGER,
    request            BLOB    NOT NULL,
    status             TEXT    NOT NULL,
    resolved_by        TEXT,
    updated_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (tenant_scope, interaction_id)
);
```

Never touch `PRAGMA user_version` (kernel-owned in shared files). Map store failures to `StoreUnavailable`/`StoreIntegrity` with codes following the worker's naming, backend-prefixed (`"sqlite_hitl_open"`, `"sqlite_hitl_upsert"`, `"sqlite_hitl_row"`, …; the prefix was added during the final review wave).

- [ ] **Step 1: Write the failing test** — `tests/hitl/sqlite.rs`: run the exact Task 2 assertions against `SqliteHitlStore::open(tempdir.path().join("hitl.sqlite"))` (extract the assertion body into a shared `fn exercise_store(store: &dyn HitlInboxStore)` in `tests/hitl/store.rs` and call it from both); plus a persistence check — drop the store, reopen the same path, rows and statuses survive; plus co-location — open a `SqliteWorkerStore` on the same file first, then `SqliteHitlStore::open` on it, both operate without error.
- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-hitl sqlite` → compile failure.
- [ ] **Step 3: Implement** — sqlite.rs per above; add the SQLite dependency line copied from the worker's Cargo.toml; re-export `SqliteHitlStore`.
- [ ] **Step 4: Run to verify pass** — `cargo test -p finstack-ai-workflow-hitl` and clippy, exit 0.
- [ ] **Step 5: Commit** — `git commit -m "feat(workflow): sqlite hitl inbox store"`

---

### Task 4: Capture — `capture` and the composing `park`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/capture.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-hitl/src/lib.rs`
- Test: `extensions/workflow/finstack-ai-workflow-hitl/tests/hitl/capture.rs`

**Interfaces:**
- Consumes: `classify_wait`, `WorkflowWait`, `WorkflowSession`, `WorkflowCheckpoint` (runtime, `native-tokio`); `finstack_ai_workflow_worker::{park as worker_park, WakeIndexStore}`.
- Produces:

```rust
/// Record an interaction wait into the HITL inbox. Returns true when a row was written
/// (wait was Interaction), false otherwise. Row fields: coordinates from the checkpoint,
/// interaction_id/kind/expires_at from the request, request = serde_json bytes of the
/// envelope, status = Open, requested_at = updated_at = now.
pub fn capture(
    store: &dyn HitlInboxStore,
    checkpoint: &WorkflowCheckpoint,
    wait: &WorkflowWait,
    now: Timestamp,
) -> Result<bool, HitlError>;

/// Worker park + HITL capture in one call. Classifies the wait first, delegates to
/// finstack_ai_workflow_worker::park, then captures Interaction waits.
pub fn park(
    session: &mut WorkflowSession,
    wake: &dyn WakeIndexStore,
    inbox: &dyn HitlInboxStore,
    workflow_kind: &str,
    now: Timestamp,
) -> Result<WorkflowCheckpoint, HitlError>;
```

Serialization note: `InteractionRequest` is a journaled kernel envelope and serde-serializable; `capture` uses `serde_json::to_vec`, mapping failure to `StoreIntegrity { code: "hitl_request_encode" }`. `kind` and `expires_at` are read from the request's accessors (the same ones `request_approval_interaction` populates); `interaction_id` via `request.interaction_id().to_canonical_string()`, identical to the worker's wake `pending_id` so the reconcile join in Task 6 is exact. If any accessor named here does not exist verbatim, record it under "Spec deviations" and use the actual accessor — do not add kernel API.

`park` computes `classify_wait(session.last_state())` **before** calling `worker_park` (which consumes the state and aborts the owner), then calls `worker_park(session, wake, workflow_kind)?`, then `capture(...)` when the wait was `Interaction`. Re-parking an already-captured interaction upserts the same row (idempotent).

- [ ] **Step 1: Write the failing test** — `tests/hitl/capture.rs`, unit-level via `capture` (no live session needed): build a `WorkflowCheckpoint` fixture and a `WorkflowWait::Interaction` whose `InteractionRequest` is constructed exactly as `extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/restart.rs:143-318` builds its interaction fixture (copy that construction). Assert: returns `Ok(true)`; the stored row's `interaction_id` equals `request.interaction_id().to_canonical_string()`; `kind == "approval"`; `expires_at` round-trips; `serde_json::from_slice::<InteractionRequest>(&row.request)` equals the original. Second case: `WorkflowWait::Timer { .. }` returns `Ok(false)` and writes nothing. Third: capturing the same interaction twice leaves one row.
- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-hitl capture` → compile failure.
- [ ] **Step 3: Implement** — capture.rs per above; re-export `capture, park`.
- [ ] **Step 4: Run to verify pass** — `cargo test -p finstack-ai-workflow-hitl` and clippy, exit 0.
- [ ] **Step 5: Write the failing integration test for `park`** — extend `tests/hitl/capture.rs` with the live-session shape: SQLite journal + scripted approval-parking model (fixture copied from `restart.rs:143-318`), drive to `AwaitingInteraction`, attach a `WorkflowSession`, call this crate's `park(session, &wake, &inbox, "hitl-demo", now)`. Assert the wake store has one `WakeReason::Interaction` row and the inbox has one `Open` row with matching `pending_id`/`interaction_id`.
- [ ] **Step 6: Run red, make green** — `cargo test -p finstack-ai-workflow-hitl capture`, iterate to exit 0.
- [ ] **Step 7: Commit** — `git commit -m "feat(workflow): capture interaction parks into hitl inbox"`

---

### Task 5: `HitlRouter` — `pending` and authorized `resolve`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/router.rs`
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/authorize.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-hitl/src/lib.rs`
- Test: `extensions/workflow/finstack-ai-workflow-hitl/tests/hitl/resolve.rs`

**Interfaces:**
- Consumes: `WorkflowWorker::deliver_interaction(&InteractionResolutionCommand, received_at) -> Result<(), WorkerError>` (worker plan Task 10); kernel `InteractionResolution`, `InteractionResolutionCommand`, `OperationLocator`, `PrincipalRef`, `AuthorizationEvidence`, `RawJson` (construction pattern verbatim from `examples/durable-interaction/src/main.rs:469-484`).
- Produces:

```rust
/// Decides whether a principal may resolve a pending interaction. Fail closed.
pub trait ResolveAuthorizer: Send + Sync {
    fn authorize(&self, row: &InteractionRow, request: &InteractionRequest,
        principal: &PrincipalRef) -> Result<(), HitlError>; // deny => Unauthorized { code }
}

/// Default: the principal's tenant must equal the interaction's tenant_scope.
pub struct TenantAuthorizer;
impl ResolveAuthorizer for TenantAuthorizer { /* Unauthorized { code: "tenant_mismatch" } */ }

pub struct HitlRouter { /* store, worker: Arc<WorkflowWorker>, wake: Arc<dyn WakeIndexStore>,
    authorizer: Arc<dyn ResolveAuthorizer>, expiry: Arc<dyn ExpiryPolicy> — expiry lands Task 6 */ }
impl HitlRouter {
    #[must_use] pub fn new(store: Arc<dyn HitlInboxStore>, worker: Arc<WorkflowWorker>,
        wake: Arc<dyn WakeIndexStore>) -> Self; // TenantAuthorizer + ApprovalExpiry defaults
    #[must_use] pub fn with_authorizer(self, authorizer: Arc<dyn ResolveAuthorizer>) -> Self;
    /// Open interactions for a tenant (host inbox view).
    pub fn pending(&self, tenant_scope: &str) -> Result<Vec<InteractionRow>, HitlError>;
    /// Authorize, build the resolution command, deliver durably, mark Delivered.
    pub fn resolve(&self, tenant_scope: &str, interaction_id: &str, resolution_id: &str,
        principal: PrincipalRef, evidence: AuthorizationEvidence, payload: RawJson,
        note: Option<&str>, now: Timestamp) -> Result<(), HitlError>;
}
```

`resolve` sequence: `store.load` → `UnknownInteraction` / `NotOpen` unless status is `Open`; deserialize the envelope (`StoreIntegrity { code: "hitl_request_decode" }`); `authorizer.authorize(&row, &request, &principal)?`; rebuild the locator from the row's four coordinates via `OperationLocator::try_new` (failure → `StoreIntegrity { code: "hitl_locator" }`); construct `InteractionResolution::try_new(request.interaction_id(), resolution_id, principal.clone(), evidence, payload, note)` then `InteractionResolutionCommand::try_new(locator, resolution)` (kernel construction failures → `InvalidResolution { code: "hitl_resolution" }`); `worker.deliver_interaction(&command, now)?`; `store.set_status(tenant, id, Delivered, Some(principal-subject), now)`. Delivery failure leaves the row `Open` (retryable).

- [ ] **Step 1: Write the failing test** — `tests/hitl/resolve.rs`. Build a real `WorkflowWorker` via `WorkerBuilder::new(journal, cron, wake, fires, inbox)` on memory stores (no tick needed — delivery only inserts). Seed one `Open` row via `capture` from Task 4's fixture. Assert: (a) happy path — `resolve` with a tenant-matching principal returns `Ok`, the worker's `InboxStore::load_all()` shows one `InboxKind::Interaction` row whose `pending_id` matches, and the HITL row is `Delivered` with `resolved_by` set; (b) `pending("tenant-a")` listed the row before and not after; (c) wrong-tenant principal → `Unauthorized`/`tenant_mismatch`, no worker inbox row, row stays `Open`; (d) unknown id → `unknown_interaction`; (e) second resolve after Delivered → `not_open`.
- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-hitl resolve` → compile failure.
- [ ] **Step 3: Implement** — router.rs + authorize.rs per above; re-export `HitlRouter, ResolveAuthorizer, TenantAuthorizer`.
- [ ] **Step 4: Run to verify pass** — `cargo test -p finstack-ai-workflow-hitl` and clippy, exit 0.
- [ ] **Step 5: Commit** — `git commit -m "feat(workflow): hitl router with authorized resolve"`

---

### Task 6: Expiry + reconcile — `sweep`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/src/expiry.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-hitl/src/router.rs`, `src/lib.rs`
- Test: `extensions/workflow/finstack-ai-workflow-hitl/tests/hitl/sweep.rs`

**Interfaces:**
- Consumes: `WakeIndexStore::load_tenant`, `WakeReason::Interaction`, `WakeRow.pending_id`.
- Produces:

```rust
/// What an expired interaction resolves to. None = no expiry action; the row stays Open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiryResolution {
    pub principal: PrincipalRef,
    pub evidence: AuthorizationEvidence,
    pub payload: RawJson,
}

/// Host policy for interactions that cross expires_at.
pub trait ExpiryPolicy: Send + Sync {
    fn expire(&self, row: &InteractionRow, request: &InteractionRequest)
        -> Result<Option<ExpiryResolution>, HitlError>;
}

/// Default: refuse expired approvals; leave every other kind untouched.
/// Principal ("finstack.workflow.hitl", "expiry", Some(tenant)),
/// evidence ("hitl-expiry-v1", "refuse-on-expiry"), payload {"approved": false}.
pub struct ApprovalExpiry;
impl ExpiryPolicy for ApprovalExpiry { /* per doc comment */ }

pub struct SweepReport { pub expired: usize, pub reconciled: usize }

impl HitlRouter {
    #[must_use] pub fn with_expiry_policy(self, policy: Arc<dyn ExpiryPolicy>) -> Self;
    /// Reconcile then expire. Reconcile: any active (Open|Delivered) row with no
    /// matching Interaction wake row (tenant load, reason == Interaction,
    /// pending_id == interaction_id) is set Closed. Expire: remaining Open rows with
    /// expires_at <= now get the policy's resolution delivered via the worker inbox
    /// (resolution_id = "hitl-expiry-<interaction_id>") and are set Expired.
    pub fn sweep(&self, now: Timestamp) -> Result<SweepReport, HitlError>;
}
```

*Historical: the `ApprovalExpiry` default sketched in this block — refusing expired approvals under a synthetic `("finstack.workflow.hitl", "expiry", …)` principal — was superseded during implementation by the amendment recorded in [design spec §2.4](../specs/2026-08-20-workflow-hitl-router-design.md): the shipped default is fail-closed (it declines every row, because no battery-authored credential satisfies the runtime's interaction ingress), credential-free expiry is driven by the worker tick's kernel `ExpireIfDue`, and `ExpiryPolicy` is a row-disposition hook rather than a way to author the run's outcome. The task below is left as written.*

Ordering is load-bearing: reconcile **before** expiry so an interaction resolved out-of-band (wake row consumed by a tick) is closed rather than refused. A policy returning `None` leaves the row `Open` — it will be reconsidered next sweep, so `sweep` stays idempotent. Policy or delivery failure for one row does not abort the sweep; the row stays `Open` and the error propagates only if every row failed — no: keep it simpler and strict, first error aborts and returns `Err`; rows already transitioned stay transitioned (each transition is independently durable, re-sweep is safe).

- [ ] **Step 1: Write the failing test** — `tests/hitl/sweep.rs`, memory stores + real worker as in Task 5: (a) approval row with `expires_at = t1`, wake row present, `sweep(t0 < t1)` → report `{0, 0}`, row `Open`; (b) `sweep(t1)` → `{expired: 1, ...}`, worker inbox has one interaction row whose deserialized command's payload is `{"approved": false}` and principal subject is `"expiry"`, HITL row `Expired`; (c) non-approval kind past expiry with default policy → untouched, still `Open`; (d) reconcile — `Delivered` row whose wake row was deleted → `Closed`, counted in `reconciled`; (e) reconcile-beats-expiry — expired row with missing wake row ends `Closed`, not `Expired`, and no delivery happens; (f) sweep twice → second report `{0, 0}`.
- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-hitl sweep` → compile failure.
- [ ] **Step 3: Implement** — expiry.rs + `sweep` per above; re-export `ApprovalExpiry, ExpiryPolicy, ExpiryResolution, SweepReport`.
- [ ] **Step 4: Run to verify pass** — `cargo test -p finstack-ai-workflow-hitl` and clippy, exit 0.
- [ ] **Step 5: Commit** — `git commit -m "feat(workflow): hitl expiry sweep and wake reconcile"`

---

### Task 7: UC-05 end-to-end integration tests

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/tests/hitl/uc05.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-hitl/tests/hitl/main.rs`

**Interfaces:**
- Consumes: everything shipped in Tasks 1–6 plus `WorkflowWorker::tick()`; the completion-driving scripted-model fixture from `extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/restart.rs:143-318` (which drives past resolution), on SQLite stores.

Both tests run the full UC-05 sentence with a **process-death boundary**: build stores on a tempdir, park, then drop every in-memory handle and rebuild stores/worker/router from the paths alone.

- [ ] **Step 1: Write the failing test `uc05_resolve_end_to_end`** — SQLite journal + `SqliteWorkerStore` + `SqliteHitlStore` (same file); scripted approval model with a second scripted response that finalizes after the tool result (copy `restart.rs` fixture); drive to `AwaitingInteraction`; `park` (this crate's); **drop everything, reopen from paths**; rebuild worker (`WorkerBuilder` with a `PortsFactory` binding the scripted model/catalog, per the worker's own tick tests) and `HitlRouter`; assert `pending("tenant-a")` lists exactly one approval with the expected prompt metadata; `resolve` with an authorized principal and payload `{"approved":true}`; `worker.tick()` → assert `TickReport.sessions_resumed == 1`; attach a fresh `WorkflowSession` and assert `classify_wait` yields `Terminal` with the run completed and the echo tool executed exactly once (journal state, as `restart.rs` asserts it); assert the HITL row reconciles to `Closed` on a final `sweep`.
- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-hitl uc05` → red (test drives behavior not yet composed).
- [ ] **Step 3: Make it pass** — fix composition seams only; no new public API without noting it in Self-review.
- [ ] **Step 4: Write the failing test `uc05_expiry_end_to_end`** — same setup, never resolve; advance the router-visible `now` past `expires_at`; `sweep(now)` → `expired == 1`; `worker.tick()` resumes the session; assert the run proceeds with the refusal outcome (model observes the refused approval — assert via journal terminal state per the `Refused` path semantics: synthetic `TOOL_APPROVAL_REQUIRED` tool outcome, tool never executed) and the row is `Expired`.
- [ ] **Step 5: Run red, make green** — `cargo test -p finstack-ai-workflow-hitl uc05`, iterate to exit 0.
- [ ] **Step 6: Full battery gate** — `cargo test -p finstack-ai-workflow-hitl` and `cargo clippy -p finstack-ai-workflow-hitl --all-targets -- -D warnings`, exit 0.
- [ ] **Step 7: Commit** — `git commit -m "test(workflow): uc-05 hitl resolve and expiry end to end"`

---

### Task 8: Upgrade `examples/durable-interaction` to the full UC-05 story

**Files:**
- Modify: `examples/durable-interaction/src/main.rs`
- Modify: `examples/durable-interaction/Cargo.toml` (add `finstack-ai-workflow-worker`, `finstack-ai-workflow-hitl`)
- Modify: `examples/README.md` (bullet text)

**Interfaces:**
- Consumes: Tasks 1–7. No new API.

The example currently ends at `resolve_interaction` without proving resumption (`main.rs:469-487`). Rework its tail: park via the router's `park`, simulate death (existing `drop` choreography at `:458-459`), rebuild from paths, list via `pending()`, resolve via `HitlRouter::resolve`, `tick()`, and print the terminal phase — the printed line becomes `durable interaction {interaction_id} resolved and run completed after restart`. Keep the example single-file and `publish = false`.

- [ ] **Step 1: Rework the example** as above; extend the `ScriptedModel` with the finalizing second response (same shape as Task 7's fixture).
- [ ] **Step 2: Verify** — `cargo run -p finstack-ai-example-durable-interaction` exits 0 and prints the completion line; `cargo clippy -p finstack-ai-example-durable-interaction --all-targets -- -D warnings` exits 0.
- [ ] **Step 3: Commit** — `git commit -m "docs(examples): durable-interaction drives uc-05 to completion via hitl router"`

---

### Task 9: Battery packaging — README, docs row, baselines

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-hitl/README.md`
- Modify: the docs/site page that carries the workflow shipping-leaves table (locate with `rg -l "finstack-ai-workflow-local" docs/site/`; if no page lists it, create `docs/site/workflow.md` mirroring `docs/site/middleware.md`'s table shape)
- Modify: CHANGELOG (the file the verify-promotion plan's Task 6 updates — same one)
- Create (generated): `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-workflow-hitl.txt`

**Interfaces:** none — packaging only.

- [ ] **Step 1: README** — sections matching an existing battery README (e.g. `extensions/workflow/finstack-ai-workflow-local/README.md`): what it is (the spec's §2 summary), the park→pending→resolve/sweep→tick lifecycle, the two store impls, the authorization and expiry hooks, and the explicit non-goals (no assignment UI, reminders, or SLOs).
- [ ] **Step 2: Docs row** — add: `| [\`finstack-ai-workflow-hitl\`](../../extensions/workflow/finstack-ai-workflow-hitl/README.md) | Battery. Durable interaction inbox over the workflow worker: capture on park, authorized resolve, expiry sweep. |` (adjust columns to the table's actual header).
- [ ] **Step 3: CHANGELOG entry** — one line under the unreleased section: `finstack-ai-workflow-hitl: new HITL router battery (UC-05) — interaction inbox, authorized resolve, expiry timers.`
- [ ] **Step 4: Public-api baseline** — `uv run --no-project python scripts/compat/public_items.py` then `mise run check-public-api`, exit 0; review the new `.txt` diff contains exactly this crate's intended surface.
- [ ] **Step 5: Workspace gate** — `mise run check-rust` and `mise run test-rust`, both exit 0.
- [ ] **Step 6: Commit** — `git commit -m "docs: workflow hitl battery readme, site row, public-api baseline"`

---

## Deliberately out of scope

- Assignment/assignee routing, reminders, notification transports, SLO/aging metrics (the brief's explicit exclusions).
- Completion-ingress interaction tokens — future token kind per `docs/superpowers/specs/2026-08-20-completion-ingress-design.md` §8; the router is the component that design defers to, and its `resolve` is the seam a token terminator will call.
- Python/JS bindings and `finstack-ai` facade wiring — follow-up once the Rust surface settles.
- Any change to kernel, runtime, or worker public API. If a consumed worker signature drifted at merge, adapt here and record it; do not push changes upstream from this plan.

## Self-review notes

- **Spec coverage:** §2.1 capture → Task 4; §2.2 durable resolve → Task 5; §2.3 authorization → Task 5; §2.4 expiry → Task 6; §2.5 reconcile → Task 6; §4 data/stores → Tasks 2–3; §6 acceptance → Tasks 7–8; battery packaging → Task 9.
- **Known risks, verified as far as `main` allows:**
  - `WorkflowWorker::deliver_interaction` and `tick` are worker-plan Tasks 9–10, reviewed from the plan text and the worktree, not from `main`. First-day job of Task 1's implementer: confirm the merged signatures and open a "Spec deviations" section if they moved.
  - `InteractionRequest` serde round-trip and its `kind`/`expires_at` accessors are asserted by Task 4 Step 1 before anything depends on them — that test failing is the designed tripwire, and the fallback (store projected fields instead of the envelope) is a contained change to `capture` + `InteractionRow.request`.
  - The wake index's `pending_id` column is type-erased across `InteractionId`/`EffectId` canonical strings; the reconcile join additionally filters `reason == WakeReason::Interaction`, so an effect-id collision cannot close an interaction row.
- **Type consistency:** `HitlInboxStore`'s five methods are used with the same names in Tasks 4–7; `InteractionStatus` tokens match the SQLite `status` column; `resolve`'s parameter order matches the example's `InteractionResolution::try_new` order.
