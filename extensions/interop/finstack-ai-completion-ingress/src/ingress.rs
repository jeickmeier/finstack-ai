//! Completion ingress service.

use thiserror::Error;

/// Caller-visible delivery failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IngressError {
    /// One non-existence-revealing response for every token/body/audit failure.
    #[error("external completion rejected")]
    Rejected,
    /// Store/commit/id-allocation failure on a known authorized target.
    #[error("external completion ingress unavailable: {reason_code}")]
    Unavailable {
        /// Stable retryable-failure reason.
        reason_code: &'static str,
    },
}
