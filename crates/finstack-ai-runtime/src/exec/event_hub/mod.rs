//! Bounded event delivery contracts and the native per-run event hub.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{RunEvent, RunEventClass, RunEventKind, Sensitivity};
use thiserror::Error;

/// Per-run source bounds for the native event hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventHubConfig {
    /// Maximum event publications waiting for hub fan-out.
    pub source_capacity: usize,
    /// Maximum simultaneously registered subscriptions.
    pub max_subscribers: usize,
}

impl EventHubConfig {
    /// Validate non-zero hub bounds.
    ///
    /// # Errors
    ///
    /// Returns [`EventSubscriptionError::InvalidConfiguration`] for a zero bound.
    pub fn validate(self) -> Result<Self, EventSubscriptionError> {
        if self.source_capacity == 0 || self.max_subscribers == 0 {
            return Err(EventSubscriptionError::InvalidConfiguration);
        }
        Ok(self)
    }
}

/// Transport batching thresholds for one subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventBatchConfig {
    /// Flush after this many events.
    pub flush_count: usize,
    /// Flush when serialized event bytes reach this threshold.
    ///
    /// One valid event larger than this threshold is delivered alone.
    pub flush_bytes: usize,
    /// Flush after this much operational time from the first pending event.
    pub flush_interval: Duration,
}

/// Immutable event selector for one subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFilter {
    /// Include durable-derived events.
    pub include_durable: bool,
    /// Include transient progress events.
    pub include_transient: bool,
    /// Included kinds; empty means all kinds in the selected classes.
    pub kinds: Arc<[RunEventKind]>,
    /// Highest sensitivity delivered to this subscription.
    pub max_sensitivity: Sensitivity,
}

impl EventFilter {
    fn validate(&self) -> Result<(), EventSubscriptionError> {
        if !self.include_durable && !self.include_transient {
            return Err(EventSubscriptionError::InvalidConfiguration);
        }
        for (index, kind) in self.kinds.iter().enumerate() {
            if self.kinds[..index].contains(kind) {
                return Err(EventSubscriptionError::InvalidConfiguration);
            }
        }
        Ok(())
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    fn matches(&self, event: &RunEvent) -> bool {
        let class_allowed = match event.class() {
            RunEventClass::DurableDerived => self.include_durable,
            RunEventClass::Transient => self.include_transient,
        };
        class_allowed
            && (self.kinds.is_empty() || self.kinds.contains(&event.kind()))
            && crate::ports::middleware::sensitivity_rank(event.sensitivity())
                <= crate::ports::middleware::sensitivity_rank(self.max_sensitivity)
    }
}

/// Whether fine-grained progress waits for transport batching thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressCoalescing {
    /// Flush each transient progress event without waiting for more progress.
    Disabled,
    /// Accumulate intact events until a count, byte, time, or durable boundary.
    Enabled,
}

/// Who registered the subscription. Observer delivery never waits.
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriberAudience {
    /// Frontend or SDK consumer; honors [`EventLagPolicy`].
    Interactive,
    /// Isolated observer; try-only, drop progress, fail closed on durable loss.
    Observer,
}

/// Behavior when a subscription cannot keep up with event delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLagPolicy {
    /// Backpressure all selected events for at most the configured timeout.
    BlockBounded {
        /// Maximum operational wait for queue capacity.
        timeout: Duration,
    },
    /// Drop transient progress, but protect durable events with a bounded wait.
    DropProgress {
        /// Maximum operational wait for a durable event.
        durable_timeout: Duration,
    },
    /// Disconnect immediately when the bounded queue is full.
    Disconnect,
}

/// Complete operational configuration for one event subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSubscriptionConfig {
    /// Bounded capacity of both the subscription inbox and delivered-batch queue.
    pub queue_capacity: usize,
    /// Immutable event filter.
    pub filter: EventFilter,
    /// Transport batching thresholds.
    pub batching: EventBatchConfig,
    /// Progress coalescing mode.
    pub progress_coalescing: ProgressCoalescing,
    /// Slow-consumer behavior.
    pub lag_policy: EventLagPolicy,
}

impl EventSubscriptionConfig {
    /// Validate all queue, batch, filter, and timeout bounds.
    ///
    /// # Errors
    ///
    /// Returns [`EventSubscriptionError::InvalidConfiguration`] when a bound is zero
    /// or the filter selects neither event class or repeats an event kind.
    pub fn validate(&self) -> Result<(), EventSubscriptionError> {
        if self.queue_capacity == 0
            || self.batching.flush_count == 0
            || self.batching.flush_bytes == 0
            || self.batching.flush_interval.is_zero()
        {
            return Err(EventSubscriptionError::InvalidConfiguration);
        }
        match self.lag_policy {
            EventLagPolicy::BlockBounded { timeout } if timeout.is_zero() => {
                return Err(EventSubscriptionError::InvalidConfiguration);
            }
            EventLagPolicy::DropProgress { durable_timeout } if durable_timeout.is_zero() => {
                return Err(EventSubscriptionError::InvalidConfiguration);
            }
            _ => {}
        }
        self.filter.validate()
    }
}

