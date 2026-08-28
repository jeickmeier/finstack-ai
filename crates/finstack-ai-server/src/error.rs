//! Reference-server errors.

use finstack_ai_protocol::ProtocolError;
use finstack_ai_runtime::audit::SecurityAuditGateError;
use thiserror::Error;

/// Stable reference-server failures.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Listen address or TLS policy was rejected.
    #[error("server listen invalid")]
    ListenInvalid,
    /// Audit gate was missing, unhealthy, or failed a required write.
    #[error("server audit not ready")]
    AuditNotReady,
    /// Authentication failed or used bearer over plaintext TCP.
    #[error("server authentication failed")]
    AuthenticationFailure,
    /// Authentication verifier or context configuration was invalid.
    #[error("server authentication configuration invalid")]
    AuthenticationConfigurationInvalid,
    /// Locator was unknown or unauthorized; the two cases are not distinguished.
    #[error("unknown locator")]
    UnknownLocator,
    /// Authenticated scope did not match the command tenant.
    #[error("scope mismatch")]
    ScopeMismatch,
    /// A second writer tried to own the same session.
    #[error("session writer busy")]
    SessionBusy,
    /// Live events were queued before the sync barrier.
    #[error("live event before sync barrier")]
    LiveBeforeBarrier,
    /// Command identity was reused with a conflicting digest.
    #[error("command idempotency conflict")]
    IdempotencyConflict,
    /// Replica reached the command-receipt retention cap.
    #[error("receipt cap exceeded")]
    ReceiptCap,
    /// Session replica identity or tenant scope was invalid.
    #[error("server session invalid")]
    SessionInvalid,
    /// Ack deadline elapsed, or an outbound batch could not fit in the
    /// remaining window. Both use the stable code `credit_timeout`.
    #[error("credit window timeout")]
    CreditTimeout,
    /// Handshake or parse deadline elapsed.
    #[error("handshake timeout")]
    HandshakeTimeout,
    /// Protocol codec or frame failure.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// I/O failure on the accepted stream.
    #[error("server io: {0}")]
    Io(#[from] std::io::Error),
    /// Audit gate construction/write failure.
    #[error(transparent)]
    Audit(#[from] SecurityAuditGateError),
}

impl ServerError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ListenInvalid => "server_listen_invalid",
            Self::AuditNotReady => "server_audit_not_ready",
            Self::AuthenticationFailure => "authentication_failure",
            Self::AuthenticationConfigurationInvalid => "authentication_configuration_invalid",
            Self::UnknownLocator => "unknown_locator",
            Self::ScopeMismatch => "scope_mismatch",
            Self::SessionBusy => "session_busy",
            Self::LiveBeforeBarrier => "live_before_barrier",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::ReceiptCap => "receipt_cap",
            Self::SessionInvalid => "session_invalid",
            Self::CreditTimeout => "credit_timeout",
            Self::HandshakeTimeout => "handshake_timeout",
            Self::Protocol(_) => "protocol",
            Self::Io(_) => "io",
            Self::Audit(_) => "audit",
        }
    }
}
