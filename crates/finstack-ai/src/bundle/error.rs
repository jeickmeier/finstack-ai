//! Bundle, catalog, and lock resolution errors.

use std::sync::Arc;

use thiserror::Error;

use super::{
    BUNDLE_RESOLUTION_CONFLICT, BUNDLE_RESOLUTION_INVALID, BUNDLE_RESOLUTION_LOCK_MISMATCH,
    BUNDLE_RESOLUTION_MISSING,
};

/// Finite bundle/catalog/lock resolution failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BundleResolutionError {
    /// Malformed or unsupported specification/lock.
    #[error("{}: {message}", BUNDLE_RESOLUTION_INVALID)]
    Invalid {
        /// Stable non-secret diagnostic.
        message: Arc<str>,
    },
    /// Required exact identity or service is absent.
    #[error("{}: missing {item}", BUNDLE_RESOLUTION_MISSING)]
    Missing {
        /// Missing non-secret identity.
        item: Arc<str>,
    },
    /// Finite requirement/conflict failed.
    #[error("{}: conflicting {item}", BUNDLE_RESOLUTION_CONFLICT)]
    Conflict {
        /// Conflicting non-secret identity.
        item: Arc<str>,
    },
    /// Imported lock could not be reconstructed exactly.
    #[error("{}: {message}", BUNDLE_RESOLUTION_LOCK_MISMATCH)]
    LockMismatch {
        /// Stable non-secret diagnostic.
        message: Arc<str>,
    },
}

impl BundleResolutionError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => BUNDLE_RESOLUTION_INVALID,
            Self::Missing { .. } => BUNDLE_RESOLUTION_MISSING,
            Self::Conflict { .. } => BUNDLE_RESOLUTION_CONFLICT,
            Self::LockMismatch { .. } => BUNDLE_RESOLUTION_LOCK_MISMATCH,
        }
    }
}
