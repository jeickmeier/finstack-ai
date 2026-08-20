# Billing Ledger Observer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A read-only `Observer` leaf crate, `finstack-ai-observer-billing`, that aggregates kernel `CostAmount` (integer micros, unit, pricing_policy_version) by session, run, model, and policy version — the FinStack-shaped spend projection.

**Architecture:** New crate at `extensions/observers/finstack-ai-observer-billing`, modeled directly on `extensions/observers/finstack-ai-observer-metrics`. Synchronous ingest of `RunEvent` batches under a `Mutex`, bounded `ObserverQueue` for backpressure diagnostics, `BTreeMap` ledger keyed `(SessionId, RunId, Option<model>)` with spend cells keyed `(unit, pricing_policy_version)`. No kernel/runtime changes.

**Tech Stack:** Rust workspace crate; deps `finstack-ai-kernel`, `finstack-ai-runtime` (default-features off + `native-tokio`), `serde_json`, `thiserror`; dev-deps `finstack-ai-test`, `tokio`.

**Spec:** `docs/superpowers/specs/2026-08-20-billing-observer-design.md`

## Global Constraints

- **No floating point in the money path.** Micros accumulate in `u128` via `saturating_add`; tokens in `u64` saturating. `#![warn(clippy::float_cmp)]` and no `f64` anywhere in this crate.
- **Never merge across `unit` or `pricing_policy_version`.** Spend cells are keyed by both.
- **Never synthesize cost.** No pricing table; absent `Usage.cost` increments `uncosted_effects` only.
- **Exports render every numeric as a decimal string**, never a JSON number.
- Crate lint header identical to `extensions/observers/finstack-ai-observer-metrics/src/lib.rs:1-22` (`forbid(unsafe_code)`, `deny(clippy::unwrap_used)`, `deny(clippy::expect_used)`, `deny(clippy::panic)`, `deny(clippy::unreachable)`, `warn(missing_docs)`, plus the `cfg_attr(test, allow(...))` block). Every public item gets a doc comment or the build fails.
- Component id `finstack.observer.billing`, version `0.0.1`.
- Constructor bounds: `queue_capacity` validated by `ObserverQueue::try_new` (1..=1_000_000); `max_entries` validated identically in our code.
- Constants: `MAX_PENDING_EFFECTS = 4096`, `MAX_MODEL_NAME_BYTES = 256`.
- All commands run from the repo root `/Users/jeickmeier/Projects/finstack-ai`. Run tests with `cargo test -p finstack-ai-observer-billing --locked` and check the exit code (per RTK guidance, do not gate decisions on summarized output).

---

### Task 1: Crate scaffold, workspace wiring, constructor, conformance

**Files:**
- Create: `extensions/observers/finstack-ai-observer-billing/Cargo.toml`
- Create: `extensions/observers/finstack-ai-observer-billing/src/lib.rs`
- Create: `extensions/observers/finstack-ai-observer-billing/src/tests.rs`
- Create: `extensions/observers/finstack-ai-observer-billing/README.md` (stub; full text in Task 6)
- Modify: root `Cargo.toml` (`[workspace] members` list and `[workspace.dependencies]`)

**Interfaces:**
- Produces: `BillingObserver::try_new(queue_capacity: usize, backpressure: ObserverBackpressure, max_entries: usize) -> Result<BillingObserver, BillingObserverError>`, `BillingObserverError::Configuration { reason: &'static str }`, `impl Observer for BillingObserver`, `BillingObserver::dropped() -> u64`, `BillingObserver::last_diagnostic() -> Option<ObserverDiagnostic>`. Later tasks add `snapshot()` and `export_jsonl()` to this same struct.

- [ ] **Step 1: Add the crate to the workspace**

