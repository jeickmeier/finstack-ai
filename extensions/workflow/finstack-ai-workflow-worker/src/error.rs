use std::sync::Arc;

use finstack_ai_runtime::WorkflowDriverError;
use finstack_ai_workflow_local::CronError;
use thiserror::Error;

/// Fail-closed worker failures. These are not kernel record errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkerError {
    /// Adapter table is temporarily unavailable.
    #[error("worker store unavailable: {code}")]
    StoreUnavailable {
        /// Stable reason code.
        code: &'static str,
    },
    /// Adapter table could not be decoded safely.
    #[error("worker store integrity: {code}")]
    StoreIntegrity {
        /// Stable reason code.
        code: &'static str,
    },
    /// Clock or lease arithmetic left the representable range.
    #[error("worker time overflow")]
    TimeOverflow,
    /// No ports factory is registered for the parked workflow kind.
    #[error("unknown workflow kind: {kind}")]
    UnknownKind {
        /// The kind recorded at park time.
        kind: Arc<str>,
    },
    /// The session has no classified wait to park on.
    #[error("session is not parked on a wait")]
    NotParked,
    /// Underlying workflow driver failure.
    #[error(transparent)]
    Driver(#[from] WorkflowDriverError),
    /// Underlying adapter cron failure.
    #[error(transparent)]
    Cron(#[from] CronError),
}

impl WorkerError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::StoreUnavailable { code } | Self::StoreIntegrity { code } => code,
            Self::TimeOverflow => "time_overflow",
            Self::UnknownKind { .. } => "unknown_workflow_kind",
            Self::NotParked => "not_parked",
            Self::Driver(error) => error.code(),
            Self::Cron(error) => error.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            WorkerError::StoreUnavailable { code: "x" }.code(),
            "x"
        );
        assert_eq!(WorkerError::TimeOverflow.code(), "time_overflow");
        assert_eq!(WorkerError::NotParked.code(), "not_parked");
    }
}
