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
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ComponentId, ComponentRef, EffectId, EffectInput, EffectKind, EffectOutputKind, Metadata,
    RunEvent, RunEventBody, RunEventKind, RunId, SessionId, Usage, Version, label_is_valid,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::observer::{
    Observer, ObserverDescriptor, ObserverDiagnostic, ObserverError, ObserverPayloadMode,
};
use thiserror::Error;

/// Diagnostic stored when the ledger's entry bound rejects a new key.
pub const BILLING_LEDGER_SATURATED: ObserverDiagnostic = ObserverDiagnostic {
    code: "billing_ledger_saturated",
    detail: "billing ledger entry bound reached; new attribution keys dropped",
};

/// Diagnostic stored when the pending-attribution bound evicts the oldest
/// tracked model effect to make room for a new one.
pub const BILLING_PENDING_SATURATED: ObserverDiagnostic = ObserverDiagnostic {
    code: "billing_pending_saturated",
    detail: "billing pending-attribution bound reached; oldest pending origins are evicted",
};

/// Diagnostic stored when a model request yields no usable model name.
pub const BILLING_MODEL_NAME_INVALID: ObserverDiagnostic = ObserverDiagnostic {
    code: "billing_model_name_invalid",
    detail: "model request had no usable top-level model name; spend will aggregate under model none",
};

/// Maximum distinct attribution keys accepted by the ledger.
const MAX_ENTRIES_CEILING: usize = 1_000_000;
/// Pending model effects tracked for attribution.
const MAX_PENDING_EFFECTS: usize = 4096;

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
    /// Provider component, when known. This is the FIRST provider seen for
    /// this attribution key: spend from the same (session, run, model) later
    /// served by a different provider is still attributed to the first
    /// provider observed.
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
    /// Settled effects observed for this key. Includes non-model (tool,
    /// context) effects that settle with usage; those always key on
    /// `model: None` since only model requests are attribution-tracked.
    pub effects: u64,
    /// Settled effects that carried no cost amount. Includes non-model
    /// (tool, context) effects that settle with usage but no cost.
    pub uncosted_effects: u64,
}

/// Point-in-time projection of the ledger. Deterministic ordering.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LedgerSnapshot {
    /// Spend rows ordered by (session, run, model, unit, policy version).
    pub spend: Vec<SpendEntry>,
    /// Usage rows ordered by (session, run, model).
    pub usage: Vec<UsageEntry>,
    /// Model effects whose request origin was not tracked when they
    /// settled. Non-model (tool, context) effects never contribute here;
    /// they always settle under `model: None` by design.
    pub unattributed_effects: u64,
    /// Events ignored because the ledger reached its entry bound.
    pub overflowed_events: u64,
}

/// Read-only spend-ledger observer. Default payload mode is metadata-only.
pub struct BillingObserver {
    descriptor: ObserverDescriptor,
    state: Mutex<LedgerState>,
    max_entries: usize,
    diagnostic: Mutex<Option<ObserverDiagnostic>>,
}

