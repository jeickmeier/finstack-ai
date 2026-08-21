//! Fail-closed security-audit port for external ingress.

use std::sync::Arc;
#[cfg(feature = "native-tokio")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "native-tokio")]
use std::time::Duration;

use finstack_ai_kernel::{Digest, PrincipalRef, Timestamp};
use thiserror::Error;

use crate::{PortFuture, PortObject};

/// Security-relevant external-ingress failure category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityAuditCategory {
    /// Caller authentication failed before command expansion.
    AuthenticationFailure,
    /// Authenticated scope did not authorize the named session/run.
    ScopeMismatch,
    /// Opaque callback token was malformed or failed signature validation.
    MalformedToken,
    /// Locator or target could not be established without revealing existence.
    UnknownLocator,
    /// Interaction routing is intentionally unavailable in this release.
    UnsupportedInteraction,
}

/// Redacted bounded event accepted by [`SecurityAuditSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityAuditEvent {
    event_id: Arc<str>,
    timestamp: Timestamp,
    principal: Option<PrincipalRef>,
    tenant_scope: Option<Arc<str>>,
    category: SecurityAuditCategory,
    reason_code: Arc<str>,
    locator_digest: Option<Digest>,
    submission_digest: Option<Digest>,
}

impl SecurityAuditEvent {
    /// Construct a redacted audit event from stable identities and digests only.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityAuditError::InvalidEvent`] for an invalid bounded label.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        event_id: impl AsRef<str>,
        timestamp: Timestamp,
        principal: Option<PrincipalRef>,
        tenant_scope: Option<impl AsRef<str>>,
        category: SecurityAuditCategory,
        reason_code: impl AsRef<str>,
        locator_digest: Option<Digest>,
        submission_digest: Option<Digest>,
    ) -> Result<Self, SecurityAuditError> {
        Ok(Self {
            event_id: audit_label(event_id.as_ref())?,
            timestamp,
            principal,
            tenant_scope: tenant_scope
                .map(|value| audit_label(value.as_ref()))
                .transpose()?,
            category,
            reason_code: audit_label(reason_code.as_ref())?,
            locator_digest,
            submission_digest,
        })
    }

    /// Stable idempotency identity.
    #[must_use]
    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    /// Frozen event timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Authenticated principal when disclosure is safe.
    #[must_use]
    pub const fn principal(&self) -> Option<&PrincipalRef> {
        self.principal.as_ref()
    }

    /// Authenticated tenant scope when disclosure is safe.
    #[must_use]
    pub fn tenant_scope(&self) -> Option<&str> {
        self.tenant_scope.as_deref()
    }

    /// Stable event category.
    #[must_use]
    pub const fn category(&self) -> SecurityAuditCategory {
        self.category
    }

    /// Stable bounded failure reason.
    #[must_use]
    pub fn reason_code(&self) -> &str {
        &self.reason_code
    }

    /// Digest of the authenticated locator, never the locator itself.
    #[must_use]
    pub const fn locator_digest(&self) -> Option<Digest> {
        self.locator_digest
    }

    /// Digest of normalized submitted content, never the content itself.
    #[must_use]
    pub const fn submission_digest(&self) -> Option<Digest> {
        self.submission_digest
    }
}

/// Idempotent audit-write receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityAuditReceipt {
    /// Stable event identity.
    pub event_id: Arc<str>,
    /// Sink acceptance time.
    pub recorded_at: Timestamp,
}

/// Security-audit sink health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityAuditHealth {
    /// Whether the sink can durably accept required ingress audit events.
    pub ready: bool,
}

/// Fail-closed security-audit sink.
pub trait SecurityAuditSink: PortObject {
    /// Idempotently record one redacted event by `event_id`.
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>>;

    /// Report whether required audit writes are available.
    fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>>;
}

/// Stable audit service failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityAuditError {
    /// Event labels violated bounded stable-vocabulary rules.
    #[error("invalid security audit event")]
    InvalidEvent,
    /// Sink rejected or could not durably record the event.
    #[error("security audit sink unavailable: {reason_code}")]
    Unavailable {
        /// Stable non-secret reason code.
        reason_code: &'static str,
    },
}

/// Healthy, deadline-bounded audit capability required to enable ingress.
#[cfg(feature = "native-tokio")]
pub struct SecurityAuditGate {
    sink: Arc<dyn SecurityAuditSink>,
    deadline: Duration,
    ready: AtomicBool,
    failure_count: AtomicU64,
}

/// Operator-visible health of the required audit-write path.
#[cfg(feature = "native-tokio")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityAuditGateHealth {
    /// Whether the most recent required write completed durably.
    pub ready: bool,
    /// Saturating count of required write failures since enablement.
    pub failure_count: u64,
}