In root `Cargo.toml`, add to `[workspace] members` (keep the list's existing ordering style, alongside the other `extensions/observers/...` entries):

```toml
"extensions/observers/finstack-ai-observer-billing",
```

and to `[workspace.dependencies]` (next to `finstack-ai-observer-metrics`):

```toml
finstack-ai-observer-billing = { path = "extensions/observers/finstack-ai-observer-billing", version = "1.0.0" }
```

- [ ] **Step 2: Write `Cargo.toml`**

```toml
[package]
name = "finstack-ai-observer-billing"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Read-only billing ledger observer aggregating kernel CostAmount for finstack-ai"
readme = "README.md"

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
serde_json = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros"] }

[lints]
workspace = true
```

Write `README.md` with just the title line `# finstack-ai-observer-billing` for now (Task 6 fills it in).

- [ ] **Step 3: Write the failing conformance test**

`src/tests.rs`:

```rust
use std::sync::Arc;

use finstack_ai_runtime::ObserverBackpressure;
use finstack_ai_test::check_observer_conformance;

use super::BillingObserver;

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let billing =
        BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 64).expect("billing");
    check_observer_conformance(&billing, Arc::from([]))
        .await
        .expect("conformance");
}

#[tokio::test]
async fn invalid_bounds_are_rejected() {
    assert!(BillingObserver::try_new(0, ObserverBackpressure::DropProgress, 64).is_err());
    assert!(BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 0).is_err());
    assert!(BillingObserver::try_new(8, ObserverBackpressure::DropProgress, 1_000_001).is_err());
}
```

- [ ] **Step 4: Run to verify it fails**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: compile FAILURE — `BillingObserver` not defined.

- [ ] **Step 5: Write the minimal `lib.rs`**

```rust
//! Read-only billing ledger observer. Aggregates kernel `CostAmount` by
//! session, run, model, and pricing policy version. Integer money only.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, RunEvent, Version};
use finstack_ai_runtime::{
    OBSERVER_QUEUE_OVERFLOW, Observer, ObserverBackpressure, ObserverDescriptor,
    ObserverDiagnostic, ObserverError, ObserverPayloadMode, ObserverQueue, ObserverQueuePush,
    PortFuture,
};
use thiserror::Error;

/// Maximum distinct attribution keys accepted by the ledger.
const MAX_ENTRIES_CEILING: usize = 1_000_000;

/// Billing-observer construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BillingObserverError {
    /// Configuration is malformed.
    #[error("billing_observer_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

#[derive(Debug, Default)]
struct LedgerState {}

/// Read-only spend-ledger observer. Default payload mode is metadata-only.
pub struct BillingObserver {
    descriptor: ObserverDescriptor,
    queue: ObserverQueue<()>,
    state: Mutex<LedgerState>,
    max_entries: usize,
    dropped: AtomicU64,
    diagnostic: Mutex<Option<ObserverDiagnostic>>,
}

impl BillingObserver {
    /// Construct a billing observer.
    ///
    /// # Arguments
    ///
    /// * `queue_capacity` - Bounded diagnostics-queue capacity (1..=1_000_000).
    /// * `backpressure` - Queue policy applied when the bound is reached.
    /// * `max_entries` - Distinct (session, run, model) keys retained (1..=1_000_000).
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity, queue bound, or entry bound.
    pub fn try_new(
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
        max_entries: usize,
    ) -> Result<Self, BillingObserverError> {
        if max_entries == 0 || max_entries > MAX_ENTRIES_CEILING {
            return Err(BillingObserverError::Configuration {
                reason: "invalid_max_entries",
            });
        }
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.observer.billing").map_err(|_| {
                        BillingObserverError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    Some(Version {
                        major: 0,
                        minor: 0,
                        patch: 1,
                    }),
                ),
                payload_mode: ObserverPayloadMode::MetadataOnly,
                metadata: Metadata::empty(),
            },
            queue: ObserverQueue::try_new(queue_capacity, backpressure).map_err(|_| {
                BillingObserverError::Configuration {
                    reason: "invalid_queue_capacity",
                }
            })?,
            state: Mutex::new(LedgerState::default()),
            max_entries,
            dropped: AtomicU64::new(0),
            diagnostic: Mutex::new(None),
        })
    }

    /// Cumulative adapter-queue drops.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed) + self.queue.dropped()
    }

    /// Last overflow or saturation diagnostic.
    #[must_use]
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic> {
        self.diagnostic.lock().ok().and_then(|slot| *slot)
    }

    fn ingest(&self, event: &RunEvent) {
        let Ok(_state) = self.state.lock() else {
            return;
        };
        let _ = event;
    }

    fn record_overflow(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(OBSERVER_QUEUE_OVERFLOW);
        }
    }
}

impl Observer for BillingObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        for event in batch.iter() {
            self.ingest(event);
            match self.queue.push(()) {
                Ok(ObserverQueuePush::Accepted) => {}
                Ok(ObserverQueuePush::Dropped) => self.record_overflow(),
                Err(error) => {
                    self.record_overflow();
                    return Box::pin(async move { Err(error) });
                }
            }
        }
        let _ = self.queue.drain();
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests;
```

- [ ] **Step 6: Run to verify it passes**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: PASS (2 tests). If `Cargo.lock` needs updating for the new member, run `cargo metadata --locked` fails → run plain `cargo check -p finstack-ai-observer-billing` once to update the lockfile, then re-run with `--locked`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/observers/finstack-ai-observer-billing
git commit -m "feat: scaffold finstack-ai-observer-billing observer leaf"
```

---

### Task 2: Core spend/usage aggregation from `EffectCompleted`

**Files:**
- Modify: `extensions/observers/finstack-ai-observer-billing/src/lib.rs`
- Modify: `extensions/observers/finstack-ai-observer-billing/src/tests.rs`

**Interfaces:**
- Consumes: `BillingObserver` from Task 1.
- Produces: `BillingObserver::snapshot() -> LedgerSnapshot`; public types `LedgerSnapshot { spend: Vec<SpendEntry>, usage: Vec<UsageEntry>, unattributed_effects: u64, overflowed_events: u64, dropped_events: u64 }`, `SpendEntry { session_id: SessionId, run_id: RunId, model: Option<Arc<str>>, provider: Option<ComponentRef>, unit: Arc<str>, pricing_policy_version: Arc<str>, micros: u128, costed_effects: u64 }`, `UsageEntry { session_id: SessionId, run_id: RunId, model: Option<Arc<str>>, input_tokens: u64, output_tokens: u64, effects: u64, uncosted_effects: u64 }`. All three derive `Debug, Clone, PartialEq, Eq`; `LedgerSnapshot` also derives `Default`.

- [ ] **Step 1: Add shared test helpers and the failing aggregation tests**

Replace `src/tests.rs` imports/helpers and append tests. Model-effect events are durable-derived and must carry `turn_id`, `model_request_id`, `effect_id`, and `Sensitivity::Confidential` (enforced by `validate_event_policy` in the kernel's `events/derive.rs`):

```rust
use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, CostAmount, Digest, EffectCompleted, EffectInput, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectRequested, EffectTag, EventTag, Id, IdTag,
    InvocationRecovery, LaneTag, ModelRequestTag, ProviderIds, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, RawJson, RetrySafety, RunEvent, RunEventBody, RunTag, Sensitivity,
    SessionTag, Timestamp, TurnTag, Usage, Version,
};
use finstack_ai_runtime::{Observer, ObserverBackpressure};
use finstack_ai_test::check_observer_conformance;