/// One transport batch of logically ordered runtime events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBatch {
    events: Arc<[RunEvent]>,
    first_sequence: u64,
    last_sequence: u64,
    dropped_progress: u64,
}

impl EventBatch {
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn new(events: Vec<RunEvent>, dropped_progress: u64) -> Option<Self> {
        let first_sequence = events.first()?.transient_sequence();
        let last_sequence = events.last()?.transient_sequence();
        Some(Self {
            events: events.into(),
            first_sequence,
            last_sequence,
            dropped_progress,
        })
    }

    /// Ordered events in this transport batch.
    #[must_use]
    pub fn events(&self) -> &[RunEvent] {
        &self.events
    }

    /// First contained run-stream sequence.
    #[must_use]
    pub const fn first_sequence(&self) -> u64 {
        self.first_sequence
    }

    /// Last contained run-stream sequence.
    #[must_use]
    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    /// Lag-dropped transient events since the preceding delivered batch.
    #[must_use]
    pub const fn dropped_progress(&self) -> u64 {
        self.dropped_progress
    }
}

/// Cumulative delivery statistics for one subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventDeliveryStats {
    /// Delivered transport batches.
    pub delivered_batches: u64,
    /// Delivered logical events.
    pub delivered_events: u64,
    /// Transient progress dropped because of lag.
    pub dropped_progress: u64,
    /// Last delivered run-stream sequence.
    pub last_delivered_sequence: Option<u64>,
}

/// Explicit reason an event subscription stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSubscriptionCloseReason {
    /// The owning run event hub closed normally.
    HubClosed,
    /// The subscriber explicitly closed the subscription.
    SubscriberClosed,
    /// The delivered-batch receiver was dropped.
    ReceiverDropped,
    /// The subscription exceeded its configured lag policy on transient work.
    Lagged,
    /// A durable event could not be delivered within the configured policy.
    MissedDurable,
}

/// Current lifecycle and delivery statistics for a subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventSubscriptionStatus {
    /// Close reason, or `None` while active.
    pub close_reason: Option<EventSubscriptionCloseReason>,
    /// Current cumulative statistics.
    pub stats: EventDeliveryStats,
}

/// Event subscription construction and lifecycle failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EventSubscriptionError {
    /// A capacity, threshold, timeout, or filter is invalid.
    #[error("invalid event subscription configuration")]
    InvalidConfiguration,
    /// The per-run subscriber bound is exhausted.
    #[error("event subscriber capacity exhausted")]
    CapacityExhausted,
    /// The owning event hub is closed.
    #[error("event hub is closed")]
    HubClosed,
}

#[cfg(feature = "native-tokio")]
mod batching;
#[cfg(feature = "native-tokio")]
mod dispatch;
#[cfg(feature = "native-tokio")]
mod handle;
#[cfg(feature = "native-tokio")]
mod subscription;
#[cfg(feature = "native-tokio")]
mod task;

#[cfg(all(test, feature = "native-tokio"))]
mod tests;

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventPublishError {
    pub(crate) code: &'static str,
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
fn validate_event_sequences(
    sequences: impl IntoIterator<Item = u64>,
    next_sequence: &mut Option<u64>,
) -> Result<(), EventPublishError> {
    let mut sequences = sequences.into_iter();
    let Some(first) = sequences.next() else {
        return Ok(());
    };
    let mut expected = next_sequence.unwrap_or(first);
    for sequence in std::iter::once(first).chain(sequences) {
        if sequence != expected {
            return Err(EventPublishError {
                code: "event_sequence_mismatch",
            });
        }
        expected = expected.checked_add(1).ok_or(EventPublishError {
            code: "event_sequence_exhausted",
        })?;
    }
    *next_sequence = Some(expected);
    Ok(())
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
fn json_byte_len(event: &RunEvent) -> Result<usize, EventPublishError> {
    let mut writer = crate::ports::model::CountingWriter::default();
    serde_json::to_writer(&mut writer, event).map_err(|_| EventPublishError {
        code: "event_serialization_failed",
    })?;
    Ok(writer.len)
}

#[cfg(all(test, any(feature = "native-tokio", feature = "wasm-host")))]
mod sequence_tests;

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) trait RuntimeEventPublisher: crate::ports::PortObject {
    fn publish(
        &self,
        events: Arc<[RunEvent]>,
    ) -> crate::ports::PortFuture<Result<(), EventPublishError>>;
}

#[cfg(feature = "native-tokio")]
pub use subscription::EventSubscription;

#[cfg(feature = "native-tokio")]
pub(crate) use handle::EventHubHandle;
#[cfg(feature = "native-tokio")]
pub(crate) use task::event_hub;

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub use host::EventSubscription;

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub(crate) use host::{EventHubHandle, event_hub};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
mod host;
