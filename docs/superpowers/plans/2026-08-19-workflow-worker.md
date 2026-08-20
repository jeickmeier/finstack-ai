# finstack-ai-workflow-worker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A leased worker (library + optional binary) that polls an adapter-owned wake index and cron table in SQLite, fires due schedules into idempotent run starts, and resumes sessions parked on Timer / Interaction / DeferredEffect waits — surviving process death.

**Architecture:** New crate `extensions/workflow/finstack-ai-workflow-worker`. Three adapter-owned SQLite tables (wake index, fires, inbox) behind fail-closed traits, one `SqliteWorkerStore` implementing all three, one `MemoryWorkerStore` for tests. A `WorkflowWorker::tick()` claims work with the same `BEGIN IMMEDIATE` CAS pattern as `SqliteCronStore::try_claim`, attaches via `WorkflowSession::trusted`, binds ports through host-registered factories, and drives to the next wait. The journal is authoritative; every table is a hint. No kernel changes.

**Tech Stack:** Rust (workspace edition/lints), `rusqlite`, `thiserror`, `serde_json`, `tokio` (workspace-pinned `=1.51.1`), dev-deps `finstack-ai-store-memory`, `finstack-ai-test`, `tempfile`.

**Spec:** `docs/superpowers/specs/2026-08-19-workflow-worker-design.md`

## Global Constraints

