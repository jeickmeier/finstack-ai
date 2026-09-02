//! Reference Prometheus text metrics observer. Labels are identifiers only.

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
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ComponentId, ComponentRef, Metadata, RunEvent, RunEventBody, RunEventKind, Version,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::middleware::{
    CompactionEvidence, CompactionResult, PromptCacheImpact,
};
use finstack_ai_runtime::ports::observer::{
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode,
};
use thiserror::Error;

const LATENCY_BUCKETS: [f64; 9] = [0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0];

/// Metrics-observer construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MetricsObserverError {
    /// Configuration is malformed.
    #[error("metrics_observer_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

#[derive(Debug, Default)]
struct Histogram {
    counts: [u64; 9],
    inf: u64,
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, seconds: f64) {
        self.sum += seconds;
        self.count += 1;
        if let Some(index) = LATENCY_BUCKETS.iter().position(|bound| seconds <= *bound) {
            self.counts[index] += 1;
        } else {
            self.inf += 1;
        }
    }
}

#[derive(Debug, Default)]
struct MetricsState {
    status: BTreeMap<&'static str, u64>,
    hub_queue_depth: u64,
    effect_starts: BTreeMap<String, i64>,
    effect_latency: Histogram,
    store_latency: Histogram,
    usage_input: u64,
    usage_output: u64,
    recovery: BTreeMap<&'static str, u64>,
    compaction_trigger: BTreeMap<String, u64>,
    compaction_tokens_before: u64,
    compaction_tokens_after: u64,
    compaction_checkpoint: BTreeMap<&'static str, u64>,
    compaction_summary_latency: Histogram,
    compaction_failure: u64,
    compaction_cache_impact: BTreeMap<&'static str, u64>,
}

/// Prometheus text exposition observer. Default payload mode is metadata-only.
pub struct MetricsObserver {
    descriptor: ObserverDescriptor,
    state: Mutex<MetricsState>,
}

impl MetricsObserver {
    /// Construct a metadata-only metrics observer.
    ///
    /// # Errors
    ///
    /// Rejects an invalid observer identity.
    pub fn try_new() -> Result<Self, MetricsObserverError> {
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.observer.metrics").map_err(|_| {
                        MetricsObserverError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    Some(Version {
                        major: 0,
                        minor: 0,
                        patch: 4,
                    }),
                ),
                payload_mode: ObserverPayloadMode::MetadataOnly,
                metadata: Metadata::empty(),
            },
            state: Mutex::new(MetricsState::default()),
        })
    }

    /// Record store latency without accepting store-private payloads.
    pub fn record_store_latency(&self, seconds: f64) {
        if let Ok(mut state) = self.state.lock() {
            state.store_latency.observe(seconds);
        }
    }

    /// Ingest compaction evidence only. Replacement and summary text are ignored.
    pub fn record_compaction(&self, evidence: &CompactionEvidence) {
        if let Ok(mut state) = self.state.lock() {
            let strategy = evidence.strategy_id.to_string();
            *state.compaction_trigger.entry(strategy).or_insert(0) += 1;
            state.compaction_tokens_before += evidence.estimated_tokens_before;
            state.compaction_tokens_after += evidence.estimated_tokens_after;
            let checkpoint = if evidence.summary_digest.is_some() {
                "hit"
            } else {
                "miss"
            };
            *state.compaction_checkpoint.entry(checkpoint).or_insert(0) += 1;
            *state
                .compaction_cache_impact
                .entry(cache_impact_label(evidence.cache_impact))
                .or_insert(0) += 1;
        }
    }

    /// Ingest a compaction result while ignoring message and summary payloads.
    pub fn record_compaction_result(&self, result: &CompactionResult) {
        self.record_compaction(&result.evidence);
    }

    /// Record a compaction failure/fallback without content.
    pub fn record_compaction_failure(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.compaction_failure += 1;
        }
    }

    /// Record summary-model latency in seconds.
    pub fn record_compaction_summary_latency(&self, seconds: f64) {
        if let Ok(mut state) = self.state.lock() {
            state.compaction_summary_latency.observe(seconds);
        }
    }

    /// Encode the current series as Prometheus text.
    #[must_use]
    pub fn encode_prometheus(&self) -> String {
        let Ok(state) = self.state.lock() else {
            return String::new();
        };
        let mut out = String::new();
        emit_runtime_series(&mut out, &state);
        emit_compaction_series(&mut out, &state);
        out
    }

    fn ingest(&self, event: &RunEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match event.kind() {
            RunEventKind::RunAccepted => {
                state.status.clear();
                state.status.insert("running", 1);
            }
            RunEventKind::RunSuspended => {
                state.status.clear();
                state.status.insert("suspended", 1);
            }
            RunEventKind::RunCompleted | RunEventKind::RunFailed | RunEventKind::RunCancelled => {
                state.status.clear();
                state.status.insert("idle", 1);
            }
            RunEventKind::EffectRequested => {
                if let Some(effect_id) = event.effect_id() {
                    state
                        .effect_starts
                        .insert(effect_id.to_string(), event.timestamp().as_unix_ms());
                }
            }
            RunEventKind::EffectCompleted => {
                if let Some(effect_id) = event.effect_id()
                    && let Some(start) = state.effect_starts.remove(&effect_id.to_string())
                {
                    let delta_ms = event.timestamp().as_unix_ms().saturating_sub(start);
                    let millis = u32::try_from(delta_ms.max(0)).unwrap_or(u32::MAX);
                    state.effect_latency.observe(f64::from(millis) / 1000.0);
                }
                if let RunEventBody::EffectCompleted(completed) = event.body()
                    && let Some(usage) = completed.usage()
                {
                    state.usage_input += usage.input_tokens().unwrap_or(0);
                    state.usage_output += usage.output_tokens().unwrap_or(0);
                }
            }
            RunEventKind::EffectFailed => {
                *state.recovery.entry("fail").or_insert(0) += 1;
            }
            RunEventKind::EffectDeferred => {
                *state.recovery.entry("uncertain").or_insert(0) += 1;
            }
            RunEventKind::QueueDepthWarning => {
                if let RunEventBody::QueueDepthWarning(warning) = event.body() {
                    state.hub_queue_depth = u64::from(warning.depth);
                }
            }
            _ => {}
        }
    }
}

