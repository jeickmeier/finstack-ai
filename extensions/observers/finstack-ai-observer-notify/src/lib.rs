//! Announce-only HITL notifier observer. It watches the four interaction
//! lifecycle events and delivers redacted notifications to an outbound sink.
//! It can never resolve an interaction or change the run.

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

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    ComponentId, ComponentRef, InteractionId, Metadata, RunEvent, RunId, SessionId, Timestamp,
    Version,
};
use finstack_ai_runtime::{
    OBSERVER_QUEUE_OVERFLOW, Observer, ObserverBackpressure, ObserverDescriptor,
    ObserverDiagnostic, ObserverError, ObserverPayloadMode, ObserverQueue, ObserverQueuePush,
    PortFuture,
};
use serde::Serialize;
use thiserror::Error;

/// Notify-observer construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NotifyObserverError {
    /// Configuration is malformed.
    #[error("notify_observer_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Outbound sink delivery failure. Reasons are stable and never echo payloads.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SinkError {
    /// The sink endpoint could not be reached or rejected the notification.
    #[error("notify_sink_unavailable: {reason}")]
    Unavailable {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Which lifecycle event a notification announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionEventKind {
    /// A human interaction was requested.
    Requested,
    /// The interaction was resolved.
    Resolved,
    /// The interaction expired.
    Expired,
    /// The interaction was cancelled.
    Cancelled,
}

/// Redacted principal identity: issuer and subject labels only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrincipalLabel {
    /// Identity-issuer label.
    pub issuer: Arc<str>,
    /// Subject label within that issuer.
    pub subject: Arc<str>,
}

/// Redacted assignee hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssigneeLabel {
    /// Concrete principal (issuer/subject labels only).
    Principal(PrincipalLabel),
    /// Role name.
    Role(Arc<str>),
    /// Queue name.
    Queue(Arc<str>),
}

/// Per-event whitelisted detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "detail_kind", rename_all = "snake_case")]
pub enum NotificationDetail {
    /// A human interaction was requested.
    Requested {
        /// Interaction kind wire label (including validated custom labels).
        kind: Arc<str>,
        /// Optional redacted assignee hint.
        assignee: Option<AssigneeLabel>,
        /// Optional expiry.
        expires_at: Option<Timestamp>,
        /// Whether the assignee may delegate.
        delegatable: bool,
    },
    /// The interaction was resolved.
    Resolved {
        /// Idempotent resolution id label.
        resolution_id: Arc<str>,
        /// Resolving principal (labels only).
        principal: PrincipalLabel,
        /// Optional label-validated resolver comment.
        comment: Option<Arc<str>>,
    },
    /// The interaction expired.
    Expired {
        /// Expiry timestamp.
        expired_at: Timestamp,
    },
    /// The interaction was cancelled.
    Cancelled {
        /// Optional cancelling principal (labels only).
        principal: Option<PrincipalLabel>,
        /// Optional label-validated reason.
        reason: Option<Arc<str>>,
    },
}

/// Whitelisted projection of one interaction lifecycle event.
///
/// This is the only value a sink ever sees. It never carries prompt content,
/// response schemas, resolution responses, authorization evidence, or
/// metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionNotification {
    /// Lifecycle event announced.
    pub event: InteractionEventKind,
    /// Interaction identity.
    pub interaction_id: InteractionId,
    /// Session the interaction belongs to.
    pub session_id: SessionId,
    /// Run the interaction belongs to.
    pub run_id: RunId,
    /// Event timestamp.
    pub timestamp: Timestamp,
    /// Per-event whitelisted detail.
    pub detail: NotificationDetail,
}

/// Object-safe outbound notification sink.
pub trait NotificationSink: Send + Sync + 'static {
    /// Stable sink name for diagnostics.
    fn name(&self) -> &'static str;

    /// Deliver one notification. Side-effect-only; the observer uses the
    /// error solely for retry accounting.
    fn deliver(&self, notification: InteractionNotification) -> PortFuture<Result<(), SinkError>>;
}

const MIN_REQUEST_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_ATTEMPTS: u32 = 5;
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(10);

/// Bounded per-notification delivery policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryPolicy {
    request_timeout: Duration,
    max_attempts: u32,
    retry_backoff: Duration,
}

impl DeliveryPolicy {
    /// Construct a delivery policy.
    ///
    /// # Errors
    ///
    /// Rejects a timeout outside `100ms..=60s`, attempts outside `1..=5`,
    /// or a backoff above `10s`.
    pub fn try_new(
        request_timeout: Duration,
        max_attempts: u32,
        retry_backoff: Duration,
    ) -> Result<Self, NotifyObserverError> {
        if request_timeout < MIN_REQUEST_TIMEOUT || request_timeout > MAX_REQUEST_TIMEOUT {
            return Err(NotifyObserverError::Configuration {
                reason: "invalid_request_timeout",
            });
        }
        if !(1..=MAX_ATTEMPTS).contains(&max_attempts) {
            return Err(NotifyObserverError::Configuration {
                reason: "invalid_max_attempts",
            });
        }
        if retry_backoff > MAX_RETRY_BACKOFF {
            return Err(NotifyObserverError::Configuration {
                reason: "invalid_retry_backoff",
            });
        }
        Ok(Self {
            request_timeout,
            max_attempts,
            retry_backoff,
        })
    }
}

impl Default for DeliveryPolicy {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(5),
            max_attempts: 3,
            retry_backoff: Duration::from_millis(500),
        }
    }
}

