//! Immutable observer port and no-op/reference adapters.

use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ComponentRef, EffectId, EventId, LaneId, Metadata, ModelRequestId, RunEvent, RunEventBody,
    RunEventClass, RunEventKind, RunId, Sensitivity, SessionId, Timestamp, ToolBatchId, ToolCallId,
    TurnId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{PortFuture, PortObject};

/// Stable observer configuration error.
pub const OBSERVER_CONFIGURATION_INVALID: &str = "observer_configuration_invalid";
/// Stable bounded reference-observer capacity error.
pub const OBSERVER_CAPACITY_EXCEEDED: &str = "observer_capacity_exceeded";
/// Stable poisoned reference-observer state error.
pub const OBSERVER_UNAVAILABLE: &str = "observer_unavailable";

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
pub trait Observer: PortObject {
    /// Immutable descriptor.
    fn descriptor(&self) -> ObserverDescriptor;

    /// Observe one immutable, logically ordered event batch.
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

/// Bounded in-memory reference observer for deterministic tests and examples.
pub struct ReferenceObserver {
    descriptor: ObserverDescriptor,
    max_events: usize,
    events: Arc<Mutex<Vec<ObserverEventView>>>,
}

impl ReferenceObserver {
    /// Construct a bounded capture observer.
    ///
    /// # Errors
    ///
    /// Returns `observer_configuration_invalid` for a zero or excessive event bound.
    pub fn try_new(
        descriptor: ObserverDescriptor,
        max_events: usize,
    ) -> Result<Self, ObserverError> {
        if max_events == 0 || max_events > 1_000_000 {
            return Err(ObserverError::ConfigurationInvalid);
        }
        Ok(Self {
            descriptor,
            max_events,
            events: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Snapshot captured immutable views in delivery order.
    ///
    /// # Errors
    ///
    /// Returns `observer_unavailable` when the capture lock is poisoned.
    pub fn snapshot(&self) -> Result<Arc<[ObserverEventView]>, ObserverError> {
        self.events
            .lock()
            .map(|events| Arc::from(events.clone()))
            .map_err(|_| ObserverError::Unavailable)
    }
}

impl Observer for ReferenceObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let events = Arc::clone(&self.events);
        let max_events = self.max_events;
        let mode = self.descriptor.payload_mode;
        Box::pin(async move {
            let mut captured = events.lock().map_err(|_| ObserverError::Unavailable)?;
            if captured.len().saturating_add(batch.len()) > max_events {
                return Err(ObserverError::CapacityExceeded);
            }
            captured.extend(
                batch
                    .iter()
                    .map(|event| ObserverEventView::from_event(event, mode)),
            );
            Ok(())
        })
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

#[cfg(all(test, feature = "native-tokio"))]
mod tests {
    use super::*;
    use finstack_ai_kernel::{
        ComponentId, EventTag, Id, IdTag, LaneTag, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
        RUN_EVENT_SCHEMA_VERSION, RunEventBody, RunTag, SessionTag, Version,
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

    fn descriptor(mode: ObserverPayloadMode) -> ObserverDescriptor {
        ObserverDescriptor {
            component: ComponentRef::new(
                ComponentId::parse("fixture.observer").expect("component"),
                Some(Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                }),
            ),
            payload_mode: mode,
            metadata: Metadata::empty(),
        }
    }

    #[cfg(feature = "native-tokio")]
    #[tokio::test]
    async fn observer_projection_is_read_only_bounded_and_credential_payloads_stay_hidden() {
        let observer =
            ReferenceObserver::try_new(descriptor(ObserverPayloadMode::Full), 2).expect("observer");
        observer
            .observe(Arc::from([
                event(Sensitivity::Public),
                event(Sensitivity::Credential),
            ]))
            .await
            .expect("observe");
        let views = observer.snapshot().expect("snapshot");
        assert!(views[0].body.is_some());
        assert!(views[1].body.is_none());
        assert_eq!(views[0].run_id, id::<RunTag>(3));

        let error = observer
            .observe(Arc::from([event(Sensitivity::Public)]))
            .await
            .expect_err("capacity");
        assert_eq!(error.code(), OBSERVER_CAPACITY_EXCEEDED);

        let metadata_only = ObserverEventView::from_event(
            &event(Sensitivity::Public),
            ObserverPayloadMode::MetadataOnly,
        );
        assert!(metadata_only.body.is_none());
    }
}