#[cfg(feature = "native-tokio")]
impl SecurityAuditGate {
    /// Enable external ingress only after a bounded healthy-sink check.
    ///
    /// # Errors
    ///
    /// Fails closed for a missing/unhealthy sink, zero deadline, timeout, or sink error.
    pub async fn enable(
        sink: Option<Arc<dyn SecurityAuditSink>>,
        deadline: Duration,
    ) -> Result<Arc<Self>, SecurityAuditGateError> {
        if deadline.is_zero() {
            return Err(SecurityAuditGateError::InvalidDeadline);
        }
        let sink = sink.ok_or(SecurityAuditGateError::MissingSink)?;
        let health = tokio::time::timeout(deadline, sink.health())
            .await
            .map_err(|_| SecurityAuditGateError::Timeout)?
            .map_err(SecurityAuditGateError::Sink)?;
        if !health.ready {
            return Err(SecurityAuditGateError::Unhealthy);
        }
        Ok(Arc::new(Self {
            sink,
            deadline,
            ready: AtomicBool::new(true),
            failure_count: AtomicU64::new(0),
        }))
    }
}

/// In-process audit sink for trusted same-run list/resolve wrappers.
#[cfg(feature = "native-tokio")]
pub(crate) struct NoopSecurityAuditSink;

#[cfg(feature = "native-tokio")]
impl SecurityAuditSink for NoopSecurityAuditSink {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
        let event_id = Arc::<str>::from(event.event_id());
        let recorded_at = event.timestamp();
        Box::pin(async move {
            Ok(SecurityAuditReceipt {
                event_id,
                recorded_at,
            })
        })
    }

    fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
        Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
    }
}

#[cfg(feature = "native-tokio")]
impl SecurityAuditGate {
    /// Enable a healthy gate that records required events without an external sink.
    ///
    /// # Errors
    ///
    /// Fails closed if the no-op sink cannot be enabled.
    pub(crate) async fn enable_noop() -> Result<Arc<Self>, SecurityAuditGateError> {
        Self::enable(
            Some(Arc::new(NoopSecurityAuditSink)),
            Duration::from_millis(100),
        )
        .await
    }

    /// Record one required event before returning an ingress rejection.
    ///
    /// # Errors
    ///
    /// Fails closed on timeout, sink failure, or mismatched receipt identity.
    pub async fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> Result<SecurityAuditReceipt, SecurityAuditGateError> {
        match self.record_once(event.clone()).await {
            Ok(receipt) => Ok(receipt),
            Err(_) => self.record_once(event).await,
        }
    }

    async fn record_once(
        &self,
        event: SecurityAuditEvent,
    ) -> Result<SecurityAuditReceipt, SecurityAuditGateError> {
        let expected_id = Arc::<str>::from(event.event_id());
        let result = tokio::time::timeout(self.deadline, self.sink.record(event))
            .await
            .map_err(|_| SecurityAuditGateError::Timeout)
            .and_then(|value| value.map_err(SecurityAuditGateError::Sink))
            .and_then(|receipt| {
                if receipt.event_id == expected_id {
                    Ok(receipt)
                } else {
                    Err(SecurityAuditGateError::ReceiptMismatch)
                }
            });
        match result {
            Ok(receipt) => {
                self.ready.store(true, Ordering::Release);
                Ok(receipt)
            }
            Err(error) => {
                self.ready.store(false, Ordering::Release);
                let _ =
                    self.failure_count
                        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                            Some(current.saturating_add(1))
                        });
                Err(error)
            }
        }
    }

    /// Return operator-visible audit-path readiness without calling the sink.
    #[must_use]
    pub fn operational_health(&self) -> SecurityAuditGateHealth {
        SecurityAuditGateHealth {
            ready: self.ready.load(Ordering::Acquire),
            failure_count: self.failure_count.load(Ordering::Acquire),
        }
    }
}

/// Audit-gate construction/write failures that reject ingress closed.
#[cfg(feature = "native-tokio")]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityAuditGateError {
    /// External ingress was enabled without a sink.
    #[error("external ingress requires a security audit sink")]
    MissingSink,
    /// Audit deadline must be positive.
    #[error("security audit deadline must be positive")]
    InvalidDeadline,
    /// Sink reported not ready.
    #[error("security audit sink is unhealthy")]
    Unhealthy,
    /// Health or record operation exceeded its bounded deadline.
    #[error("security audit deadline exceeded")]
    Timeout,
    /// Sink returned an error.
    #[error(transparent)]
    Sink(SecurityAuditError),
    /// Sink acknowledged a different event identity.
    #[error("security audit receipt identity mismatch")]
    ReceiptMismatch,
}

fn audit_label(value: &str) -> Result<Arc<str>, SecurityAuditError> {
    if !finstack_ai_kernel::label_is_valid(value) {
        return Err(SecurityAuditError::InvalidEvent);
    }
    Ok(Arc::from(value))
}

