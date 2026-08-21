use finstack_ai_workflow_worker::WorkerError;
use thiserror::Error;

/// Errors surfaced by the HITL router battery.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HitlError {
    /// Backing store rejected an operation.
    #[error("hitl store unavailable: {code}")]
    StoreUnavailable {
        /// Stable reason code.
        code: &'static str,
    },
    /// Persisted row failed validation on read.
    #[error("hitl store integrity: {code}")]
    StoreIntegrity {
        /// Stable reason code.
        code: &'static str,
    },
    /// No inbox row for the requested interaction.
    #[error("unknown interaction")]
    UnknownInteraction,
    /// Row exists but is not resolvable (buffered, accepted, or closed).
    #[error("interaction is not open")]
    NotOpen,
    /// Authorizer refused the principal.
    #[error("resolution not authorized: {code}")]
    Unauthorized {
        /// Stable reason code.
        code: &'static str,
    },
    /// Resolution inputs failed kernel construction.
    #[error("invalid resolution: {code}")]
    InvalidResolution {
        /// Stable reason code.
        code: &'static str,
    },
    /// Worker delivery failed.
    #[error(transparent)]
    Worker(#[from] WorkerError),
}

impl HitlError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::StoreUnavailable { code }
            | Self::StoreIntegrity { code }
            | Self::Unauthorized { code }
            | Self::InvalidResolution { code } => code,
            Self::UnknownInteraction => "unknown_interaction",
            Self::NotOpen => "not_open",
            Self::Worker(error) => error.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(HitlError::UnknownInteraction.code(), "unknown_interaction");
        assert_eq!(HitlError::NotOpen.code(), "not_open");
        assert_eq!(
            HitlError::Unauthorized {
                code: "tenant_mismatch"
            }
            .code(),
            "tenant_mismatch"
        );
        assert_eq!(HitlError::from(WorkerError::NotParked).code(), "not_parked");
    }
}
