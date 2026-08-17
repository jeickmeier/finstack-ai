use std::sync::Arc;

use finstack_ai_kernel::Digest;
use thiserror::Error;

use crate::CommitCoordinatorError;
use crate::CommitOutcome;

/// Successful classification of one authenticated external completion command.
#[derive(Debug, Clone)]
pub enum ExternalRouteOutcome {
    /// New completion records were committed and applied.
    Committed(CommitOutcome),
    /// Equal command identity and normalized digest were already committed.
    Idempotent {
        /// External command identity.
        command_id: Arc<str>,
        /// Normalized command digest.
        submitted_digest: Digest,
    },
    /// Known authorized command was durably rejected without semantic state change.
    Rejected {
        /// Stable rejection reason.
        reason_code: &'static str,
        /// Applied state-neutral rejection evidence.
        evidence: CommitOutcome,
    },
}

/// Non-secret external-ingress failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExternalRouteError {
    /// One non-existence-revealing response for locator/auth/audit failures.
    #[error("external command rejected")]
    IngressRejected,
    /// Runtime ID generation failed before any append.
    #[error("external command id allocation failed")]
    IdAllocation,
    /// Store/replay/commit processing failed on a known authorized target.
    #[error(transparent)]
    Runtime(CommitCoordinatorError),
    /// Router could not build a valid normalized kernel command.
    #[error("invalid normalized external command")]
    InvalidNormalizedCommand,
}
