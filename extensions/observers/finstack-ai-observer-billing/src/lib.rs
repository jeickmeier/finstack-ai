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

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ComponentId, ComponentRef, EffectId, EffectInput, EffectKind, Metadata, RunEvent, RunEventBody,
    RunEventKind, RunId, SessionId, Usage, Version,
};
use finstack_ai_runtime::{
    OBSERVER_QUEUE_OVERFLOW, Observer, ObserverBackpressure, ObserverDescriptor,
    ObserverDiagnostic, ObserverError, ObserverPayloadMode, ObserverQueue, ObserverQueuePush,
    PortFuture,
};
use thiserror::Error;

/// Maximum distinct attribution keys accepted by the ledger.
const MAX_ENTRIES_CEILING: usize = 1_000_000;
/// Pending model effects tracked for attribution.
const MAX_PENDING_EFFECTS: usize = 4096;
/// Longest model-name string accepted from a request payload.
const MAX_MODEL_NAME_BYTES: usize = 256;

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
            RunEventKind::EffectRequested => {
                if let RunEventBody::EffectRequested(body) = event.body()
                    && body.kind() == EffectKind::Model
                {
                    if state.pending.len() >= MAX_PENDING_EFFECTS {
                        return;
                    }
                    let provider = body.component().map(|invocation| {
                        ComponentRef::new(invocation.component.clone(), Some(invocation.version))
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
            _ => {}
        }
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

/// Extract a bounded top-level `"model"` string from canonical request JSON.
fn parse_model_name(request: &str) -> Option<Arc<str>> {
    let value: serde_json::Value = serde_json::from_str(request).ok()?;
    let name = value.get("model")?.as_str()?;
    if name.is_empty() || name.len() > MAX_MODEL_NAME_BYTES {
        return None;
    }
    Some(Arc::from(name))
}

#[cfg(test)]
mod tests;