/// Diagnostic stored when a notification exhausts its delivery attempts.
pub const NOTIFY_DELIVERY_FAILED: ObserverDiagnostic = ObserverDiagnostic {
    code: "notify_delivery_failed",
    detail: "notification dropped after exhausting sink delivery attempts",
};

/// Announce-only interaction lifecycle observer.
pub struct NotifyObserver {
    descriptor: ObserverDescriptor,
    sink: Arc<dyn NotificationSink>,
    policy: DeliveryPolicy,
    queue: ObserverQueue<InteractionNotification>,
    dropped: AtomicU64,
    delivered: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    diagnostic: Arc<Mutex<Option<ObserverDiagnostic>>>,
    delivery_gate: Arc<tokio::sync::Mutex<()>>,
}

impl NotifyObserver {
    /// Construct a notify observer over one sink.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity, an invalid queue bound, or
    /// [`ObserverBackpressure::BlockBounded`] — the queue's only consumer is
    /// this observer's own drain, so blocking for capacity can never succeed
    /// and would spin the caller's thread; use `DropProgress` or `Disconnect`.
    pub fn try_new(
        sink: Arc<dyn NotificationSink>,
        policy: DeliveryPolicy,
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
    ) -> Result<Self, NotifyObserverError> {
        if matches!(backpressure, ObserverBackpressure::BlockBounded { .. }) {
            return Err(NotifyObserverError::Configuration {
                reason: "unsupported_backpressure_block_bounded",
            });
        }
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.observer.notify").map_err(|_| {
                        NotifyObserverError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    Some(Version {
                        major: 0,
                        minor: 0,
                        patch: 1,
                    }),
                ),
                payload_mode: ObserverPayloadMode::Full,
                metadata: Metadata::empty(),
            },
            sink,
            policy,
            queue: ObserverQueue::try_new(queue_capacity, backpressure).map_err(|_| {
                NotifyObserverError::Configuration {
                    reason: "invalid_queue_capacity",
                }
            })?,
            dropped: AtomicU64::new(0),
            delivered: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicU64::new(0)),
            diagnostic: Arc::new(Mutex::new(None)),
            delivery_gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// Notifications delivered successfully.
    #[must_use]
    pub fn delivered(&self) -> u64 {
        self.delivered.load(Ordering::Relaxed)
    }

    /// Notifications dropped after exhausting delivery attempts.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }

    /// Notifications dropped by queue backpressure.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed) + self.queue.dropped()
    }

    /// Last stored diagnostic.
    #[must_use]
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic> {
        self.diagnostic.lock().ok().and_then(|slot| *slot)
    }

    /// Store the overflow diagnostic; `count_locally` covers queue errors the
    /// queue's own `dropped()` counter does not record.
    fn record_overflow(&self, count_locally: bool) {
        if count_locally {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(OBSERVER_QUEUE_OVERFLOW);
        }
    }
}

impl Observer for NotifyObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let mut push_error = None;
        for event in batch.iter() {
            let Some(notification) = project(event) else {
                continue;
            };
            match self.queue.push(notification) {
                Ok(ObserverQueuePush::Accepted) => {}
                Ok(ObserverQueuePush::Dropped) => self.record_overflow(false),
                Err(error) => {
                    // CapacityExceeded is already counted by the queue itself.
                    self.record_overflow(!matches!(error, ObserverError::CapacityExceeded));
                    push_error = Some(error);
                    break;
                }
            }
        }
        // Drain even after a push error so accepted notifications still ship.
        let pending = match self.queue.drain() {
            Ok(pending) => pending,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let sink = Arc::clone(&self.sink);
        let policy = self.policy.clone();
        let delivered = Arc::clone(&self.delivered);
        let failed = Arc::clone(&self.failed);
        let diagnostic = Arc::clone(&self.diagnostic);
        let gate = Arc::clone(&self.delivery_gate);
        Box::pin(async move {
            if !pending.is_empty() {
                // Deliver in a spawned task so a slow sink never stalls the
                // subscription loop awaiting this future; the FIFO gate keeps
                // batches delivering in observe order.
                tokio::spawn(deliver_pending(
                    sink, policy, pending, delivered, failed, diagnostic, gate,
                ));
            }
            match push_error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    }
}

async fn deliver_pending(
    sink: Arc<dyn NotificationSink>,
    policy: DeliveryPolicy,
    pending: Vec<InteractionNotification>,
    delivered: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    diagnostic: Arc<Mutex<Option<ObserverDiagnostic>>>,
    gate: Arc<tokio::sync::Mutex<()>>,
) {
    let _ordered = gate.lock().await;
    for notification in pending {
        let mut attempt = 0_u32;
        loop {
            attempt += 1;
            let outcome =
                tokio::time::timeout(policy.request_timeout, sink.deliver(notification.clone()))
                    .await;
            match outcome {
                Ok(Ok(())) => {
                    delivered.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                // A timed-out request may still have been received by the
                // endpoint; retrying it would duplicate the notification.
                Ok(Err(_)) if attempt < policy.max_attempts => {
                    tokio::time::sleep(policy.retry_backoff).await;
                }
                Ok(Err(_)) | Err(_) => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut slot) = diagnostic.lock() {
                        *slot = Some(NOTIFY_DELIVERY_FAILED);
                    }
                    break;
                }
            }
        }
    }
}

mod http;
mod project;
mod slack;
mod webhook;

pub(crate) use project::project;
pub use slack::{SlackSink, slack_text};
pub use webhook::WebhookSink;

#[cfg(test)]
mod tests;