impl Observer for MetricsObserver {
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

fn emit_runtime_series(out: &mut String, state: &MetricsState) {
    emit_labeled(
        out,
        "finstack_runtime_status",
        "Runtime lifecycle gauge",
        "gauge",
        "state",
        &state.status,
    );
    out.push_str("# HELP finstack_runtime_queue_depth Event hub queue depth\n");
    out.push_str("# TYPE finstack_runtime_queue_depth gauge\n");
    let _ = writeln!(
        out,
        "finstack_runtime_queue_depth{{queue=\"hub\"}} {}",
        state.hub_queue_depth
    );
    emit_histogram(
        out,
        "finstack_effect_latency_seconds",
        "Effect latency",
        &state.effect_latency,
    );
    emit_histogram(
        out,
        "finstack_store_latency_seconds",
        "Store latency",
        &state.store_latency,
    );
    out.push_str("# HELP finstack_usage_tokens Token usage\n");
    out.push_str("# TYPE finstack_usage_tokens counter\n");
    let _ = writeln!(
        out,
        "finstack_usage_tokens{{direction=\"input\"}} {}",
        state.usage_input
    );
    let _ = writeln!(
        out,
        "finstack_usage_tokens{{direction=\"output\"}} {}",
        state.usage_output
    );
    emit_labeled(
        out,
        "finstack_recovery_total",
        "Recovery outcomes",
        "counter",
        "class",
        &state.recovery,
    );
}

fn emit_compaction_series(out: &mut String, state: &MetricsState) {
    emit_labeled(
        out,
        "finstack_compaction_trigger_total",
        "Compaction triggers",
        "counter",
        "strategy_id",
        &state.compaction_trigger,
    );
    out.push_str("# HELP finstack_compaction_tokens Compaction token estimates\n");
    out.push_str("# TYPE finstack_compaction_tokens counter\n");
    let _ = writeln!(
        out,
        "finstack_compaction_tokens{{bound=\"before\"}} {}",
        state.compaction_tokens_before
    );
    let _ = writeln!(
        out,
        "finstack_compaction_tokens{{bound=\"after\"}} {}",
        state.compaction_tokens_after
    );
    emit_labeled(
        out,
        "finstack_compaction_checkpoint_total",
        "Compaction checkpoint results",
        "counter",
        "result",
        &state.compaction_checkpoint,
    );
    emit_histogram(
        out,
        "finstack_compaction_summary_latency_seconds",
        "Compaction summary latency",
        &state.compaction_summary_latency,
    );
    out.push_str("# HELP finstack_compaction_failure_total Compaction failures\n");
    out.push_str("# TYPE finstack_compaction_failure_total counter\n");
    let _ = writeln!(
        out,
        "finstack_compaction_failure_total {}",
        state.compaction_failure
    );
    emit_labeled(
        out,
        "finstack_compaction_cache_impact_total",
        "Prompt-cache impact",
        "counter",
        "impact",
        &state.compaction_cache_impact,
    );
}

fn cache_impact_label(impact: PromptCacheImpact) -> &'static str {
    match impact {
        PromptCacheImpact::StablePrefixPreserved => "stable_prefix_preserved",
        PromptCacheImpact::MutableSuffixChanged => "mutable_suffix_changed",
        PromptCacheImpact::CacheInvalidated => "cache_invalidated",
    }
}

/// One labeled series: `# HELP`/`# TYPE` header, then one sample per
/// entry with the label value escaped for the text exposition format.
fn emit_labeled<K: AsRef<str>>(
    out: &mut String,
    name: &str,
    help: &str,
    metric_type: &str,
    label: &str,
    values: &BTreeMap<K, u64>,
) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {metric_type}");
    for (value, count) in values {
        let escaped = escape_label(value.as_ref());
        let _ = writeln!(out, "{name}{{{label}=\"{escaped}\"}} {count}");
    }
}

fn emit_histogram(out: &mut String, name: &str, help: &str, histogram: &Histogram) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} histogram");
    let mut cumulative = 0_u64;
    for (bound, count) in LATENCY_BUCKETS.iter().zip(histogram.counts) {
        cumulative += count;
        let _ = writeln!(out, "{name}_bucket{{le=\"{bound}\"}} {cumulative}");
    }
    let inf = cumulative + histogram.inf;
    let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {inf}");
    let _ = writeln!(out, "{name}_sum {}", histogram.sum);
    let _ = writeln!(out, "{name}_count {}", histogram.count);
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('\n', r"\n")
        .replace('"', r#"\""#)
}

#[cfg(test)]
mod tests;