use super::BillingObserver;

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn model_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"schema"),
    }
}

/// Durable model-effect event on session 1 / run 3 with the given effect id.
fn model_event(sequence: u64, effect: u64, body: RunEventBody) -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(4)),
        Some(id::<ModelRequestTag>(5)),
        None,
        Some(id::<EffectTag>(effect)),
        None,
        sequence,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Confidential,
        body,
    )
    .expect("event")
}

fn usage(input: u64, output: u64, cost: Option<CostAmount>) -> Usage {
    Usage::try_new(Some(input), Some(output), None, cost, BTreeMap::new()).expect("usage")
}

fn completed(effect: u64, usage_value: Option<Usage>) -> RunEventBody {
    RunEventBody::EffectCompleted(
        EffectCompleted::try_new(
            id::<EffectTag>(effect),
            model_contract(),
            RawJson::parse("{}").expect("json"),
            usage_value,
            vec![],
            ProviderIds::empty(),
            None::<&str>,
            None,
        )
        .expect("completed"),
    )
}

fn requested(effect: u64, request_json: &str) -> RunEventBody {
    RunEventBody::EffectRequested(
        EffectRequested::try_new(
            id::<EffectTag>(effect),
            EffectKind::Model,
            None,
            Some(ComponentInvocation {
                component: ComponentId::parse("finstack.model.demo").expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: Digest::raw_json(b"cfg"),
                recovery: InvocationRecovery::RecomputeSafe,
            }),
            None,
            model_contract(),
            EffectInput::Model {
                request: RawJson::parse(request_json).expect("json"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("requested"),
    )
}

fn cost(unit: &str, micros: u64, policy: &str) -> CostAmount {
    CostAmount::try_new(unit, micros, policy).expect("cost")
}
```

Append the aggregation tests:

```rust
#[tokio::test]
async fn costed_completions_aggregate_by_unit_and_policy() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, completed(7, Some(usage(10, 20, Some(cost("USD", 250, "prices-v1")))))),
            model_event(2, 8, completed(8, Some(usage(1, 2, Some(cost("USD", 750, "prices-v1")))))),
            model_event(3, 9, completed(9, Some(usage(0, 0, Some(cost("USD", 5, "prices-v2")))))),
            model_event(4, 10, completed(10, Some(usage(3, 4, Some(cost("EUR", 9, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.spend.len(), 3);
    let usd_v1 = snapshot
        .spend
        .iter()
        .find(|row| row.unit.as_ref() == "USD" && row.pricing_policy_version.as_ref() == "prices-v1")
        .expect("usd v1 row");
    assert_eq!(usd_v1.micros, 1_000);
    assert_eq!(usd_v1.costed_effects, 2);
    assert_eq!(usd_v1.session_id, id::<SessionTag>(1));
    assert_eq!(usd_v1.run_id, id::<RunTag>(3));
    let usage_row = snapshot.usage.first().expect("usage row");
    assert_eq!(usage_row.input_tokens, 14);
    assert_eq!(usage_row.output_tokens, 26);
    assert_eq!(usage_row.effects, 4);
    assert_eq!(usage_row.uncosted_effects, 0);
}

#[tokio::test]
async fn uncosted_completions_are_counted_never_priced() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, completed(7, Some(usage(10, 20, None)))),
            model_event(2, 8, completed(8, None)),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert!(snapshot.spend.is_empty());
    let row = snapshot.usage.first().expect("usage row");
    assert_eq!(row.input_tokens, 10);
    assert_eq!(row.effects, 2);
    assert_eq!(row.uncosted_effects, 2);
}
```

Note: `completed(8, None)` has no `Usage` at all — decide in implementation that a completion with no usage still counts as `effects += 1` and `uncosted_effects += 1` (it consumed a model call whose spend is unknown).

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: compile FAILURE — `snapshot`, `LedgerSnapshot` not defined.

- [ ] **Step 3: Implement the ledger state, ingest, and snapshot**

In `lib.rs`, extend imports:

```rust
use std::collections::BTreeMap;

use finstack_ai_kernel::{
    ComponentId, ComponentRef, EffectId, Metadata, RunEvent, RunEventBody, RunEventKind, RunId,
    SessionId, Usage, Version,
};
```

Replace the empty `LedgerState` with the real model and add the public types:

```rust
/// Ledger attribution key: session, run, and optional model name.
type AttributionKey = (SessionId, RunId, Option<Arc<str>>);
/// Spend-cell key: unit and pricing policy version. Never merged.
type CostKey = (Arc<str>, Arc<str>);

#[derive(Debug, Default)]
struct CostCell {
    micros: u128,
    costed_effects: u64,
}

#[derive(Debug, Default)]
struct AttributionState {
    provider: Option<ComponentRef>,
    input_tokens: u64,
    output_tokens: u64,
    effects: u64,
    uncosted_effects: u64,
    spend: BTreeMap<CostKey, CostCell>,
}

#[derive(Debug, Clone)]
struct EffectOrigin {
    provider: Option<ComponentRef>,
    model: Option<Arc<str>>,
}

#[derive(Debug, Default)]
struct LedgerState {
    entries: BTreeMap<AttributionKey, AttributionState>,
    pending: BTreeMap<EffectId, EffectOrigin>,
    unattributed_effects: u64,
    overflowed_events: u64,
}

/// One spend row: cost accumulated for an attribution key under one unit and
/// pricing policy version. Micros are exact integers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendEntry {
    /// Session identity.
    pub session_id: SessionId,
    /// Run identity.
    pub run_id: RunId,
    /// Model name parsed from the model request, when known.
    pub model: Option<Arc<str>>,
    /// Provider component, when known.
    pub provider: Option<ComponentRef>,
    /// Cost unit (for example an ISO currency code).
    pub unit: Arc<str>,
    /// Pricing policy version the cost was recorded under.
    pub pricing_policy_version: Arc<str>,
    /// Exact accumulated micros (millionths of the unit).
    pub micros: u128,
    /// Completions that carried a cost in this cell.
    pub costed_effects: u64,
}

/// One usage row: token totals and cost coverage for an attribution key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEntry {
    /// Session identity.
    pub session_id: SessionId,
    /// Run identity.
    pub run_id: RunId,
    /// Model name parsed from the model request, when known.
    pub model: Option<Arc<str>>,
    /// Accumulated input tokens.
    pub input_tokens: u64,
    /// Accumulated output tokens.
    pub output_tokens: u64,
    /// Settled effects observed for this key.
    pub effects: u64,
    /// Settled effects that carried no cost amount.
    pub uncosted_effects: u64,
}

/// Point-in-time projection of the ledger. Deterministic ordering.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LedgerSnapshot {
    /// Spend rows ordered by (session, run, model, unit, policy version).
    pub spend: Vec<SpendEntry>,
    /// Usage rows ordered by (session, run, model).
    pub usage: Vec<UsageEntry>,
    /// Settled effects whose origin could not be tracked.
    pub unattributed_effects: u64,
    /// Events ignored because the ledger reached its entry bound.
    pub overflowed_events: u64,
    /// Observer queue drops.
    pub dropped_events: u64,
}
```

Add `snapshot` to `impl BillingObserver` and replace `ingest`:

```rust
    /// Snapshot the ledger. Returns an empty snapshot when state is poisoned.
    #[must_use]
    pub fn snapshot(&self) -> LedgerSnapshot {
        let Ok(state) = self.state.lock() else {
            return LedgerSnapshot::default();
        };
        let mut spend = Vec::new();
        let mut usage = Vec::new();
        for ((session_id, run_id, model), entry) in &state.entries {
            for ((unit, policy), cell) in &entry.spend {
                spend.push(SpendEntry {
                    session_id: *session_id,
                    run_id: *run_id,
                    model: model.clone(),
                    provider: entry.provider.clone(),
                    unit: Arc::clone(unit),
                    pricing_policy_version: Arc::clone(policy),
                    micros: cell.micros,
                    costed_effects: cell.costed_effects,
                });
            }
            usage.push(UsageEntry {
                session_id: *session_id,
                run_id: *run_id,
                model: model.clone(),
                input_tokens: entry.input_tokens,
                output_tokens: entry.output_tokens,
                effects: entry.effects,
                uncosted_effects: entry.uncosted_effects,
            });
        }
        LedgerSnapshot {
            spend,
            usage,
            unattributed_effects: state.unattributed_effects,
            overflowed_events: state.overflowed_events,
            dropped_events: self.dropped(),
        }
    }

    fn ingest(&self, event: &RunEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match event.kind() {
            RunEventKind::EffectCompleted => {
                if let RunEventBody::EffectCompleted(body) = event.body() {
                    let origin = state.pending.remove(&body.effect_id());
                    settle(
                        &mut state,
                        self.max_entries,
                        event.session_id(),
                        event.run_id(),
                        origin,
                        body.usage(),
                    );
                }
            }
            _ => {}
        }
    }
```

Add the free function `settle` (kept out of the impl so Task 3 reuses it for `EffectFailed`):

```rust
/// Fold one settled effect into the ledger. `usage` may be absent.
fn settle(
    state: &mut LedgerState,
    max_entries: usize,
    session_id: SessionId,
    run_id: RunId,
    origin: Option<EffectOrigin>,
    usage: Option<&Usage>,
) -> bool {
    let (provider, model) = match origin {
        Some(origin) => (origin.provider, origin.model),
        None => (None, None),
    };
    let key = (session_id, run_id, model);
    if !state.entries.contains_key(&key) && state.entries.len() >= max_entries {
        state.overflowed_events = state.overflowed_events.saturating_add(1);
        return false;
    }
    let entry = state.entries.entry(key).or_default();
    if entry.provider.is_none() {
        entry.provider = provider;
    }
    entry.effects = entry.effects.saturating_add(1);
    let cost = usage.and_then(Usage::cost);
    if let Some(usage) = usage {
        entry.input_tokens = entry
            .input_tokens
            .saturating_add(usage.input_tokens().unwrap_or(0));
        entry.output_tokens = entry
            .output_tokens
            .saturating_add(usage.output_tokens().unwrap_or(0));
    }
    match cost {
        Some(cost) => {
            let cell = entry
                .spend
                .entry((Arc::from(cost.unit()), Arc::from(cost.pricing_policy_version())))
                .or_default();
            cell.micros = cell.micros.saturating_add(u128::from(cost.micros()));
            cell.costed_effects = cell.costed_effects.saturating_add(1);
        }
        None => {
            entry.uncosted_effects = entry.uncosted_effects.saturating_add(1);
        }
    }
    true
}
```

(`Usage::cost` returns `Option<&CostAmount>`; if `usage.and_then(Usage::cost)` fails to type-check, write `usage.as_ref().and_then(|u| u.cost())` — `usage` here is already `Option<&Usage>`, so the direct form is `usage.and_then(|u| u.cost())`.)

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: PASS (all tests, including Task 1's).

- [ ] **Step 5: Commit**

```bash
git add extensions/observers/finstack-ai-observer-billing/src
git commit -m "feat: aggregate CostAmount spend and usage in billing observer"
```

---

### Task 3: Model and provider attribution via `EffectRequested`

**Files:**
- Modify: `extensions/observers/finstack-ai-observer-billing/src/lib.rs`
- Modify: `extensions/observers/finstack-ai-observer-billing/src/tests.rs`

**Interfaces:**
- Consumes: `settle(...)`, `EffectOrigin`, `LedgerState.pending` from Task 2; test helpers `requested(effect, request_json)`, `model_event`, `completed`.
- Produces: attribution behavior only — `SpendEntry.model` / `UsageEntry.model` populated from the request JSON's top-level `"model"` string, `SpendEntry.provider` from `ComponentInvocation`; `LedgerSnapshot.unattributed_effects` counts settlements whose origin was never tracked.

- [ ] **Step 1: Write the failing attribution tests**

Append to `src/tests.rs`:

```rust
#[tokio::test]
async fn model_and_provider_are_attributed_from_the_request() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, requested(7, r#"{"model":"demo-model-1","messages":[]}"#)),
            model_event(2, 7, completed(7, Some(usage(10, 20, Some(cost("USD", 100, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    let row = snapshot.spend.first().expect("spend row");
    assert_eq!(row.model.as_deref(), Some("demo-model-1"));
    let provider = row.provider.clone().expect("provider");
    assert_eq!(provider.id.to_string(), "finstack.model.demo");
    assert_eq!(snapshot.unattributed_effects, 0);
}

#[tokio::test]
async fn untracked_completions_count_as_unattributed() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([model_event(
            1,
            7,
            completed(7, Some(usage(1, 1, Some(cost("USD", 1, "prices-v1"))))),
        )]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.unattributed_effects, 1);
    let row = snapshot.spend.first().expect("spend row");
    assert!(row.model.is_none());
    assert_eq!(row.micros, 1);
}

#[tokio::test]
async fn oversized_or_missing_model_names_fall_back_to_none() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    let long_model = "m".repeat(300);
    let request = format!(r#"{{"model":"{long_model}"}}"#);
    billing
        .observe(Arc::from([
            model_event(1, 7, requested(7, &request)),
            model_event(2, 7, completed(7, Some(usage(1, 1, None)))),
            model_event(3, 8, requested(8, r#"{"messages":[]}"#)),
            model_event(4, 8, completed(8, Some(usage(1, 1, None)))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.usage.len(), 1);
    assert!(snapshot.usage[0].model.is_none());
    assert_eq!(snapshot.usage[0].effects, 2);
    assert_eq!(snapshot.unattributed_effects, 0);
}
```

Note on `provider.id`: `ComponentRef` is constructed via `ComponentRef::new(id, version)`. If its fields are not public, assert via the descriptor pattern used elsewhere — check the struct in `crates/finstack-ai-kernel` (`grep -n "struct ComponentRef" -A 10 crates/finstack-ai-kernel/src/primitives/component.rs`) and use its accessor (for example `provider.component()`/`provider.id()`) instead; adjust only the assertion line, not the behavior.

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: FAIL — `model` is `None` in the first test and `unattributed_effects` is 0 in the second (origin tracking not implemented).

- [ ] **Step 3: Implement origin tracking**

In `lib.rs` add imports `EffectInput`, `EffectKind` from `finstack_ai_kernel`, plus:

```rust
/// Pending model effects tracked for attribution.
const MAX_PENDING_EFFECTS: usize = 4096;
/// Longest model-name string accepted from a request payload.
const MAX_MODEL_NAME_BYTES: usize = 256;
```

Extend `ingest`'s match:

```rust
            RunEventKind::EffectRequested => {
                if let RunEventBody::EffectRequested(body) = event.body()
                    && body.kind() == EffectKind::Model
                {
                    if state.pending.len() >= MAX_PENDING_EFFECTS {
                        return;
                    }
                    let provider = body.component().map(|invocation| {
                        ComponentRef::new(
                            invocation.component.clone(),
                            Some(invocation.version),
                        )
                    });
                    let model = match body.input() {
                        EffectInput::Model { request } => parse_model_name(request.as_str()),
                        _ => None,
                    };
                    state
                        .pending
                        .insert(body.effect_id(), EffectOrigin { provider, model });
                }
            }
            RunEventKind::EffectFailed => {
                if let RunEventBody::EffectFailed(body) = event.body() {
                    let origin = state.pending.remove(&body.effect_id());
                    if origin.is_none() {
                        state.unattributed_effects = state.unattributed_effects.saturating_add(1);
                    }
                    settle(
                        &mut state,
                        self.max_entries,
                        event.session_id(),
                        event.run_id(),
                        origin,
                        body.usage(),
                    );
                }
            }
            RunEventKind::EffectDeferred | RunEventKind::EffectCancelled => {
                if let Some(effect_id) = event.effect_id() {
                    state.pending.remove(&effect_id);
                }
            }
```

and in the existing `EffectCompleted` arm, before calling `settle`, count missing origins the same way:

```rust
                    let origin = state.pending.remove(&body.effect_id());
                    if origin.is_none() {
                        state.unattributed_effects = state.unattributed_effects.saturating_add(1);
                    }
```

Add the parser (uses `serde_json` on the already-canonical `RawJson` text):

```rust
/// Extract a bounded top-level `"model"` string from canonical request JSON.
fn parse_model_name(request: &str) -> Option<Arc<str>> {
    let value: serde_json::Value = serde_json::from_str(request).ok()?;
    let name = value.get("model")?.as_str()?;
    if name.is_empty() || name.len() > MAX_MODEL_NAME_BYTES {
        return None;
    }
    Some(Arc::from(name))
}
```

Accessor-name check: `EffectRequested` exposes `kind()`, `component()`, `input()`, `effect_id()` — verify with `grep -n "pub fn \(kind\|component\|input\|effect_id\)" crates/finstack-ai-kernel/src/effects/kinds.rs` and adjust call names to the real accessors if they differ (fields may be returned as `Option<&ComponentInvocation>`; the `.map(...)` handles both by-ref with an added `.cloned()` where needed). `EffectFailed` exposes `effect_id()` and `usage()` (see `crates/finstack-ai-kernel/src/effects/lifecycle.rs:575`).

Also check whether `RunEventKind::EffectCancelled` exists (`grep -n "EffectCancelled" crates/finstack-ai-kernel/src/events/derive.rs`); if the event kind is not derived, drop it from the match arm and keep `EffectDeferred` only.

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/observers/finstack-ai-observer-billing/src
git commit -m "feat: attribute billing ledger entries to model and provider"
```

---

### Task 4: Entry-bound saturation and queue-overflow diagnostics

**Files:**
- Modify: `extensions/observers/finstack-ai-observer-billing/src/lib.rs`
- Modify: `extensions/observers/finstack-ai-observer-billing/src/tests.rs`

**Interfaces:**
- Consumes: `settle` returning `bool` (false = dropped for capacity), `LedgerState.overflowed_events` from Task 2.
- Produces: `pub const BILLING_LEDGER_SATURATED: ObserverDiagnostic` (code `"billing_ledger_saturated"`); saturation stores it via `last_diagnostic()`; queue overflow keeps the metrics-leaf behavior (`dropped() >= 1`, code `"observer_queue_overflow"`).

- [ ] **Step 1: Write the failing tests**

Append to `src/tests.rs`. The saturation test uses distinct **sessions** to create distinct attribution keys (helper below builds an event with a parameterized session):

```rust
fn session_event(session: u64, sequence: u64, effect: u64, body: RunEventBody) -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(sequence),
        id::<SessionTag>(session),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(4)),
        Some(id::<ModelRequestTag>(5)),
        None,
        Some(id::<EffectTag>(effect)),
        None,
        sequence,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Confidential,
        body,
    )
    .expect("event")
}

#[tokio::test]
async fn ledger_saturation_is_counted_and_diagnosed() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 1).expect("billing");
    billing
        .observe(Arc::from([
            session_event(1, 1, 7, completed(7, Some(usage(1, 1, Some(cost("USD", 1, "prices-v1")))))),
            session_event(2, 2, 8, completed(8, Some(usage(1, 1, Some(cost("USD", 1, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.spend.len(), 1);
    assert_eq!(snapshot.overflowed_events, 1);
    assert_eq!(
        billing.last_diagnostic().expect("diagnostic").code,
        "billing_ledger_saturated"
    );
    // The existing key keeps aggregating after saturation.
    billing
        .observe(Arc::from([session_event(
            1, 3, 9,
            completed(9, Some(usage(1, 1, Some(cost("USD", 4, "prices-v1"))))),
        )]))
        .await
        .expect("observe");
    let snapshot = billing.snapshot();
    assert_eq!(snapshot.spend[0].micros, 5);
}

#[tokio::test]
async fn drop_progress_overflow_is_diagnosed() {
    let billing =
        BillingObserver::try_new(1, ObserverBackpressure::DropProgress, 64).expect("billing");
    billing
        .observe(Arc::from([
            model_event(1, 7, completed(7, None)),
            model_event(2, 8, completed(8, None)),
        ]))
        .await
        .expect("observe");
    assert!(billing.dropped() >= 1);
    assert_eq!(billing.snapshot().dropped_events, billing.dropped());
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: FAIL — `billing_ledger_saturated` diagnostic never stored (the saturation test's diagnostic assertion fails; the overflow test may already pass from Task 1's plumbing).

- [ ] **Step 3: Implement the saturation diagnostic**

In `lib.rs`, add the public const:

```rust
/// Diagnostic stored when the ledger's entry bound rejects a new key.
pub const BILLING_LEDGER_SATURATED: ObserverDiagnostic = ObserverDiagnostic {
    code: "billing_ledger_saturated",
    detail: "billing ledger entry bound reached; new attribution keys dropped",
};
```

`settle` already returns `bool`. At each `settle` call site in `ingest`, store the diagnostic on `false`:

```rust
                    if !settle(
                        &mut state,
                        self.max_entries,
                        event.session_id(),
                        event.run_id(),
                        origin,
                        body.usage(),
                    ) && let Ok(mut slot) = self.diagnostic.lock()
                    {
                        *slot = Some(BILLING_LEDGER_SATURATED);
                    }
```

(If borrowing `self.diagnostic` inside the `state` lock trips clippy's significant-drop lint, set a local `saturated = !settle(...)` and store the diagnostic after the match, still inside `ingest`.)

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/observers/finstack-ai-observer-billing/src
git commit -m "feat: bound billing ledger with saturation diagnostics"
```

---

### Task 5: JSONL export with decimal-string money and canary test

**Files:**
- Modify: `extensions/observers/finstack-ai-observer-billing/src/lib.rs`
- Modify: `extensions/observers/finstack-ai-observer-billing/src/tests.rs`

**Interfaces:**
- Consumes: `snapshot()` from Task 2.
- Produces: `BillingObserver::export_jsonl(&self) -> String` — one JSON object per line: every `spend` row (`"kind":"spend"`), every `usage` row (`"kind":"usage"`), one trailing `"kind":"summary"` line. Every numeric value is a decimal string.

- [ ] **Step 1: Write the failing tests**

Append to `src/tests.rs`:

```rust
const CANARY: &str = "CANARY_SECRET_VALUE";

#[tokio::test]
async fn export_jsonl_renders_decimal_strings_and_no_payloads() {
    let billing =
        BillingObserver::try_new(64, ObserverBackpressure::DropProgress, 64).expect("billing");
    let request = format!(r#"{{"model":"demo-model-1","system":"{CANARY}"}}"#);
    billing
        .observe(Arc::from([
            model_event(1, 7, requested(7, &request)),
            model_event(2, 7, completed(7, Some(usage(10, 20, Some(cost("USD", 1_250_000, "prices-v1")))))),
        ]))
        .await
        .expect("observe");
    let text = billing.export_jsonl();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    let spend: serde_json::Value = serde_json::from_str(lines[0]).expect("spend json");
    assert_eq!(spend["kind"], "spend");
    assert_eq!(spend["micros"], "1250000");
    assert_eq!(spend["unit"], "USD");
    assert_eq!(spend["pricing_policy_version"], "prices-v1");
    assert_eq!(spend["model"], "demo-model-1");
    assert!(spend["micros"].is_string());
    let usage_line: serde_json::Value = serde_json::from_str(lines[1]).expect("usage json");
    assert_eq!(usage_line["kind"], "usage");
    assert_eq!(usage_line["input_tokens"], "10");
    let summary: serde_json::Value = serde_json::from_str(lines[2]).expect("summary json");
    assert_eq!(summary["kind"], "summary");
    assert_eq!(summary["unattributed_effects"], "0");
    assert!(!text.contains(CANARY));
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: compile FAILURE — `export_jsonl` not defined.

- [ ] **Step 3: Implement `export_jsonl`**

Add to `impl BillingObserver` (uses `serde_json::json!` and `serde_json::Map` already available via the dep; render `ComponentRef` as `id@major.minor.patch`, omitting `@...` when the version is absent — check `ComponentRef`'s accessors as in Task 3):

```rust
    /// Export the ledger as JSONL. All numerics are decimal strings; content
    /// payloads are never included.
    #[must_use]
    pub fn export_jsonl(&self) -> String {
        let snapshot = self.snapshot();
        let mut out = String::new();
        for row in &snapshot.spend {
            let line = serde_json::json!({
                "kind": "spend",
                "session_id": row.session_id.to_string(),
                "run_id": row.run_id.to_string(),
                "model": row.model.as_deref(),
                "provider": row.provider.as_ref().map(render_component),
                "unit": row.unit.as_ref(),
                "pricing_policy_version": row.pricing_policy_version.as_ref(),
                "micros": row.micros.to_string(),
                "costed_effects": row.costed_effects.to_string(),
            });
            push_line(&mut out, &line);
        }
        for row in &snapshot.usage {
            let line = serde_json::json!({
                "kind": "usage",
                "session_id": row.session_id.to_string(),
                "run_id": row.run_id.to_string(),
                "model": row.model.as_deref(),
                "input_tokens": row.input_tokens.to_string(),
                "output_tokens": row.output_tokens.to_string(),
                "effects": row.effects.to_string(),
                "uncosted_effects": row.uncosted_effects.to_string(),
            });
            push_line(&mut out, &line);
        }
        let summary = serde_json::json!({
            "kind": "summary",
            "unattributed_effects": snapshot.unattributed_effects.to_string(),
            "overflowed_events": snapshot.overflowed_events.to_string(),
            "dropped_events": snapshot.dropped_events.to_string(),
        });
        push_line(&mut out, &summary);
        out
    }
```

with free helpers:

```rust
fn render_component(component: &ComponentRef) -> String {
    // Adjust accessor names to the real ComponentRef API (see Task 3 note).
    match component.version.as_ref() {
        Some(version) => format!(
            "{}@{}.{}.{}",
            component.id, version.major, version.minor, version.patch
        ),
        None => component.id.to_string(),
    }
}

fn push_line(out: &mut String, value: &serde_json::Value) {
    if let Ok(text) = serde_json::to_string(value) {
        out.push_str(&text);
        out.push('\n');
    }
}
```

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/observers/finstack-ai-observer-billing/src
git commit -m "feat: JSONL spend export with decimal-string money"
```

---

### Task 6: README, public-api baseline, workspace verification

**Files:**
- Modify: `extensions/observers/finstack-ai-observer-billing/README.md`
- Create (generated): `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-observer-billing.txt`

**Interfaces:**
- Consumes: the finished public API from Tasks 1–5.
- Produces: docs and baselines only; no code changes.

- [ ] **Step 1: Write the README**

Follow the metrics leaf's README shape (title, one-paragraph purpose, runnable example, caveats). Content:

````markdown
# finstack-ai-observer-billing

Read-only spend-ledger observer. Aggregates the kernel's typed `CostAmount`
(integer micros, unit, `pricing_policy_version`) from settled effects into a
ledger keyed by session, run, and model. Integer money only — no floats, no
pricing tables, no cross-unit totals.

This ledger is a best-effort **projection** for dashboards and operators.
Observers may drop events under backpressure; authoritative billing must
re-derive spend from journal records (`EffectCompleted.usage` and its
`usage_digest`). Cost appears only when the model provider emits
`Usage.cost`; effects without a cost are reported as `uncosted_effects`,
never priced locally.

```rust
use std::sync::Arc;

use finstack_ai_observer_billing::BillingObserver;
use finstack_ai_runtime::ObserverBackpressure;

let billing = BillingObserver::try_new(1024, ObserverBackpressure::DropProgress, 10_000)
    .expect("billing observer");
// Register with AgentBuilder::observer(component_ref, Arc::new(billing)), then:
// let snapshot = billing.snapshot();
// let jsonl = billing.export_jsonl();
let _ = Arc::new(billing);
```

Diagnostics: `dropped()` counts queue drops; `last_diagnostic()` surfaces
`observer_queue_overflow` and `billing_ledger_saturated` (entry bound hit;
new attribution keys are dropped, existing keys keep aggregating).
````

- [ ] **Step 2: Regenerate public-api baselines**

```bash
uv run --no-project python scripts/compat/public_items.py --write
```

Expected: a new `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-observer-billing.txt` appears (the generator auto-globs `extensions/*/**/Cargo.toml`); no other baseline changes. Then verify:

```bash
uv run --no-project python scripts/compat/public_items.py --check
```

Expected: exit 0.

- [ ] **Step 3: Workspace checks**

```bash
cargo clippy -p finstack-ai-observer-billing --all-targets --locked -- -D warnings
```

Expected: exit 0, no warnings. Then confirm no forbidden graph edges (leaf depends on kernel/runtime contracts only):

```bash
cargo tree -p finstack-ai-observer-billing --locked -e normal --depth 1
```

Expected: direct deps are exactly `finstack-ai-kernel`, `finstack-ai-runtime`, `serde_json`, `thiserror`.

- [ ] **Step 4: Full test pass**

```bash
cargo test -p finstack-ai-observer-billing --locked
```

Expected: PASS. Check the exit code, not the summary text.

- [ ] **Step 5: Commit**

```bash
git add extensions/observers/finstack-ai-observer-billing/README.md fixtures/compatibility/public-rust-api
git commit -m "docs: billing observer README and public-api baseline"
```

---

## Self-review notes

- Spec §2 money rules → Task 2 (`u128` saturating, cells keyed by unit+policy, uncosted counting) and Task 5 (decimal strings). No float appears in any task.
- Spec §3.1 attribution → Task 3 (pending map, bounded, `EffectFailed` usage aggregation, deferred/cancelled cleanup, `unattributed_effects`).
- Spec §3.2 bounds → Task 4 (`max_entries`, `overflowed_events`, `BILLING_LEDGER_SATURATED`, no eviction).
- Spec §3.3 contract → Task 1 (descriptor `MetadataOnly`, queue/diagnostics parity with the metrics leaf), Task 5 canary test, Task 6 README best-effort disclaimer. No Prometheus surface anywhere, by design.
- Spec §4 API → Tasks 1/2/4/5 produce exactly the listed items; nothing extra is `pub`.
- Known executor adjustment points (accessor names on `ComponentRef`, `EffectRequested`, and the presence of `RunEventKind::EffectCancelled`) are called out inline with the grep to run — behavior is fixed, only call syntax may shift.
