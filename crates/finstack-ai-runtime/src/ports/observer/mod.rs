//! Immutable observer port and no-op/reference adapters.

pub mod export;

#[cfg(any(feature = "native-tokio", feature = "wasm-host", test))]
use std::collections::VecDeque;
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentRef, EffectId, EventId, LaneId, Metadata, ModelRequestId, RunEvent, RunEventBody,
    RunEventClass, RunEventKind, RunId, Sensitivity, SessionId, Timestamp, ToolBatchId, ToolCallId,
    TurnId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ports::{PortFuture, PortObject};

/// Stable observer configuration error.
pub const OBSERVER_CONFIGURATION_INVALID: &str = "observer_configuration_invalid";
/// Stable bounded reference-observer capacity error.
pub const OBSERVER_CAPACITY_EXCEEDED: &str = "observer_capacity_exceeded";
/// Stable poisoned reference-observer state error.
pub const OBSERVER_UNAVAILABLE: &str = "observer_unavailable";
/// Stable observer subscription failure diagnostic.
pub const OBSERVER_SUBSCRIPTION_FAILED: &str = "observer_subscription_failed";
/// Stable observer callback failure diagnostic.
pub const OBSERVER_DELIVERY_FAILED: &str = "observer_delivery_failed";
/// Stable observer shutdown timeout diagnostic.
pub const OBSERVER_SHUTDOWN_TIMEOUT: &str = "observer_shutdown_timeout";

#[cfg(any(feature = "native-tokio", feature = "wasm-host", test))]
const MAX_OBSERVER_DIAGNOSTICS: usize = 32;

/// Non-semantic observer diagnostic. Never a `RunEvent` kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserverDiagnostic {
    /// Stable diagnostic code.
    pub code: &'static str,
    /// Non-secret operator text.
    pub detail: &'static str,
}

/// Bounded non-semantic diagnostics retained by one process-local run owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverDiagnostics {
    /// Total diagnostics observed, including entries evicted from `recent`.
    pub total: u64,
    /// Number of older diagnostics evicted to preserve the fixed bound.
    pub dropped: u64,
    /// Most recent redacted diagnostics in source order.
    pub recent: Arc<[ObserverDiagnostic]>,
}

impl ObserverDiagnostics {
    /// Return a safe snapshot when the diagnostics lock cannot be read.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn unavailable() -> Self {
        Self {
            total: 1,
            dropped: 0,
            recent: Arc::from([ObserverDiagnostic {
                code: OBSERVER_UNAVAILABLE,
                detail: "observer diagnostics unavailable",
            }]),
        }
    }
}

/// Mutable fixed-capacity storage shared by one process-local run owner.
#[derive(Default)]
#[cfg(any(feature = "native-tokio", feature = "wasm-host", test))]
pub(crate) struct ObserverDiagnosticBuffer {
    total: u64,
    recent: VecDeque<ObserverDiagnostic>,
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host", test))]
impl ObserverDiagnosticBuffer {
    /// Retain one redacted diagnostic, evicting the oldest entry at capacity.
    pub(crate) fn record(&mut self, diagnostic: ObserverDiagnostic) {
        self.total = self.total.saturating_add(1);
        if self.recent.len() == MAX_OBSERVER_DIAGNOSTICS {
            self.recent.pop_front();
        }
        self.recent.push_back(diagnostic);
    }

    /// Clone the current bounded summary for read-only callers.
    pub(crate) fn snapshot(&self) -> ObserverDiagnostics {
        let retained = u64::try_from(self.recent.len()).unwrap_or(u64::MAX);
        ObserverDiagnostics {
            total: self.total,
            dropped: self.total.saturating_sub(retained),
            recent: self.recent.iter().copied().collect::<Vec<_>>().into(),
        }
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn diagnostics_retain_only_the_fixed_recent_bound() {
        let mut diagnostics = ObserverDiagnosticBuffer::default();
        for _ in 0..(MAX_OBSERVER_DIAGNOSTICS + 5) {
            diagnostics.record(ObserverDiagnostic {
                code: OBSERVER_DELIVERY_FAILED,
                detail: "observer delivery failed",
            });
        }
        let snapshot = diagnostics.snapshot();
        assert_eq!(
            snapshot.total,
            u64::try_from(MAX_OBSERVER_DIAGNOSTICS + 5).expect("bound")
        );
        assert_eq!(snapshot.dropped, 5);
        assert_eq!(snapshot.recent.len(), MAX_OBSERVER_DIAGNOSTICS);
    }
}

/// Payload projection requested by a trusted native observer adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverPayloadMode {
    /// Correlation and classification only.
    MetadataOnly,
    /// Public event bodies only; higher sensitivity remains metadata-only.
    Redacted,
    /// Full non-credential event bodies.
    Full,
}

/// Immutable observer descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverDescriptor {
    /// Exact resolved component.
    pub component: ComponentRef,
    /// Payload projection applied by the adapter.
    pub payload_mode: ObserverPayloadMode,
    /// Non-secret descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

/// Immutable event view retained by the reference observer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObserverEventView {
    /// Durable or transient class.
    pub class: RunEventClass,
    /// Event kind.
    pub kind: RunEventKind,
    /// Event identity.
    pub event_id: EventId,
    /// Session identity.
    pub session_id: SessionId,
    /// Lane identity.
    pub lane_id: LaneId,
    /// Run identity.
    pub run_id: RunId,
    /// Optional turn identity.
    pub turn_id: Option<TurnId>,
    /// Optional model request identity.
    pub model_request_id: Option<ModelRequestId>,
    /// Optional tool batch identity.
    pub tool_batch_id: Option<ToolBatchId>,
    /// Optional effect identity.
    pub effect_id: Option<EffectId>,
    /// Optional tool call identity.
    pub tool_call_id: Option<ToolCallId>,
    /// Optional durable record sequence.
    pub durable_sequence: Option<u64>,
    /// Per-run transient stream sequence.
    pub transient_sequence: u64,
    /// Semantic timestamp.
    pub timestamp: Timestamp,
    /// Sensitivity classification.
    pub sensitivity: Sensitivity,
    /// Body only when the configured view authorizes it.
    pub body: Option<RunEventBody>,
}