- No changes to `finstack-ai-kernel` or the `JournalStore` port. The only runtime change is Task 1 (configurable drive timeout, default unchanged at 2 s).
- All new store trait methods default to fail-closed errors (`..._unsupported` codes), mirroring `CronScheduleStore::try_claim` (`extensions/workflow/finstack-ai-workflow-local/src/store.rs:36-54`).
- Adapter tables never touch `PRAGMA user_version` and are not kernel records (same doctrine as `finstack_workflow_local_cron`).
- Crate name `finstack-ai-workflow-worker`; workspace inheritance for `version/edition/rust-version/license/repository/homepage/authors`; `[lints] workspace = true` (workspace lints require docs on every public item — write real doc comments, follow the terse style of `cron.rs`).
- Error enums follow the `CronError` pattern: `thiserror`, stable `&'static str` codes, a `code()` method.
- Ids cross the SQL boundary as canonical strings via `Id::to_canonical_string()` / `Id::parse()` (`crates/finstack-ai-kernel/src/primitives/ids.rs:93,117`); timestamps as `i64` unix ms via `Timestamp::as_unix_ms()` / `Timestamp::from_unix_ms()`.
- Commit after every green test cycle. Run checks with workspace binaries and check exit codes explicitly: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets`, never a summarizing proxy.

---

### Task 1: Configurable `drive_until_wait` timeout (runtime)

**Files:**
- Modify: `crates/finstack-ai-runtime/src/driver/workflow/mod.rs:366-538`
- Test: `extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/driver.rs` (append)

**Interfaces:**
- Produces: `WorkflowSession::with_drive_timeout(self, timeout: Duration) -> Self` and `WorkflowSession::drive_timeout(&self) -> Duration` (default `Duration::from_secs(2)`). Task 9's worker calls `with_drive_timeout`.

- [ ] **Step 1: Write the failing test**

Append to `extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/driver.rs` (helpers `attach_driver`, `memory_store`, `timestamp`, `completed_plan`, `profile` are already in scope via `helpers/mod.rs`; `ScriptedModel` via existing imports — mirror the imports at the top of that file):

```rust
#[tokio::test]
async fn drive_timeout_is_configurable_and_defaults_to_two_seconds() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("done")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let driver = attach_driver(store, model, clock, 900).await;
    assert_eq!(driver.drive_timeout(), StdDuration::from_secs(2));
    let session = driver
        .into_session()
        .with_drive_timeout(StdDuration::from_millis(250));
    assert_eq!(session.drive_timeout(), StdDuration::from_millis(250));
}
```

This also needs a consuming accessor on the local driver. Add to `extensions/workflow/finstack-ai-workflow-local/src/driver.rs` (after `session_mut`):

```rust
    /// Unwrap the inner session, dropping adapter cron state.
    #[must_use]
    pub fn into_session(self) -> WorkflowSession {
        self.session
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p finstack-ai-workflow-local drive_timeout_is_configurable -- --nocapture`
Expected: FAIL to compile — `drive_timeout`/`with_drive_timeout` not found.

- [ ] **Step 3: Implement in the runtime**

In `crates/finstack-ai-runtime/src/driver/workflow/mod.rs`:

1. Add a field to `WorkflowSession` (after `last_state: KernelState,`):

```rust
    drive_timeout: Duration,
```

2. Initialize it in `attach` (in the `Ok(Self { ... })` literal, after `last_state: coordinator.state().clone(),`):

```rust
            drive_timeout: Duration::from_secs(2),
```

3. Add builder + getter (after `with_context_providers`):

```rust
    /// Override the [`Self::drive_until_wait`] poll bound. Default is 2 s.
    #[must_use]
    pub const fn with_drive_timeout(mut self, timeout: Duration) -> Self {
        self.drive_timeout = timeout;
        self
    }

    /// Current [`Self::drive_until_wait`] poll bound.
    #[must_use]
    pub const fn drive_timeout(&self) -> Duration {
        self.drive_timeout
    }
```

4. In `drive_until_wait`, replace `tokio::time::timeout(Duration::from_secs(2), async {` with:

```rust
        tokio::time::timeout(self.drive_timeout, async {
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-workflow-local && cargo test -p finstack-ai-runtime`
Expected: PASS (existing suites confirm the default is unchanged).

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai-runtime/src/driver/workflow/mod.rs extensions/workflow/finstack-ai-workflow-local
git commit -m "Make the workflow drive_until_wait poll bound configurable"
```

---

### Task 2: Crate scaffold, `WorkerError`, workspace registration, stale-comment fix

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/Cargo.toml`
- Create: `extensions/workflow/finstack-ai-workflow-worker/README.md`
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/lib.rs`
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/error.rs`
- Modify: `Cargo.toml` (workspace root, members list around line 38 and deps table around line 142)
- Modify: `extensions/workflow/finstack-ai-workflow-local/src/lib.rs:5-6`

**Interfaces:**
- Produces: `WorkerError` with variants `StoreUnavailable { code }`, `StoreIntegrity { code }`, `TimeOverflow`, `UnknownKind { kind: Arc<str> }`, `NotParked`, `Driver(WorkflowDriverError)`, `Cron(CronError)`; method `code(&self) -> &'static str`. Every later task returns it.

- [ ] **Step 1: Register the crate in the workspace**

In root `Cargo.toml`, add to `[workspace] members` (alphabetical, next to the workflow-local entry):

```toml
    "extensions/workflow/finstack-ai-workflow-worker",
```

and to `[workspace.dependencies]` (next to `finstack-ai-workflow-local`):

```toml
finstack-ai-workflow-worker = { path = "extensions/workflow/finstack-ai-workflow-worker", version = "1.0.0" }
```

- [ ] **Step 2: Create the crate**

`extensions/workflow/finstack-ai-workflow-worker/Cargo.toml`:

```toml
[package]
name = "finstack-ai-workflow-worker"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Leased worker daemon for the finstack-ai local workflow driver"
readme = "README.md"

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
finstack-ai-workflow-local = { workspace = true }
rusqlite = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros", "sync", "time"] }

[dev-dependencies]
finstack-ai-store-memory = { workspace = true }
finstack-ai-store-sqlite = { workspace = true }
finstack-ai-test = { workspace = true }
tempfile = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros", "sync", "time"] }

[lints]
workspace = true
```

`src/lib.rs`:

```rust
//! Leased worker over the local workflow driver.
//!
//! Owns three adapter tables — a wake index, cron-fire records, and a
//! response inbox — and a tick loop that claims due work with CAS leases.
//! Every table is a hint; the kernel journal stays authoritative.

mod error;

pub use error::WorkerError;
```

`src/error.rs`:

```rust
use std::sync::Arc;

use finstack_ai_runtime::WorkflowDriverError;
use finstack_ai_workflow_local::CronError;
use thiserror::Error;

/// Fail-closed worker failures. These are not kernel record errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkerError {
    /// Adapter table is temporarily unavailable.
    #[error("worker store unavailable: {code}")]
    StoreUnavailable {
        /// Stable reason code.
        code: &'static str,
    },
    /// Adapter table could not be decoded safely.
    #[error("worker store integrity: {code}")]
    StoreIntegrity {
        /// Stable reason code.
        code: &'static str,
    },
    /// Clock or lease arithmetic left the representable range.
    #[error("worker time overflow")]
    TimeOverflow,
    /// No ports factory is registered for the parked workflow kind.
    #[error("unknown workflow kind: {kind}")]
    UnknownKind {
        /// The kind recorded at park time.
        kind: Arc<str>,
    },
    /// The session has no classified wait to park on.
    #[error("session is not parked on a wait")]
    NotParked,
    /// Underlying workflow driver failure.
    #[error(transparent)]
    Driver(#[from] WorkflowDriverError),
    /// Underlying adapter cron failure.
    #[error(transparent)]
    Cron(#[from] CronError),
}

impl WorkerError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::StoreUnavailable { code } | Self::StoreIntegrity { code } => code,
            Self::TimeOverflow => "time_overflow",
            Self::UnknownKind { .. } => "unknown_workflow_kind",
            Self::NotParked => "not_parked",
            Self::Driver(error) => error.code(),
            Self::Cron(error) => error.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            WorkerError::StoreUnavailable { code: "x" }.code(),
            "x"
        );
        assert_eq!(WorkerError::TimeOverflow.code(), "time_overflow");
        assert_eq!(WorkerError::NotParked.code(), "not_parked");
    }
}
```

`README.md`:

```markdown
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
```

- [ ] **Step 3: Fix the stale sibling comment**

In `extensions/workflow/finstack-ai-workflow-local/src/lib.rs`, replace the sentence on lines 5-6 that references the Temporal-shaped sibling with:

```rust
//! This crate stays dependency-light: adapter cron state lives beside the
//! journal file and never becomes a kernel record.
```

(Adjust to splice cleanly with the surrounding doc text — keep the existing first paragraph intact.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p finstack-ai-workflow-worker && cargo clippy -p finstack-ai-workflow-worker --all-targets`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml extensions/workflow
git commit -m "Scaffold the finstack-ai-workflow-worker crate"
```

---

### Task 3: Wake index — `WakeRow`, `WakeReason`, `WakeIndexStore`, `MemoryWorkerStore`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/wake.rs`
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/memory.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-worker/src/lib.rs`

**Interfaces:**
- Produces:
  - `WakeReason { Timer, Interaction, Deferred }` with `as_str(self) -> &'static str` (`"timer"`, `"interaction"`, `"deferred"`) and `parse(&str) -> Result<Self, WorkerError>` (`StoreIntegrity { code: "wake_reason" }` on unknown).
  - `WakeRow { tenant_scope: Arc<str>, session_id: SessionId, lane_id: LaneId, run_id: RunId, workflow_kind: Arc<str>, reason: WakeReason, wake_at: Option<Timestamp>, pending_id: Arc<str>, leased_by: Option<Arc<str>>, lease_expires_at: Option<Timestamp>, attempts: u32 }` (`Debug, Clone, PartialEq, Eq`).
  - `trait WakeIndexStore: Send + Sync` with:
    - `fn upsert(&self, row: &WakeRow) -> Result<(), WorkerError>;`
    - `fn delete(&self, tenant_scope: &str, session_id: SessionId) -> Result<(), WorkerError>;`
    - `fn load_due(&self, now: Timestamp) -> Result<Vec<WakeRow>, WorkerError>;`
    - `fn load_tenant(&self, tenant_scope: &str) -> Result<Vec<WakeRow>, WorkerError>;`
    - `fn try_claim(&self, tenant_scope: &str, session_id: SessionId, worker_id: &str, now: Timestamp, lease_ttl_ms: u64) -> Result<bool, WorkerError>` — default body fails closed with `StoreUnavailable { code: "wake_claim_unsupported" }`.
    - `fn renew(&self, tenant_scope: &str, session_id: SessionId, worker_id: &str, now: Timestamp, lease_ttl_ms: u64) -> Result<bool, WorkerError>` — default fails closed with `code: "wake_renew_unsupported"`.
    - `fn record_failure(&self, tenant_scope: &str, session_id: SessionId, retry_at: Timestamp) -> Result<(), WorkerError>;` — clears the lease, increments `attempts`, sets `wake_at = Some(retry_at)`.
  - `MemoryWorkerStore::new()` implementing the trait (it will also implement Tasks 5–6's traits).
- Consumes: `WorkerError` (Task 2).

**Semantics locked here:** `load_due(now)` returns rows whose lease is absent or expired (`lease_expires_at <= now`) AND (`reason == Timer` implies `wake_at <= now`; non-timer rows are always "due" — the tick loop gates them on the inbox). `try_claim` wins only when the lease is absent or expired.

- [ ] **Step 1: Write the failing tests**

In `src/memory.rs` `#[cfg(test)] mod tests`:

```rust
use std::sync::Arc;

use finstack_ai_kernel::Timestamp;

use crate::wake::{WakeIndexStore, WakeReason, WakeRow};
use super::MemoryWorkerStore;

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    finstack_ai_kernel::Id::from_bytes(bytes)
}

fn timer_row(tenant: &str, session: u64, due_ms: i64) -> WakeRow {
    WakeRow {
        tenant_scope: Arc::from(tenant),
        session_id: id(session),
        lane_id: id(2),
        run_id: id(3),
        workflow_kind: Arc::from("research"),
        reason: WakeReason::Timer,
        wake_at: Some(ts(due_ms)),
        pending_id: Arc::from("effect-1"),
        leased_by: None,
        lease_expires_at: None,
        attempts: 0,
    }
}

#[test]
fn timer_rows_are_due_only_at_or_after_wake_at() {
    let store = MemoryWorkerStore::new();
    store.upsert(&timer_row("tenant-a", 1, 2_000)).expect("upsert");
    assert!(store.load_due(ts(1_999)).expect("early").is_empty());
    assert_eq!(store.load_due(ts(2_000)).expect("due").len(), 1);
}

#[test]
fn non_timer_rows_are_always_due() {
    let store = MemoryWorkerStore::new();
    let mut row = timer_row("tenant-a", 1, 9_000);
    row.reason = WakeReason::Interaction;
    row.wake_at = None;
    store.upsert(&row).expect("upsert");
    assert_eq!(store.load_due(ts(0)).expect("due").len(), 1);
}

#[test]
fn claim_excludes_row_until_lease_expires() {
    let store = MemoryWorkerStore::new();
    store.upsert(&timer_row("tenant-a", 1, 1_000)).expect("upsert");
    assert!(store
        .try_claim("tenant-a", id(1), "worker-a", ts(1_500), 1_000)
        .expect("first claim"));
    assert!(!store
        .try_claim("tenant-a", id(1), "worker-b", ts(1_600), 1_000)
        .expect("held"));
    assert!(store.load_due(ts(1_600)).expect("hidden").is_empty());
    assert!(store
        .try_claim("tenant-a", id(1), "worker-b", ts(2_600), 1_000)
        .expect("expired lease is claimable"));
}

#[test]
fn record_failure_backs_off_and_unleases() {
    let store = MemoryWorkerStore::new();
    store.upsert(&timer_row("tenant-a", 1, 1_000)).expect("upsert");
    assert!(store
        .try_claim("tenant-a", id(1), "worker-a", ts(1_000), 1_000)
        .expect("claim"));
    store
        .record_failure("tenant-a", id(1), ts(5_000))
        .expect("failure");
    let rows = store.load_tenant("tenant-a").expect("load");
    assert_eq!(rows[0].attempts, 1);
    assert_eq!(rows[0].leased_by, None);
    assert!(store.load_due(ts(4_999)).expect("backoff").is_empty());
    assert_eq!(store.load_due(ts(5_000)).expect("retry").len(), 1);
}

#[test]
fn tenants_are_isolated() {
    let store = MemoryWorkerStore::new();
    store.upsert(&timer_row("tenant-a", 1, 1_000)).expect("a");
    store.upsert(&timer_row("tenant-b", 2, 1_000)).expect("b");
    assert_eq!(store.load_tenant("tenant-a").expect("a").len(), 1);
    assert_eq!(store.load_tenant("tenant-b").expect("b").len(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-workflow-worker wake -- --nocapture`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `src/wake.rs` and `MemoryWorkerStore`**

`src/wake.rs` — the types and trait exactly as in **Interfaces** above, with doc comments on every public item. The default `try_claim`/`renew` bodies:

```rust
    fn try_claim(
        &self,
        tenant_scope: &str,
        session_id: SessionId,
        worker_id: &str,
        now: Timestamp,
        lease_ttl_ms: u64,
    ) -> Result<bool, WorkerError> {
        let _ = (tenant_scope, session_id, worker_id, now, lease_ttl_ms);
        Err(WorkerError::StoreUnavailable {
            code: "wake_claim_unsupported",
        })
    }
```

Shared due/claimable predicates (free functions in `wake.rs`, used by both stores):

```rust
/// Whether the row's lease is absent or expired at `now`.
#[must_use]
pub fn lease_open(row: &WakeRow, now: Timestamp) -> bool {
    row.lease_expires_at.is_none_or(|expires| expires <= now)
}

/// Whether the row is due at `now` (lease aside).
#[must_use]
pub fn wake_due(row: &WakeRow, now: Timestamp) -> bool {
    match row.reason {
        WakeReason::Timer => row.wake_at.is_some_and(|due| due <= now),
        WakeReason::Interaction | WakeReason::Deferred => true,
    }
}
```

Lease expiry arithmetic (shared):

```rust
/// `now + ttl` as a timestamp, failing closed on overflow.
pub fn lease_deadline(now: Timestamp, lease_ttl_ms: u64) -> Result<Timestamp, WorkerError> {
    let ttl = i64::try_from(lease_ttl_ms).map_err(|_| WorkerError::TimeOverflow)?;
    let ms = now
        .as_unix_ms()
        .checked_add(ttl)
        .ok_or(WorkerError::TimeOverflow)?;
    Timestamp::from_unix_ms(ms).map_err(|_| WorkerError::TimeOverflow)
}
```

`src/memory.rs` — `MemoryWorkerStore` with `Mutex<BTreeMap<(Arc<str>, SessionId), WakeRow>>` (poisoned lock → `StoreUnavailable { code: "memory_worker_lock_poisoned" }`, mirroring `MemoryCronStore`). `try_claim` checks `lease_open` then writes `leased_by`/`lease_expires_at = lease_deadline(now, ttl)`. `renew` succeeds only when `leased_by == Some(worker_id)`. `load_due` filters `lease_open(row, now) && wake_due(row, now)`. `record_failure`: `attempts += 1`, `leased_by = None`, `lease_expires_at = None`, `wake_at = Some(retry_at)`.

Update `lib.rs`:

```rust
mod error;
mod memory;
mod wake;

pub use error::WorkerError;
pub use memory::MemoryWorkerStore;
pub use wake::{WakeIndexStore, WakeReason, WakeRow, lease_deadline, lease_open, wake_due};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-workflow-worker && cargo clippy -p finstack-ai-workflow-worker --all-targets`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Add the wake index trait and memory store to the workflow worker"
```

---

### Task 4: `SqliteWorkerStore` — wake table with CAS lease

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/sqlite.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-worker/src/lib.rs`

**Interfaces:**
- Produces: `SqliteWorkerStore::open(path: impl AsRef<Path>) -> Result<Self, WorkerError>` implementing `WakeIndexStore` (and later `FireStore` + `InboxStore` in Tasks 5–6 — create all three tables in one DDL batch now).
- Consumes: Task 3's trait and helpers.

- [ ] **Step 1: Write the failing tests**

In `src/sqlite.rs` `#[cfg(test)] mod tests` (reuse the `ts`/`id`/`timer_row` helper shapes from Task 3's tests):

```rust
#[test]
fn two_sqlite_stores_claim_exactly_one_lease() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("worker.sqlite");
    let first = SqliteWorkerStore::open(&path).expect("first");
    let second = SqliteWorkerStore::open(&path).expect("second");
    first.upsert(&timer_row("tenant-a", 1, 1_000)).expect("upsert");
    let won_first = first
        .try_claim("tenant-a", id(1), "worker-a", ts(1_500), 60_000)
        .expect("first");
    let won_second = second
        .try_claim("tenant-a", id(1), "worker-b", ts(1_500), 60_000)
        .expect("second");
    assert_eq!(usize::from(won_first) + usize::from(won_second), 1);
}

#[test]
fn sqlite_round_trips_every_column() {
    let dir = tempfile::tempdir().expect("dir");
    let store = SqliteWorkerStore::open(dir.path().join("w.sqlite")).expect("open");
    let mut row = timer_row("tenant-a", 1, 2_000);
    row.attempts = 3;
    store.upsert(&row).expect("upsert");
    let loaded = store.load_tenant("tenant-a").expect("load");
    assert_eq!(loaded, vec![row]);
}

#[test]
fn expired_lease_is_reclaimed_and_renew_requires_holder() {
    let dir = tempfile::tempdir().expect("dir");
    let store = SqliteWorkerStore::open(dir.path().join("w.sqlite")).expect("open");
    store.upsert(&timer_row("tenant-a", 1, 1_000)).expect("upsert");
    assert!(store
        .try_claim("tenant-a", id(1), "worker-a", ts(1_000), 1_000)
        .expect("claim"));
    assert!(store
        .renew("tenant-a", id(1), "worker-a", ts(1_500), 1_000)
        .expect("holder renews"));
    assert!(!store
        .renew("tenant-a", id(1), "worker-b", ts(1_500), 1_000)
        .expect("stranger cannot renew"));
    assert!(store
        .try_claim("tenant-a", id(1), "worker-b", ts(9_000), 1_000)
        .expect("expired lease reclaimed"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-workflow-worker sqlite -- --nocapture`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`src/sqlite.rs` mirrors `SqliteCronStore` structurally (`Mutex<Connection>`, `busy_timeout(1s)`, WAL for file paths, `with_conn`/`with_conn_mut` helpers with `StoreUnavailable` codes `sqlite_worker_open`, `sqlite_worker_busy_timeout`, `sqlite_worker_wal`, `sqlite_worker_schema`, `sqlite_worker_lock_poisoned`). DDL creates all three worker tables now:

```sql
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_wake (
  tenant_scope TEXT NOT NULL,
  session_id TEXT NOT NULL,
  lane_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  workflow_kind TEXT NOT NULL,
  reason TEXT NOT NULL,
  wake_at_unix_ms INTEGER,
  pending_id TEXT NOT NULL,
  leased_by TEXT,
  lease_expires_unix_ms INTEGER,
  attempts INTEGER NOT NULL,
  PRIMARY KEY (tenant_scope, session_id)
);
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_fires (
  tenant_scope TEXT NOT NULL,
  schedule_id TEXT NOT NULL,
  fire_count INTEGER NOT NULL,
  fired_unix_ms INTEGER NOT NULL,
  status TEXT NOT NULL,
  started_session TEXT,
  PRIMARY KEY (tenant_scope, schedule_id, fire_count)
);
CREATE TABLE IF NOT EXISTS finstack_workflow_worker_inbox (
  tenant_scope TEXT NOT NULL,
  session_id TEXT NOT NULL,
  pending_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload BLOB NOT NULL,
  received_unix_ms INTEGER NOT NULL,
  PRIMARY KEY (tenant_scope, session_id, pending_id)
);
```

`WakeIndexStore` impl:
- `upsert`: `INSERT OR REPLACE` of all columns; ids via `to_canonical_string()`.
- `try_claim`: `BEGIN IMMEDIATE` transaction, then

```sql
UPDATE finstack_workflow_worker_wake
SET leased_by = ?1, lease_expires_unix_ms = ?2
WHERE tenant_scope = ?3 AND session_id = ?4
  AND (leased_by IS NULL OR lease_expires_unix_ms <= ?5)
```

with `?2 = lease_deadline(now, lease_ttl_ms)?.as_unix_ms()`, `?5 = now.as_unix_ms()`; winner = `tx.changes() == 1`, then commit (codes `sqlite_wake_begin_immediate`, `sqlite_wake_claim`, `sqlite_wake_commit`).
- `renew`: same shape, `WHERE ... AND leased_by = ?worker_id`.
- `load_due`:

```sql
SELECT tenant_scope, session_id, lane_id, run_id, workflow_kind, reason,
       wake_at_unix_ms, pending_id, leased_by, lease_expires_unix_ms, attempts
FROM finstack_workflow_worker_wake
WHERE (leased_by IS NULL OR lease_expires_unix_ms <= ?1)
  AND (reason != 'timer' OR (wake_at_unix_ms IS NOT NULL AND wake_at_unix_ms <= ?1))
ORDER BY tenant_scope, session_id
```

- `load_tenant`: same SELECT with `WHERE tenant_scope = ?1`.
- `delete`: `DELETE ... WHERE tenant_scope = ?1 AND session_id = ?2`.
- `record_failure`: single UPDATE setting `attempts = attempts + 1, leased_by = NULL, lease_expires_unix_ms = NULL, wake_at_unix_ms = ?retry_at`.
- Row decoding maps parse failures (ids via `Id::parse`, reason via `WakeReason::parse`, timestamps) to `StoreIntegrity` codes (`sqlite_wake_row`, `sqlite_wake_id`, `sqlite_wake_time`).

Export from `lib.rs`: `pub use sqlite::SqliteWorkerStore;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-workflow-worker && cargo clippy -p finstack-ai-workflow-worker --all-targets`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Add the sqlite worker store with CAS wake leases"
```

---

### Task 5: Fire records — `FireRow`, `FireStore`, both impls

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/fires.rs`
- Modify: `src/memory.rs`, `src/sqlite.rs`, `src/lib.rs`

**Interfaces:**
- Produces:
  - `FireStatus { Claimed, Started }` (`as_str` → `"claimed"`/`"started"`, `parse` fails closed with `StoreIntegrity { code: "fire_status" }`).
  - `FireRow { tenant_scope: Arc<str>, schedule_id: Arc<str>, fire_count: u64, fired_at: Timestamp, status: FireStatus, started_session: Option<Arc<str>> }`.
  - `fn idempotency_key(row: &FireRow) -> String` returning `format!("{}:{}:{}", row.tenant_scope, row.schedule_id, row.fire_count)`.
  - `trait FireStore: Send + Sync`:
    - `fn record_claimed(&self, row: &FireRow) -> Result<(), WorkerError>;` — idempotent (`INSERT OR IGNORE`); re-recording an existing key is a no-op.
    - `fn mark_started(&self, tenant_scope: &str, schedule_id: &str, fire_count: u64, started_session: &str) -> Result<(), WorkerError>;`
    - `fn load_unstarted(&self) -> Result<Vec<FireRow>, WorkerError>;` — all rows with `status = claimed`, any tenant.
- Consumes: `WorkerError`, `MemoryWorkerStore`, `SqliteWorkerStore` (table already created in Task 4).

- [ ] **Step 1: Write the failing tests** (in `src/fires.rs`, run against both stores via a generic helper)

```rust
fn exercise_fire_store(store: &dyn FireStore) {
    let row = FireRow {
        tenant_scope: Arc::from("tenant-a"),
        schedule_id: Arc::from("nightly"),
        fire_count: 7,
        fired_at: ts(2_000),
        status: FireStatus::Claimed,
        started_session: None,
    };
    store.record_claimed(&row).expect("claim");
    store.record_claimed(&row).expect("idempotent re-claim");
    assert_eq!(store.load_unstarted().expect("unstarted"), vec![row.clone()]);
    store
        .mark_started("tenant-a", "nightly", 7, "session-9")
        .expect("start");
    assert!(store.load_unstarted().expect("drained").is_empty());
}

#[test]
fn memory_fire_store_round_trips() {
    exercise_fire_store(&crate::MemoryWorkerStore::new());
}

#[test]
fn sqlite_fire_store_round_trips() {
    let dir = tempfile::tempdir().expect("dir");
    let store = crate::SqliteWorkerStore::open(dir.path().join("w.sqlite")).expect("open");
    exercise_fire_store(&store);
}
```

Also assert the key format:

```rust
#[test]
fn idempotency_key_is_tenant_schedule_count() {
    let row = /* as above */;
    assert_eq!(idempotency_key(&row), "tenant-a:nightly:7");
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-worker fires` → compile FAIL.

- [ ] **Step 3: Implement** — `fires.rs` types + trait (no fail-closed defaults needed; all three methods are required). `MemoryWorkerStore`: `Mutex<BTreeMap<(Arc<str>, Arc<str>, u64), FireRow>>`. `SqliteWorkerStore`: `INSERT OR IGNORE`, `UPDATE ... SET status='started', started_session=?4 WHERE tenant_scope=?1 AND schedule_id=?2 AND fire_count=?3`, `SELECT ... WHERE status='claimed' ORDER BY tenant_scope, schedule_id, fire_count`. `fire_count` converts through `i64::try_from` with `StoreIntegrity { code: "sqlite_fire_count" }` (same shape as `i64_from_u64` in `store.rs:333`). Export `FireRow, FireStatus, FireStore, idempotency_key` from `lib.rs`.

- [ ] **Step 4: Run** — `cargo test -p finstack-ai-workflow-worker && cargo clippy -p finstack-ai-workflow-worker --all-targets` → PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Record claimed cron fires with idempotency keys"
```

---

### Task 6: Inbox — `InboxRow`, `InboxStore`, both impls

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/inbox.rs`
- Modify: `src/memory.rs`, `src/sqlite.rs`, `src/lib.rs`

**Interfaces:**
- Produces:
  - `InboxKind { Interaction, External }` (`as_str` → `"interaction"`/`"external"`; `parse` fails closed `StoreIntegrity { code: "inbox_kind" }`).
  - `InboxRow { tenant_scope: Arc<str>, session_id: SessionId, pending_id: Arc<str>, kind: InboxKind, payload: Arc<[u8]>, received_at: Timestamp }`.
  - `trait InboxStore: Send + Sync`:
    - `fn insert(&self, row: &InboxRow) -> Result<(), WorkerError>;` — `INSERT OR REPLACE` (a redelivered response overwrites; duplicate settlement is journal-side, per §10.4).
    - `fn load_all(&self) -> Result<Vec<InboxRow>, WorkerError>;`
    - `fn delete(&self, tenant_scope: &str, session_id: SessionId, pending_id: &str) -> Result<(), WorkerError>;`
- `payload` is the `serde_json` serialization of `InteractionResolutionCommand` / `ExternalEffectCompletionCommand` (both `Serialize` + validating `Deserialize`, `crates/finstack-ai-kernel/src/records/run/external.rs:109-218`). The worker deserializes at consume time; a payload that fails to deserialize is a `StoreIntegrity { code: "inbox_payload" }` handled per-row.

- [ ] **Step 1: Write the failing tests** (generic over both stores, same pattern as Task 5)

```rust
fn exercise_inbox(store: &dyn InboxStore) {
    let row = InboxRow {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        pending_id: Arc::from("interaction-1"),
        kind: InboxKind::Interaction,
        payload: Arc::from(&br#"{"approved":true}"#[..]),
        received_at: ts(3_000),
    };
    store.insert(&row).expect("insert");
    store.insert(&row).expect("redelivery replaces");
    assert_eq!(store.load_all().expect("all"), vec![row.clone()]);
    store
        .delete("tenant-a", id(1), "interaction-1")
        .expect("consume");
    assert!(store.load_all().expect("drained").is_empty());
}
```

Plus `#[test] fn memory_inbox_round_trips()` and `#[test] fn sqlite_inbox_round_trips()` wrappers as in Task 5.

- [ ] **Step 2: Run to verify failure** — compile FAIL.
- [ ] **Step 3: Implement** in `inbox.rs` + both stores (sqlite table exists from Task 4; codes `sqlite_inbox_insert`, `sqlite_inbox_query`, `sqlite_inbox_row`, `sqlite_inbox_delete`). Export from `lib.rs`.
- [ ] **Step 4: Run** — full crate tests + clippy → PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Add the durable response inbox to the workflow worker"
```

---

### Task 7: Cron discovery — `CronScheduleStore::load_due` (workflow-local)

**Files:**
- Modify: `extensions/workflow/finstack-ai-workflow-local/src/store.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-local/README.md` (one line documenting the new method)

**Interfaces:**
- Produces: on `CronScheduleStore`:

```rust
    /// Every schedule (any tenant) due at or before `now`.
    ///
    /// Third-party stores fail closed unless they override this method.
    ///
    /// # Errors
    ///
    /// Returns [`CronError::StoreUnavailable`] by default.
    fn load_due(&self, now: Timestamp) -> Result<Vec<CronSchedule>, CronError> {
        let _ = now;
        Err(CronError::StoreUnavailable {
            code: "load_due_unsupported",
        })
    }
```

with real implementations on `MemoryCronStore` (filter `next_fire_at <= now` across all tenants) and `SqliteCronStore` (`SELECT tenant_scope, schedule_id, expression, origin_unix_ms, next_fire_unix_ms, last_fired_unix_ms, fire_count FROM finstack_workflow_local_cron WHERE next_fire_unix_ms <= ?1 ORDER BY tenant_scope, schedule_id` — reuse the row-decoding of `load_tenant`, but read `tenant_scope` from the row).
- Consumed by Task 8's `fire_due_all`.

- [ ] **Step 1: Write the failing test** (in `store.rs` tests, reusing the fixtures of `memory_store_isolates_tenants` / `two_sqlite_stores_claim_exactly_one_fire`):

```rust
#[test]
fn load_due_crosses_tenants_and_respects_now() {
    let store = MemoryCronStore::new();
    let origin = Timestamp::from_unix_ms(2_000).expect("origin");
    for (tenant, next) in [("tenant-a", 2_010), ("tenant-b", 2_020)] {
        store
            .upsert(&CronSchedule {
                tenant_scope: Arc::from(tenant),
                schedule_id: Arc::from("tick"),
                expression: IntervalSchedule::parse("every 10ms").expect("expr"),
                origin,
                next_fire_at: Timestamp::from_unix_ms(next).expect("next"),
                last_fired_at: None,
                fire_count: 0,
            })
            .expect("upsert");
    }
    let due = store
        .load_due(Timestamp::from_unix_ms(2_015).expect("now"))
        .expect("due");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].tenant_scope.as_ref(), "tenant-a");
}
```

And a sqlite twin using `SqliteCronStore::open` on a tempfile.

- [ ] **Step 2: Run to verify failure** — `cargo test -p finstack-ai-workflow-local load_due` → compile FAIL (method missing) or default-error FAIL.
- [ ] **Step 3: Implement** the trait default + both overrides.
- [ ] **Step 4: Run** — `cargo test -p finstack-ai-workflow-local` → PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-local
git commit -m "Let cron stores enumerate due schedules across tenants"
```

---### Task 8: `park()` and test scaffolding for kernel-backed tests

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/park.rs`
- Create: `extensions/workflow/finstack-ai-workflow-worker/tests/worker/main.rs`
- Create: `extensions/workflow/finstack-ai-workflow-worker/tests/worker/helpers/mod.rs`
- Create: `extensions/workflow/finstack-ai-workflow-worker/tests/worker/park.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces:

```rust
/// Classify the current wait, index it for the worker, and drop the owner.
///
/// The row is a hint: the journal stays authoritative. A terminal state
/// deletes any existing row instead of writing one.
///
/// # Errors
///
/// Returns [`WorkerError::NotParked`] when no wait is classified, and
/// store or handoff failures otherwise.
pub fn park(
    session: &mut WorkflowSession,
    wake: &dyn WakeIndexStore,
    workflow_kind: &str,
) -> Result<WorkflowCheckpoint, WorkerError>
```

Behavior: `classify_wait(session.last_state())`; `None` → `Err(NotParked)`. `persist_handoff()` gives tenant/session/lane/run. `Terminal` → `wake.delete(...)`. `Timer { effect_id, due_at }` → row with `reason: Timer, wake_at: Some(due_at), pending_id: effect_id.to_canonical_string()`. `Interaction { interaction_id, .. }` → `reason: Interaction, wake_at: None, pending_id: interaction_id.to_canonical_string()`. `DeferredEffect { effect_id, .. }` → `reason: Deferred, pending_id: effect_id.to_canonical_string()`. Always `leased_by: None, lease_expires_at: None, attempts: 0`, then `session.abort_owner()`.
- Also produces the integration-test harness every later task uses.

- [ ] **Step 1: Copy the executable-spec helpers**

```bash
mkdir -p extensions/workflow/finstack-ai-workflow-worker/tests/worker/helpers
cp extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/helpers/mod.rs \
   extensions/workflow/finstack-ai-workflow-worker/tests/worker/helpers/mod.rs
```

Then edit the copy: change the doc comment first line to `//! Shared fixtures copied from finstack-ai-workflow-local's executable spec.`, and keep only what compiles against this crate's dev-deps (the file's imports already match: `finstack_ai_kernel`, `finstack_ai_runtime`, `finstack_ai_store_memory`, `finstack_ai_test`, `finstack_ai_workflow_local`). Delete `journal_trace`, `drive_to_after_model`, and `wait_state_on` if unused by the end of Task 12 — do that pruning in Task 12, not now.

`tests/worker/main.rs`:

```rust
//! Executable spec for the leased workflow worker.

mod helpers;
mod park;
```

- [ ] **Step 2: Write the failing test** — `tests/worker/park.rs`:

```rust
use std::sync::Arc;

use finstack_ai_runtime::{ExternalClock, Model, WorkflowWait};
use finstack_ai_test::{ScriptedModel, ScriptedModelPlan, ScriptedModelAction};
use finstack_ai_workflow_worker::{MemoryWorkerStore, WakeIndexStore, WakeReason, park};

use crate::helpers::{
    attach_session, drive_to_active_model_request, env, memory_store, profile, retryable_failure,
    spawn_model_owner, stage, timestamp, wait_state,
};
use finstack_ai_kernel::{RunPhase, Stage};
use finstack_ai_runtime::CommitCoordinator;

#[tokio::test]
async fn park_indexes_a_timer_wait_and_drops_the_owner() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    // Build a run parked on a retry timer — identical setup to
    // timer_survives_worker_restart in finstack-ai-workflow-local
    // (tests/local_workflow/restart.rs:1-41): spawn, drive to the model
    // request, submit the Retry directive, wait for Sleeping, drop owner.
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        800,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| state.phase == Some(RunPhase::BeforeFinalize)).await;
    owner
        .handle()
        .submit(
            env(2_300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                finstack_ai_kernel::ReducerStageOutcome::Retry(
                    finstack_ai_kernel::RetryDirective::try_new(
                        finstack_ai_kernel::RetryClassification::Model,
                        finstack_ai_kernel::KernelDuration::from_millis(10),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    wait_state(&store, |state| state.phase == Some(RunPhase::Sleeping)).await;
    drop(owner);

    let mut session = attach_session(store, Arc::clone(&model), clock, 800).await;
    let WorkflowWait::Timer { due_at, .. } = session.drive_until_wait().await.expect("timer")
    else {
        panic!("expected timer wait");
    };
    let wake = MemoryWorkerStore::new();
    let checkpoint = park(&mut session, &wake, "research").expect("park");
    assert!(!session.owner_is_live());
    let rows = wake.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason, WakeReason::Timer);
    assert_eq!(rows[0].wake_at, Some(due_at));
    assert_eq!(rows[0].session_id, checkpoint.session_id);
    assert_eq!(rows[0].workflow_kind.as_ref(), "research");
}
```

Add to the copied `helpers/mod.rs` (the local crate's `attach_driver` wraps a `LocalWorkflowDriver`; the worker tests want the bare session):

```rust
pub(crate) async fn attach_session(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    clock: ExternalClock,
    seed: u64,
) -> WorkflowSession {
    WorkflowSession::trusted(store, locator(), clock, seed)
        .await
        .expect("attach")
        .with_ports(model, locked_profile(), None)
}
```

- [ ] **Step 3: Run to verify failure** — `cargo test -p finstack-ai-workflow-worker --test worker` → compile FAIL (`park` missing).

- [ ] **Step 4: Implement `src/park.rs`**

```rust
use finstack_ai_runtime::{WorkflowCheckpoint, WorkflowSession, WorkflowWait, classify_wait};

use crate::error::WorkerError;
use crate::wake::{WakeIndexStore, WakeReason, WakeRow};
use std::sync::Arc;

pub fn park(
    session: &mut WorkflowSession,
    wake: &dyn WakeIndexStore,
    workflow_kind: &str,
) -> Result<WorkflowCheckpoint, WorkerError> {
    let wait = classify_wait(session.last_state()).ok_or(WorkerError::NotParked)?;
    let checkpoint = session.persist_handoff()?;
    match &wait {
        WorkflowWait::Terminal { .. } => {
            wake.delete(checkpoint.tenant_scope.as_ref(), checkpoint.session_id)?;
        }
        WorkflowWait::Timer { effect_id, due_at } => {
            wake.upsert(&row(
                &checkpoint,
                workflow_kind,
                WakeReason::Timer,
                Some(*due_at),
                effect_id.to_canonical_string(),
            ))?;
        }
        WorkflowWait::Interaction { interaction_id, .. } => {
            wake.upsert(&row(
                &checkpoint,
                workflow_kind,
                WakeReason::Interaction,
                None,
                interaction_id.to_canonical_string(),
            ))?;
        }
        WorkflowWait::DeferredEffect { effect_id, .. } => {
            wake.upsert(&row(
                &checkpoint,
                workflow_kind,
                WakeReason::Deferred,
                None,
                effect_id.to_canonical_string(),
            ))?;
        }
    }
    session.abort_owner();
    Ok(checkpoint)
}

fn row(
    checkpoint: &WorkflowCheckpoint,
    workflow_kind: &str,
    reason: WakeReason,
    wake_at: Option<finstack_ai_kernel::Timestamp>,
    pending_id: String,
) -> WakeRow {
    WakeRow {
        tenant_scope: Arc::clone(&checkpoint.tenant_scope),
        session_id: checkpoint.session_id,
        lane_id: checkpoint.lane_id,
        run_id: checkpoint.run_id,
        workflow_kind: Arc::from(workflow_kind),
        reason,
        wake_at,
        pending_id: Arc::from(pending_id.as_str()),
        leased_by: None,
        lease_expires_at: None,
        attempts: 0,
    }
}
```

(Doc comments as in **Interfaces**; export `park` from `lib.rs`.)

- [ ] **Step 5: Run** — `cargo test -p finstack-ai-workflow-worker` → PASS.
- [ ] **Step 6: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Index parked waits for the worker through a park helper"
```

---

### Task 9: `WorkflowWorker`, `WorkerBuilder`, tick — cron bridge + timer resume

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/worker.rs`
- Create: `extensions/workflow/finstack-ai-workflow-worker/tests/worker/tick.rs`
- Modify: `src/lib.rs`, `tests/worker/main.rs`

**Interfaces:**
- Produces:

```rust
/// Binds host-owned ports onto a bare attached session.
pub trait PortsFactory: Send + Sync {
    /// Rebind model/tool/middleware ports for one workflow kind.
    ///
    /// # Errors
    ///
    /// Returns host-defined failures as [`WorkerError`].
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError>;
}

/// Identity of a run started from a cron fire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedRun {
    /// Canonical session id string of the accepted run.
    pub session_id: Arc<str>,
}

/// Starts one run for one claimed cron fire, idempotently.
pub trait RunStarter: Send + Sync {
    /// Start (or find, when the key was already used) the run for `fire`.
    fn start<'a>(
        &'a self,
        fire: &'a CronFire,
        idempotency_key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<StartedRun, WorkerError>> + Send + 'a>>;
}

pub struct WorkerBuilder { /* private */ }
impl WorkerBuilder {
    pub fn new(
        journal: Arc<dyn JournalStore>,
        cron: Arc<dyn CronScheduleStore>,
        wake: Arc<dyn WakeIndexStore>,
        fires: Arc<dyn FireStore>,
        inbox: Arc<dyn InboxStore>,
    ) -> Self;
    #[must_use] pub fn worker_id(self, worker_id: &str) -> Self;          // default "worker-1"
    #[must_use] pub fn lease_ttl_ms(self, ttl: u64) -> Self;              // default 30_000
    #[must_use] pub fn drive_timeout(self, timeout: Duration) -> Self;    // default 2 s
    #[must_use] pub fn clock(self, clock: ExternalClock) -> Self;         // default ExternalClock::new(Timestamp::from_unix_ms(0))
    #[must_use] pub fn register_ports(self, kind: &str, factory: Arc<dyn PortsFactory>) -> Self;
    #[must_use] pub fn register_starter(self, schedule_id: &str, starter: Arc<dyn RunStarter>) -> Self;
    #[must_use] pub fn build(self) -> WorkflowWorker;
}

/// Per-tick outcome counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    pub cron_fires: usize,
    pub runs_started: usize,
    pub sessions_resumed: usize,
    pub sessions_reparked: usize,
    pub failures: usize,
}

impl WorkflowWorker {
    #[must_use] pub fn clock(&self) -> &ExternalClock;
    pub async fn tick(&self) -> Result<TickReport, WorkerError>;
}
```

- Tick order (each phase isolates per-item errors into `failures` + `record_failure`/log, never aborting the tick):
  1. `now = self.clock.now()` (map to `TimeOverflow`).
  2. **Cron:** `cron.load_due(now)`; for each due schedule, build the claimed row exactly as `LocalWorkflowDriver::fire_due` does (`last_fired_at = Some(now)`, `fire_count += 1`, `next_fire_at = expression.next_after(origin, now)`), call `cron.try_claim(...)`; on win, `fires.record_claimed(FireRow { status: Claimed, fired_at: now, fire_count: claimed.fire_count, .. })`, increment `cron_fires`.
  3. **Bridge:** `fires.load_unstarted()`; for each row with a registered starter (`starters.get(schedule_id)`), call `starter.start(&fire, &idempotency_key(&row))`; on success `fires.mark_started(...)`, increment `runs_started`. Rows without a starter are logged (a `log` callback is out of scope; count them in `failures` — visible, never dropped) but NOT marked started.
  4. **Wake:** `wake.load_due(now)`, plus `inbox.load_all()` indexed by `(tenant_scope, session_id, pending_id)`. For each row: non-timer rows are skipped unless the inbox map has a matching entry. `wake.try_claim(...)` — losers are skipped silently. Winners: `WorkflowSession::trusted(journal, locator, clock.clone(), seed)` where `locator = OperationLocator::try_new(tenant, session_id, lane_id, run_id)` (a failed `try_new` is `StoreIntegrity { code: "wake_locator" }`) and `seed = self.seed_counter.fetch_add(1, Ordering::AcqRel)`; then `ports.bind(session)?` (missing kind → `UnknownKind`) and `.with_drive_timeout(self.drive_timeout)`. For inbox-driven rows first deserialize and submit: `Interaction` → `serde_json::from_slice::<InteractionResolutionCommand>` then `session.resolve_interaction(command, now).await`; `Deferred` → `ExternalEffectCompletionCommand` then `session.complete_external(command, now).await`; then `inbox.delete(...)`. Then `session.drive_until_wait().await`; on `Ok(_)` re-park via `park(&mut session, wake, kind)` — `park` deletes on Terminal (increment `sessions_resumed`, and `sessions_reparked` when the row was re-upserted, i.e. wait was not Terminal). On any per-row error: `wake.record_failure(tenant, session_id, retry_at)` with `retry_at = lease_deadline(now, backoff_ms)?` where `backoff_ms = 1_000 * 2^min(attempts, 6)`, increment `failures`.
- Consumes: everything from Tasks 1–8.

- [ ] **Step 1: Write the failing test** — `tests/worker/tick.rs`, registered in `main.rs` (`mod tick;`):

```rust
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_kernel::Timestamp;
use finstack_ai_runtime::{ExternalClock, Model};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_local::{CronFire, IntervalSchedule, MemoryCronStore, CronSchedule, CronScheduleStore};
use finstack_ai_workflow_worker::{
    MemoryWorkerStore, PortsFactory, RunStarter, StartedRun, WakeIndexStore, WorkerBuilder,
    WorkerError, park,
};

use crate::helpers::{
    attach_session, completed_plan, locator, memory_store, profile, retryable_failure, timestamp,
};

struct BindPorts {
    model: Arc<dyn Model>,
}

impl PortsFactory for BindPorts {
    fn bind(
        &self,
        session: finstack_ai_runtime::WorkflowSession,
    ) -> Result<finstack_ai_runtime::WorkflowSession, WorkerError> {
        Ok(session.with_ports(
            Arc::clone(&self.model),
            crate::helpers::locked_profile(),
            None,
        ))
    }
}

struct CountingStarter {
    calls: AtomicUsize,
}

impl RunStarter for CountingStarter {
    fn start<'a>(
        &'a self,
        _fire: &'a CronFire,
        idempotency_key: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<StartedRun, WorkerError>> + Send + 'a>>
    {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let key: Arc<str> = Arc::from(idempotency_key);
        Box::pin(async move { Ok(StartedRun { session_id: key }) })
    }
}

#[tokio::test]
async fn tick_fires_due_cron_and_starts_runs_exactly_once() {
    let journal = memory_store();
    let cron = Arc::new(MemoryCronStore::new());
    let store = Arc::new(MemoryWorkerStore::new());
    cron.upsert(&CronSchedule {
        tenant_scope: Arc::from("tenant-a"),
        schedule_id: Arc::from("nightly"),
        expression: IntervalSchedule::parse("every 10ms").expect("expr"),
        origin: timestamp(2_000),
        next_fire_at: timestamp(2_010),
        last_fired_at: None,
        fire_count: 0,
    })
    .expect("schedule");
    let starter = Arc::new(CountingStarter { calls: AtomicUsize::new(0) });
    let clock = ExternalClock::new(timestamp(2_000));
    let worker = WorkerBuilder::new(
        journal,
        cron,
        Arc::clone(&store) as Arc<dyn finstack_ai_workflow_worker::WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn finstack_ai_workflow_worker::FireStore>,
        Arc::clone(&store) as Arc<dyn finstack_ai_workflow_worker::InboxStore>,
    )
    .clock(clock.clone())
    .register_starter("nightly", Arc::clone(&starter) as Arc<dyn RunStarter>)
    .build();

    let early = worker.tick().await.expect("early tick");
    assert_eq!(early.cron_fires, 0);
    clock.set(timestamp(2_015));
    let due = worker.tick().await.expect("due tick");
    assert_eq!(due.cron_fires, 1);
    assert_eq!(due.runs_started, 1);
    let repeat = worker.tick().await.expect("repeat tick");
    assert_eq!(repeat.runs_started, 0, "fire is not re-started");
    assert_eq!(starter.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn tick_resumes_a_due_timer_to_completion() {
    let journal = memory_store();
    // Model: first request fails retryably (parks a retry timer), the
    // retried request completes.
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![
            ScriptedModelPlan {
                actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
            },
            completed_plan("done"),
        ],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    // Park a run on its retry timer exactly as in Task 8's park test
    // (spawn owner, drive to model request, submit Retry, wait Sleeping,
    // drop owner, attach, drive_until_wait to Timer) — then park():
    let mut session = build_timer_parked_session(&journal, &model, &clock).await;
    let store = Arc::new(MemoryWorkerStore::new());
    park(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn finstack_ai_workflow_worker::WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn finstack_ai_workflow_worker::FireStore>,
        Arc::clone(&store) as Arc<dyn finstack_ai_workflow_worker::InboxStore>,
    )
    .clock(clock.clone())
    .register_ports("research", Arc::new(BindPorts { model }))
    .build();

    let before_due = worker.tick().await.expect("before due");
    assert_eq!(before_due.sessions_resumed, 0);
    clock.jump(60_000).expect("past due");
    let resumed = worker.tick().await.expect("resume");
    assert_eq!(resumed.sessions_resumed, 1);
    assert!(
        store.load_tenant("tenant-a").expect("rows").is_empty(),
        "terminal run deletes its wake row"
    );
}
```

`build_timer_parked_session` is a test-local `async fn` in `tick.rs` containing the exact spawn/submit/wait sequence from Task 8's park test (extract it there into `helpers/mod.rs` as `pub(crate) async fn park_on_retry_timer(store, model, clock, seed) -> WorkflowSession` during this task so both tests share it — that refactor is part of this task's step).

- [ ] **Step 2: Run to verify failure** — compile FAIL (`WorkerBuilder` missing).

- [ ] **Step 3: Implement `src/worker.rs`** per the **Interfaces** contract. Key skeleton:

```rust
pub struct WorkflowWorker {
    journal: Arc<dyn JournalStore>,
    cron: Arc<dyn CronScheduleStore>,
    wake: Arc<dyn WakeIndexStore>,
    fires: Arc<dyn FireStore>,
    inbox: Arc<dyn InboxStore>,
    ports: BTreeMap<Arc<str>, Arc<dyn PortsFactory>>,
    starters: BTreeMap<Arc<str>, Arc<dyn RunStarter>>,
    clock: ExternalClock,
    worker_id: Arc<str>,
    lease_ttl_ms: u64,
    drive_timeout: Duration,
    seed_counter: AtomicU64,
}
```

`tick` implements the four phases from **Interfaces**. The wake phase's per-row body goes in a private `async fn resume_row(&self, row: &WakeRow, inbox_entry: Option<&InboxRow>, now: Timestamp) -> Result<bool, WorkerError>` returning `true` when the run reached Terminal (row deleted) — so `tick` stays under clippy's length lints. Inside `resume_row`, after resolving/completing and `drive_until_wait`, always call `park(&mut session, self.wake.as_ref(), row.workflow_kind.as_ref())` — `park` writes the fresh row (unleased) or deletes on Terminal, which also releases the lease by replacement; there is no separate `release` call.

Export from `lib.rs`: `PortsFactory, RunStarter, StartedRun, TickReport, WorkerBuilder, WorkflowWorker`.

- [ ] **Step 4: Run** — `cargo test -p finstack-ai-workflow-worker && cargo clippy -p finstack-ai-workflow-worker --all-targets` → PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Drive cron fires and due timers from a leased worker tick"
```

---

### Task 10: Inbox-driven resumes — interaction and deferred end-to-end

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/tests/worker/inbox_resume.rs`
- Modify: `src/worker.rs` (only if Step 3 finds gaps), `tests/worker/main.rs`

**Interfaces:**
- Produces on `WorkflowWorker`:

```rust
    /// Durably record one interaction response for a future tick.
    ///
    /// # Errors
    ///
    /// Returns serialization or store failures.
    pub fn deliver_interaction(
        &self,
        command: &InteractionResolutionCommand,
        received_at: Timestamp,
    ) -> Result<(), WorkerError>;

    /// Durably record one external completion for a future tick.
    pub fn deliver_external(
        &self,
        command: &ExternalEffectCompletionCommand,
        received_at: Timestamp,
    ) -> Result<(), WorkerError>;
```

Both extract `(tenant_scope, session_id)` from `command.locator`, `pending_id` from `command.resolution.interaction_id()` / `command.completion.effect_id()` canonical strings, serialize with `serde_json::to_vec` (`StoreIntegrity { code: "inbox_encode" }` on failure), and `inbox.insert(...)`. No live session required.

- [ ] **Step 1: Write the failing tests** — `tests/worker/inbox_resume.rs`:

Two tests, each in three acts:

**`deferred_completion_delivered_while_down_resumes_on_tick`** — (a) Build a deferred-parked run exactly as `deferred_survives_worker_restart` does (`extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/restart.rs:65-136`): scripted model emits `ModelStreamItem::Deferred` with an `ExternalHandleRef`, wait for `AwaitingExternal`, drop the owner; attach, `drive_until_wait` → `WorkflowWait::DeferredEffect { effect_id, .. }`, `park(&mut session, store, "research")`, drop the session. (b) Construct the `ExternalEffectCompletionCommand` verbatim from that test (locator, `PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))`, `AuthorizationEvidence::try_new("policy-v1", "decision-v1")`, `ExternalEffectCompletion::try_new(effect_id, "ext-1", Completed { output, usage: None, artifacts })`) and call `worker.deliver_interaction`-analog `worker.deliver_external(&command, timestamp(3_000))` — worker built as in Task 9 with a `BindPorts` factory whose model has a follow-up `completed_plan("done")`. (c) `worker.tick()`: assert `sessions_resumed == 1`, the inbox is empty (`store.load_all()` via `InboxStore`), and the deferred wake row is gone or re-parked with a different reason.

**`interaction_resolution_delivered_while_down_resumes_on_tick`** — same shape using the interaction fixture from `interaction_survives_worker_restart` (`restart.rs:143-318`): the `ToolSpec`/`ScriptedToolset`/`ResolvedToolCatalog`/approval-required policy block builds the catalog; `BindPorts` for this test passes `Some(catalog)` to `with_ports`. Park on `WorkflowWait::Interaction { interaction_id, .. }`, deliver the `InteractionResolutionCommand` (verbatim construction from `restart.rs:301-313`), tick, assert resumed + inbox drained. A tick *before* delivery must claim nothing: assert `worker.tick().await.expect("no response yet").sessions_resumed == 0` between park and deliver.

Copy the needed extra imports/fixture code into the test file rather than growing `helpers/mod.rs` (the catalog block is single-use).

- [ ] **Step 2: Run to verify failure** — compile FAIL (`deliver_external` missing).
- [ ] **Step 3: Implement** the two `deliver_*` methods on `WorkflowWorker`; confirm `resume_row` (Task 9) already routes `Interaction`/`Deferred` inbox payloads through `resolve_interaction`/`complete_external` and deletes the consumed inbox row — patch it here if not.
- [ ] **Step 4: Run** — full crate tests + clippy → PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Resume interaction and deferred waits from the durable inbox"
```

---

### Task 11: Failure isolation, index-is-a-hint, tenant negatives

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-worker/tests/worker/hardening.rs`
- Modify: `tests/worker/main.rs`, `src/worker.rs` (only where a test exposes a gap)

**Interfaces:** consumes everything; produces no new API.

- [ ] **Step 1: Write the failing tests** — `tests/worker/hardening.rs`:

```rust
#[tokio::test]
async fn a_poisoned_row_backs_off_and_does_not_stall_the_tick() {
    // Wake row whose workflow_kind has NO registered factory, alongside a
    // healthy timer row (built with park_on_retry_timer + park).
    // tick(): failures == 1, sessions_resumed == 1 (the healthy row),
    // and the poisoned row now has attempts == 1, no lease, and a
    // wake_at pushed into the future (load_due at `now` excludes it).
}

#[tokio::test]
async fn a_stale_wake_row_is_corrected_by_the_journal() {
    // Park a run on a retry timer, then resolve it out-of-band: attach a
    // separate session, jump the clock past due, drive_until_wait to
    // Terminal — WITHOUT touching the wake store. The wake row now lies.
    // tick(): the worker claims the row, attaches, sees Terminal, and
    // deletes the row. sessions_resumed == 1, wake table empty.
}

#[tokio::test]
async fn cross_tenant_delivery_is_rejected_at_resolve_time() {
    // Park an interaction wait for tenant-a. Hand-insert an InboxRow whose
    // pending_id matches but whose payload command locator was built with
    // tenant "tenant-b" (construct the command with a tenant-b locator via
    // OperationLocator::try_new("tenant-b", ...) — same ids otherwise).
    // tick(): resolve_interaction fails with UnknownLocator (require_locator,
    // TM-19); failures == 1; the run is NOT resolved (wake row still
    // reason == Interaction); the poisoned inbox row is deleted so it cannot
    // wedge the loop forever.
}

#[test]
fn wake_claim_defaults_fail_closed() {
    struct Naked;
    impl WakeIndexStore for Naked {
        fn upsert(&self, _: &WakeRow) -> Result<(), WorkerError> { Ok(()) }
        fn delete(&self, _: &str, _: finstack_ai_kernel::SessionId) -> Result<(), WorkerError> { Ok(()) }
        fn load_due(&self, _: Timestamp) -> Result<Vec<WakeRow>, WorkerError> { Ok(Vec::new()) }
        fn load_tenant(&self, _: &str) -> Result<Vec<WakeRow>, WorkerError> { Ok(Vec::new()) }
        fn record_failure(&self, _: &str, _: finstack_ai_kernel::SessionId, _: Timestamp) -> Result<(), WorkerError> { Ok(()) }
    }
    let err = Naked
        .try_claim("tenant-a", id(1), "w", ts(0), 1_000)
        .expect_err("fail closed");
    assert_eq!(err.code(), "wake_claim_unsupported");
}
```

Write the three async tests out fully using the fixtures already established in Tasks 8–10 (the comments above are the assertions to encode, not placeholders to leave).

- [ ] **Step 2: Run to verify failure** — expect at least the poisoned-inbox-row deletion and stale-row-correction behaviors to fail until `resume_row` handles them.
- [ ] **Step 3: Implement the gaps** in `resume_row`: (a) an inbox payload whose submit is rejected with `WorkflowDriverError::UnknownLocator` deletes the inbox row, records the failure on the wake row, and continues; (b) a claimed row whose recovered state classifies Terminal is deleted (this already falls out of `park` — verify); (c) `UnknownKind` records failure with backoff.
- [ ] **Step 4: Run** — full crate tests + clippy → PASS.
- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Harden the worker tick against stale rows and cross-tenant delivery"
```

---

### Task 12: Daemon loop, binary, docs

**Files:**
- Modify: `extensions/workflow/finstack-ai-workflow-worker/src/worker.rs`
- Create: `extensions/workflow/finstack-ai-workflow-worker/src/bin/finstack_workflow_worker.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-worker/Cargo.toml`, `README.md`, `tests/worker/helpers/mod.rs` (prune unused helpers)

**Interfaces:**
- Produces:

```rust
/// Handle to a spawned worker loop.
pub struct WorkerHandle { /* shutdown: tokio::sync::watch::Sender<bool>, join: tokio::task::JoinHandle<()> */ }

impl WorkerHandle {
    /// Signal shutdown and wait for the loop to finish the current tick.
    pub async fn shutdown(self);
}

impl WorkflowWorker {
    /// Run the tick loop every `poll_interval`, pumping the clock from
    /// [`SystemClock`] before each tick. Tick errors are counted and the
    /// loop continues; it never panics out.
    #[must_use]
    pub fn spawn(self: Arc<Self>, poll_interval: Duration) -> WorkerHandle;
}
```

`spawn` body: `tokio::spawn` a loop over `tokio::time::interval(poll_interval)` and a `watch::Receiver`; each iteration does `self.clock.set(SystemClock.now()?)` (on clock error, skip the tick) then `let _ = self.tick().await;` — per-tick failures are already isolated inside `tick`.

- [ ] **Step 1: Write the failing test** (append to `tests/worker/tick.rs`):

```rust
#[tokio::test]
async fn spawn_ticks_and_shuts_down_cleanly() {
    // Worker over empty stores; spawn with a 5ms interval, sleep 30ms,
    // shutdown().await — the test passes if shutdown returns (no hang)
    // and nothing panicked. Uses real time; keep intervals tiny.
    let worker = Arc::new(/* WorkerBuilder over fresh memory stores */.build());
    let handle = Arc::clone(&worker).spawn(StdDuration::from_millis(5));
    tokio::time::sleep(StdDuration::from_millis(30)).await;
    handle.shutdown().await;
}
```

- [ ] **Step 2: Run to verify failure** — compile FAIL.
- [ ] **Step 3: Implement `spawn`/`WorkerHandle`**, then the binary:

`src/bin/finstack_workflow_worker.rs`:

```rust
//! Stock daemon: fires cron and resumes parked runs for hosts that
//! registered nothing dynamic. Usage: finstack_workflow_worker <sqlite-path>.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_workflow_local::SqliteCronStore;
use finstack_ai_workflow_worker::{SqliteWorkerStore, WorkerBuilder};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: finstack_workflow_worker <sqlite-path>");
        std::process::exit(2);
    });
    let store = Arc::new(SqliteWorkerStore::open(&path).expect("worker store"));
    let cron = Arc::new(SqliteCronStore::open(&path).expect("cron store"));
    let journal = Arc::new(
        finstack_ai_store_sqlite::SqliteJournalStore::try_open(
            finstack_ai_store_sqlite::SqliteStoreConfig {
                path: path.clone().into(),
                ..Default::default()
            },
        )
        .expect("journal"),
    );
    let worker = Arc::new(
        WorkerBuilder::new(
            journal,
            cron,
            Arc::clone(&store) as _,
            Arc::clone(&store) as _,
            Arc::clone(&store) as _,
        )
        .build(),
    );
    let handle = worker.spawn(Duration::from_millis(500));
    tokio::signal::ctrl_c().await.expect("signal");
    handle.shutdown().await;
}
```

Check whether `SqliteStoreConfig` implements `Default`; if not, spell out the same literal used by `open_sqlite_journal` in `extensions/workflow/finstack-ai-workflow-local/tests/local_workflow/restart.rs:326-340`. Add to `Cargo.toml`: `finstack-ai-store-sqlite = { workspace = true }` moves from dev-deps to a `[dependencies]` entry gated by the binary — simplest correct form is a plain dependency plus:

```toml
[[bin]]
name = "finstack_workflow_worker"
path = "src/bin/finstack_workflow_worker.rs"
```

(The stock daemon fires cron and corrects stale rows; runs needing ports resume only in embedding hosts — say exactly that in its module doc and the README.)

Prune `tests/worker/helpers/mod.rs` of anything now unused (`journal_trace`, `drive_to_after_model`, `wait_state_on` — keep whatever Tasks 8–11 ended up using). Extend the README with a short "Embedding" section showing `WorkerBuilder` + `register_ports` + `register_starter` + `spawn`, and a "Guarantees" section: lease CAS + journal append CAS, fire-once catch-up, idempotency keys `(tenant, schedule_id, fire_count)`, index-is-a-hint.

- [ ] **Step 4: Run everything**

Run: `cargo test -p finstack-ai-workflow-worker -p finstack-ai-workflow-local -p finstack-ai-runtime && cargo clippy -p finstack-ai-workflow-worker --all-targets`
Expected: PASS, no warnings. Then a full-workspace sanity pass: `cargo test --workspace` — check the exit code explicitly.

- [ ] **Step 5: Commit**

```bash
git add extensions/workflow/finstack-ai-workflow-worker
git commit -m "Spawn the worker daemon loop with a stock binary"
```

---

## Self-Review Notes (already applied)

- **Spec coverage:** §4 components → Tasks 2/3/4/9/12; §5 wake index + park → Tasks 3/4/8; §6 fire bridge + idempotency → Tasks 5/9; §7 inbox → Tasks 6/10; §8 lease protocol → Tasks 3/4 (CAS + expiry + renew) and 9 (drive timeout below TTL is the builder default pairing: 2 s « 30 s); §9 tick → Task 9; §10 error handling → Tasks 2/11 (fail-closed defaults, backoff, unknown kind); §11 testing → Tasks 4 (two-store claim, lease expiry), 9 (cron exactly-once, timer restart), 10 (interaction/deferred restart via inbox), 11 (tenant negative, index-is-a-hint, failure isolation), 5 (fire idempotency); §13 housekeeping → Task 1 (timeout) and Task 2 (stale comment). Heartbeat renewal (§8) ships as `renew` on the trait (Task 4) with the holder-only test; the tick loop does not yet call it mid-drive because the default drive timeout (2 s) is far below the default lease TTL (30 s) — noted in `worker.rs` docs as the invariant `drive_timeout < lease_ttl`.
- **Deviation from spec:** the spec's §11 "tokio paused time" is replaced by manual `ExternalClock` control, which the existing executable spec already uses and which exercises the same no-real-sleeps property; the one real-time test (Task 12 spawn) uses millisecond intervals.
- **Type consistency:** `WakeIndexStore`/`FireStore`/`InboxStore` names and signatures are defined once in Tasks 3/5/6 and consumed verbatim in Tasks 8–12; `park` takes `&mut WorkflowSession` (not `LocalWorkflowDriver`) everywhere; `idempotency_key` format is asserted by test in Task 5 and consumed in Task 9.
