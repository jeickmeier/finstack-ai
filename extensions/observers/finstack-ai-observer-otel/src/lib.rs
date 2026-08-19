//! OpenTelemetry observer. Default export is in-process; OTLP is feature-gated.

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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, RunEvent, RunEventKind, Version};
use finstack_ai_runtime::{
    OBSERVER_QUEUE_OVERFLOW, Observer, ObserverBackpressure, ObserverDescriptor,
    ObserverDiagnostic, ObserverError, ObserverEventView, ObserverPayloadMode, ObserverQueue,
    ObserverQueuePush, PortFuture,
};
use opentelemetry::KeyValue;
use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;
use thiserror::Error;

/// OTel-observer construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OtelObserverError {
    /// Configuration is malformed.
    #[error("otel_observer_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// One captured in-process span used by tests and support snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSpan {
    /// Stable span name.
    pub name: String,
    /// Identifier and classification attributes only, unless a public body is authorized.
    pub attributes: Vec<(String, String)>,
}

/// OpenTelemetry observer with a bounded export queue and in-memory capture.
pub struct OtelObserver {
    descriptor: ObserverDescriptor,
    provider: SdkTracerProvider,
    queue: ObserverQueue<CapturedSpan>,
    captured: Mutex<Vec<CapturedSpan>>,
    capture_capacity: usize,
    diagnostic: Mutex<Option<ObserverDiagnostic>>,
}

impl OtelObserver {
    /// Construct a redacted in-memory observer.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity or queue bound.
    pub fn try_redacted(
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
    ) -> Result<Self, OtelObserverError> {
        Self::try_new(ObserverPayloadMode::Redacted, queue_capacity, backpressure)
    }

    /// Construct an observer with an explicit payload mode.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity or queue bound.
    pub fn try_new(
        payload_mode: ObserverPayloadMode,
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
    ) -> Result<Self, OtelObserverError> {
        let provider = SdkTracerProvider::builder().build();
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.observer.otel").map_err(|_| {
                        OtelObserverError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    Some(Version {
                        major: 0,
                        minor: 0,
                        patch: 4,
                    }),
                ),
                payload_mode,
                metadata: Metadata::empty(),
            },
            provider,
            queue: ObserverQueue::try_new(queue_capacity, backpressure).map_err(|_| {
                OtelObserverError::Configuration {
                    reason: "invalid_queue_capacity",
                }
            })?,
            captured: Mutex::new(Vec::new()),
            capture_capacity: queue_capacity,
            diagnostic: Mutex::new(None),
        })
    }

    /// Snapshot captured spans in delivery order.
    ///
    /// # Errors
    ///
    /// Returns `observer_unavailable` when the capture lock is poisoned.
    pub fn snapshot(&self) -> Result<Vec<CapturedSpan>, ObserverError> {
        self.captured
            .lock()
            .map(|spans| spans.clone())
            .map_err(|_| ObserverError::Unavailable)
    }

    /// Cumulative dropped export spans.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.queue.dropped()
    }

    /// Last overflow diagnostic.
    #[must_use]
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic> {
        self.diagnostic.lock().ok().and_then(|slot| *slot)
    }

    /// Flush the in-process provider. No network I/O.
    pub fn force_flush(&self) {
        let _ = self.provider.force_flush();
    }

    fn record_overflow(&self) {
        if let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(OBSERVER_QUEUE_OVERFLOW);
        }
    }

    fn map_span(&self, event: &RunEvent) -> CapturedSpan {
        let view = ObserverEventView::from_event(event, self.descriptor.payload_mode);
        let mut attributes = vec![
            ("finstack.class".to_owned(), format!("{:?}", view.class)),
            ("finstack.kind".to_owned(), format!("{:?}", view.kind)),
            (
                "finstack.sensitivity".to_owned(),
                format!("{:?}", view.sensitivity),
            ),
            (
                "finstack.session_id".to_owned(),
                view.session_id.to_string(),
            ),
            ("finstack.run_id".to_owned(), view.run_id.to_string()),
            ("finstack.event_id".to_owned(), view.event_id.to_string()),
        ];
        if let Some(effect_id) = view.effect_id {
            attributes.push(("finstack.effect_id".to_owned(), effect_id.to_string()));
        }
        if let Some(tool_call_id) = view.tool_call_id {
            attributes.push(("finstack.tool_call_id".to_owned(), tool_call_id.to_string()));
        }
        if let Some(body) = &view.body {
            attributes.push(("finstack.body".to_owned(), serde_json_string(body)));
        }
        CapturedSpan {
            name: span_name(view.kind).to_owned(),
            attributes,
        }
    }
}

impl Observer for OtelObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        for event in batch.iter() {
            let span = self.map_span(event);
            match self.queue.push(span) {
                Ok(ObserverQueuePush::Accepted) => {}
                Ok(ObserverQueuePush::Dropped) => self.record_overflow(),
                Err(error) => {
                    self.record_overflow();
                    return Box::pin(async move { Err(error) });
                }
            }
        }
        let accepted = match self.queue.drain() {
            Ok(spans) => spans,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let tracer = self.provider.tracer("finstack-ai-observer-otel");
        for span in &accepted {
            let attributes = span
                .attributes
                .iter()
                .map(|(key, value)| KeyValue::new(key.clone(), value.clone()))
                .collect::<Vec<_>>();
            let mut started = tracer
                .span_builder(span.name.clone())
                .with_attributes(attributes)
                .start(&tracer);
            started.end();
        }
        if let Ok(mut captured) = self.captured.lock() {
            captured.extend(accepted);
            if captured.len() > self.capture_capacity {
                let overflow = captured.len().saturating_sub(self.capture_capacity);
                captured.drain(..overflow);
                self.record_overflow();
            }
        }
        self.force_flush();
        Box::pin(async { Ok(()) })
    }
}

fn span_name(kind: RunEventKind) -> &'static str {
    match kind {
        RunEventKind::EffectRequested
        | RunEventKind::EffectDeferred
        | RunEventKind::EffectCompleted
        | RunEventKind::EffectFailed
        | RunEventKind::EffectCancelled => "finstack.effect",
        RunEventKind::ToolSettled | RunEventKind::ToolProgress => "finstack.tool",
        _ => "finstack.run",
    }
}

fn serde_json_string(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// Default bounded-block timeout used by examples.
#[must_use]
pub const fn default_block_timeout() -> Duration {
    Duration::from_millis(20)
}

#[cfg(test)]
mod tests;