impl BillingObserver {
    /// Construct a billing observer.
    ///
    /// # Arguments
    ///
    /// * `max_entries` - Distinct (session, run, model) keys retained (`1..=1_000_000`).
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity or entry bound.
    pub fn try_new(max_entries: usize) -> Result<Self, BillingObserverError> {
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
            state: Mutex::new(LedgerState::default()),
            max_entries,
            diagnostic: Mutex::new(None),
        })
    }

    /// Last overflow or saturation diagnostic.
    ///
    /// # Errors
    ///
    /// Returns [`ObserverError::Unavailable`] when the diagnostic lock is
    /// poisoned. `None` would be indistinguishable from no recorded
    /// diagnostic, so the failure is reported rather than swallowed.
    pub fn last_diagnostic(&self) -> Result<Option<ObserverDiagnostic>, ObserverError> {
        self.diagnostic
            .lock()
            .map(|slot| *slot)
            .map_err(|_| ObserverError::Unavailable)
    }

    /// Snapshot the ledger.
    ///
    /// The ledger assumes at-most-once settlement delivery: a duplicate
    /// `EffectCompleted` (or `EffectFailed`) for the same effect id is folded
    /// in again and double-counts usage, cost, and effect tallies.
    ///
    /// # Errors
    ///
    /// Returns [`ObserverError::Unavailable`] when the ledger lock is
    /// poisoned. An empty snapshot would be indistinguishable from no
    /// recorded spend, so the failure is reported rather than swallowed.
    pub fn snapshot(&self) -> Result<LedgerSnapshot, ObserverError> {
        let Ok(state) = self.state.lock() else {
            return Err(ObserverError::Unavailable);
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
        Ok(LedgerSnapshot {
            spend,
            usage,
            unattributed_effects: state.unattributed_effects,
            overflowed_events: state.overflowed_events,
        })
    }

    /// Export the ledger as JSONL. All numerics are decimal strings; content
    /// payloads are never included.
    ///
    /// # Errors
    ///
    /// Returns [`ObserverError::Unavailable`] when the ledger lock is
    /// poisoned, matching [`Self::snapshot`].
    pub fn export_jsonl(&self) -> Result<String, ObserverError> {
        let snapshot = self.snapshot()?;
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
        });
        push_line(&mut out, &summary);
        Ok(out)
    }

    fn ingest(&self, event: &RunEvent) {
        // Compute the EffectRequested origin (provider + model name) before
        // taking the state lock; the parse work does not need the lock held.
        let mut model_name_invalid = false;
        let requested_origin =
            if let (RunEventKind::EffectRequested, RunEventBody::EffectRequested(body)) =
                (event.kind(), event.body())
                && body.kind() == EffectKind::Model
            {
                let provider = body.component().map(|invocation| {
                    ComponentRef::new(invocation.component.clone(), Some(invocation.version))
                });
                let model = match body.input() {
                    EffectInput::Model { request } => parse_model_name(request.as_str()),
                    _ => None,
                };
                if model.is_none() {
                    model_name_invalid = true;
                }
                Some((body.effect_id(), EffectOrigin { provider, model }))
            } else {
                None
            };

        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let mut saturated = false;
        let mut pending_saturated = false;
        match event.kind() {
            RunEventKind::EffectCompleted => {
                if let RunEventBody::EffectCompleted(body) = event.body() {
                    saturated = !settle_effect(
                        &mut state,
                        self.max_entries,
                        event.session_id(),
                        event.run_id(),
                        body.effect_id(),
                        body.output_contract().kind == EffectOutputKind::ModelResponse,
                        body.usage(),
                    );
                }
            }
            RunEventKind::EffectRequested => {
                if let Some((effect_id, origin)) = requested_origin {
                    if state.pending.len() >= MAX_PENDING_EFFECTS {
                        pending_saturated = true;
                        state.pending.pop_first();
                    }
                    state.pending.insert(effect_id, origin);
                }
            }
            RunEventKind::EffectFailed => {
                if let RunEventBody::EffectFailed(body) = event.body() {
                    saturated = !settle_effect(
                        &mut state,
                        self.max_entries,
                        event.session_id(),
                        event.run_id(),
                        body.effect_id(),
                        body.output_contract().kind == EffectOutputKind::ModelResponse,
                        body.usage(),
                    );
                }
            }
            RunEventKind::EffectCancelled => {
                if let Some(effect_id) = event.effect_id() {
                    state.pending.remove(&effect_id);
                }
            }
            // `EffectDeferred` intentionally falls through here: deferral
            // then completion under the same EffectId is the kernel's
            // intended lifecycle, so the pending origin must survive it.
            _ => {}
        }
        drop(state);
        if saturated && let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(BILLING_LEDGER_SATURATED);
        }
        if pending_saturated && let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(BILLING_PENDING_SATURATED);
        }
        if model_name_invalid && let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(BILLING_MODEL_NAME_INVALID);
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
        }
        Box::pin(async { Ok(()) })
    }
}

/// Remove any pending origin tracked for `effect_id` and fold the settled
/// effect into the ledger. Bumps `unattributed_effects` only when `is_model`
/// is true and no pending origin was found — non-model (tool, context)
/// effects are never attribution-tracked, so their settlement without a
/// pending origin is expected, not a miss.
fn settle_effect(
    state: &mut LedgerState,
    max_entries: usize,
    session_id: SessionId,
    run_id: RunId,
    effect_id: EffectId,
    is_model: bool,
    usage: Option<&Usage>,
) -> bool {
    let origin = state.pending.remove(&effect_id);
    if origin.is_none() && is_model {
        state.unattributed_effects = state.unattributed_effects.saturating_add(1);
    }
    settle(state, max_entries, session_id, run_id, origin, usage)
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
                .entry((
                    Arc::from(cost.unit()),
                    Arc::from(cost.pricing_policy_version()),
                ))
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

/// Render a component reference as `id@major.minor.patch`, or bare `id` when
/// the version is absent.
fn render_component(component: &ComponentRef) -> String {
    match component.version() {
        Some(version) => format!(
            "{}@{}.{}.{}",
            component.id(),
            version.major,
            version.minor,
            version.patch
        ),
        None => component.id().to_string(),
    }
}

/// Append one JSON value as a line to `out`. The `Result` is deliberately
/// discarded: every row here is a map of strings and decimal strings, which
/// cannot realistically fail to serialize.
fn push_line(out: &mut String, value: &serde_json::Value) {
    if let Ok(text) = serde_json::to_string(value) {
        out.push_str(&text);
        out.push('\n');
    }
}

/// Extract a bounded top-level `"model"` string from canonical request JSON.
fn parse_model_name(request: &str) -> Option<Arc<str>> {
    let value: serde_json::Value = serde_json::from_str(request).ok()?;
    let name = value.get("model")?.as_str()?;
    if !label_is_valid(name) {
        return None;
    }
    Some(Arc::from(name))
}

#[cfg(test)]
mod tests;