#[cfg(all(test, feature = "native-tokio"))]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Clone, Copy)]
    enum Behavior {
        Ready,
        Unhealthy,
        FailRecord,
        Slow,
    }

    struct TestSink {
        behavior: Behavior,
        events: Mutex<BTreeMap<Arc<str>, SecurityAuditEvent>>,
    }

    impl TestSink {
        fn new(behavior: Behavior) -> Self {
            Self {
                behavior,
                events: Mutex::new(BTreeMap::new()),
            }
        }
    }

    impl SecurityAuditSink for TestSink {
        fn record(
            &self,
            event: SecurityAuditEvent,
        ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
            if matches!(self.behavior, Behavior::FailRecord) {
                return Box::pin(async {
                    Err(SecurityAuditError::Unavailable {
                        reason_code: "write_failed",
                    })
                });
            }
            let mut events = self.events.lock().expect("lock");
            let event_id = Arc::<str>::from(event.event_id());
            if let Some(existing) = events.get(&event_id) {
                if existing != &event {
                    return Box::pin(async {
                        Err(SecurityAuditError::Unavailable {
                            reason_code: "event_id_reuse",
                        })
                    });
                }
            } else {
                events.insert(Arc::clone(&event_id), event.clone());
            }
            let recorded_at = event.timestamp();
            Box::pin(async move {
                Ok(SecurityAuditReceipt {
                    event_id,
                    recorded_at,
                })
            })
        }

        fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
            let behavior = self.behavior;
            Box::pin(async move {
                if matches!(behavior, Behavior::Slow) {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Ok(SecurityAuditHealth {
                    ready: !matches!(behavior, Behavior::Unhealthy),
                })
            })
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime")
    }

    fn event(id: &str, reason: &str) -> SecurityAuditEvent {
        SecurityAuditEvent::try_new(
            id,
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            None::<&str>,
            SecurityAuditCategory::UnknownLocator,
            reason,
            Some(Digest::raw_json(b"locator")),
            Some(Digest::raw_json(b"submission")),
        )
        .expect("event")
    }

    #[test]
    fn event_is_bounded_and_redacted_by_construction() {
        assert!(
            SecurityAuditEvent::try_new(
                "",
                Timestamp::from_unix_ms(1_000).expect("timestamp"),
                None,
                None::<&str>,
                SecurityAuditCategory::MalformedToken,
                "malformed_token",
                None,
                None,
            )
            .is_err()
        );
        let event = event("audit-1", "unknown_locator");
        assert_eq!(event.event_id(), "audit-1");
        assert!(event.principal().is_none());
        assert!(event.tenant_scope().is_none());
        assert!(event.locator_digest().is_some());
        assert!(event.submission_digest().is_some());
    }

    #[test]
    fn healthy_gate_records_idempotently() {
        runtime().block_on(async {
            let sink = Arc::new(TestSink::new(Behavior::Ready));
            let gate = SecurityAuditGate::enable(Some(sink.clone()), Duration::from_millis(100))
                .await
                .expect("gate");
            let first = gate.record(event("audit-1", "unknown_locator")).await;
            let second = gate.record(event("audit-1", "unknown_locator")).await;
            assert_eq!(first.expect("first"), second.expect("second"));
            assert_eq!(sink.events.lock().expect("lock").len(), 1);
        });
    }

    #[test]
    fn readiness_and_write_failures_reject_closed() {
        runtime().block_on(async {
            assert!(matches!(
                SecurityAuditGate::enable(None, Duration::from_millis(10)).await,
                Err(SecurityAuditGateError::MissingSink)
            ));
            assert!(matches!(
                SecurityAuditGate::enable(
                    Some(Arc::new(TestSink::new(Behavior::Unhealthy))),
                    Duration::from_millis(10),
                )
                .await,
                Err(SecurityAuditGateError::Unhealthy)
            ));
            assert!(matches!(
                SecurityAuditGate::enable(
                    Some(Arc::new(TestSink::new(Behavior::Slow))),
                    Duration::from_millis(1),
                )
                .await,
                Err(SecurityAuditGateError::Timeout)
            ));
            let gate = SecurityAuditGate::enable(
                Some(Arc::new(TestSink::new(Behavior::FailRecord))),
                Duration::from_millis(10),
            )
            .await
            .expect("gate");
            assert!(matches!(
                gate.record(event("audit-1", "unknown_locator")).await,
                Err(SecurityAuditGateError::Sink(
                    SecurityAuditError::Unavailable {
                        reason_code: "write_failed"
                    }
                ))
            ));
            assert_eq!(
                gate.operational_health(),
                SecurityAuditGateHealth {
                    ready: false,
                    failure_count: 2,
                }
            );
        });
    }
}