impl ObserverEventView {
    /// Project one immutable event without ever exposing credential-class payloads.
    #[must_use]
    pub fn from_event(event: &RunEvent, mode: ObserverPayloadMode) -> Self {
        let include_body = match mode {
            ObserverPayloadMode::MetadataOnly => false,
            ObserverPayloadMode::Redacted => event.sensitivity() == Sensitivity::Public,
            ObserverPayloadMode::Full => event.sensitivity() != Sensitivity::Credential,
        };
        Self {
            class: event.class(),
            kind: event.kind(),
            event_id: event.event_id(),
            session_id: event.session_id(),
            lane_id: event.lane_id(),
            run_id: event.run_id(),
            turn_id: event.turn_id(),
            model_request_id: event.model_request_id(),
            tool_batch_id: event.tool_batch_id(),
            effect_id: event.effect_id(),
            tool_call_id: event.tool_call_id(),
            durable_sequence: event.durable_sequence(),
            transient_sequence: event.transient_sequence(),
            timestamp: event.timestamp(),
            sensitivity: event.sensitivity(),
            body: include_body.then(|| event.body().clone()),
        }
    }
}

/// Object-safe read-only observer port.
///
/// Observers never change behavior or terminal state. Delivery is batched.
/// Failures are isolated from the run.
pub trait Observer: PortObject {
    /// Immutable descriptor.
    fn descriptor(&self) -> ObserverDescriptor;

    /// Observe one immutable, logically ordered event batch.
    ///
    /// # Arguments
    ///
    /// * `batch` - Events already committed or emitted for observation. Do not mutate them.
    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>>;
}

/// No-op observer used when no operational adapter is configured.
#[derive(Debug, Clone)]
pub struct NoopObserver {
    descriptor: ObserverDescriptor,
}

impl NoopObserver {
    /// Construct a no-op observer.
    #[must_use]
    pub const fn new(descriptor: ObserverDescriptor) -> Self {
        Self { descriptor }
    }
}

impl Observer for NoopObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, _batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Bounded observer adapter failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ObserverError {
    /// Invalid construction bound.
    #[error("{OBSERVER_CONFIGURATION_INVALID}")]
    ConfigurationInvalid,
    /// The reference adapter reached its explicit bound.
    #[error("{OBSERVER_CAPACITY_EXCEEDED}")]
    CapacityExceeded,
    /// The reference adapter state is unavailable.
    #[error("{OBSERVER_UNAVAILABLE}")]
    Unavailable,
}

impl ObserverError {
    /// Stable error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ConfigurationInvalid => OBSERVER_CONFIGURATION_INVALID,
            Self::CapacityExceeded => OBSERVER_CAPACITY_EXCEEDED,
            Self::Unavailable => OBSERVER_UNAVAILABLE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{
        EventTag, Id, IdTag, LaneTag, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
        RUN_EVENT_SCHEMA_VERSION, RunEventBody, RunTag, SessionTag,
    };

    fn id<T: IdTag>(value: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8] = 0x80;
        bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
        Id::from_bytes(bytes)
    }

    fn event(sensitivity: Sensitivity) -> RunEvent {
        RunEvent::try_transient(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            id::<EventTag>(10),
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            id::<RunTag>(3),
            None,
            None,
            None,
            None,
            None,
            1,
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            sensitivity,
            RunEventBody::QueueDepthWarning(QueueDepthWarning { depth: 1, limit: 8 }),
        )
        .expect("event")
    }

    #[test]
    fn observer_projection_keeps_credential_payloads_hidden() {
        let public =
            ObserverEventView::from_event(&event(Sensitivity::Public), ObserverPayloadMode::Full);
        assert!(public.body.is_some());
        assert_eq!(public.run_id, id::<RunTag>(3));

        let credential = ObserverEventView::from_event(
            &event(Sensitivity::Credential),
            ObserverPayloadMode::Full,
        );
        assert!(credential.body.is_none());

        let metadata_only = ObserverEventView::from_event(
            &event(Sensitivity::Public),
            ObserverPayloadMode::MetadataOnly,
        );
        assert!(metadata_only.body.is_none());
    }
}
