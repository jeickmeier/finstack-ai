//! Fail-closed mapping and payload errors for the experimental WIT surface.

use thiserror::Error;

/// Stable registration/mapping failure for experimental `@0.0.4` WIT values.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WitMapError {
    /// A payload exceeded its TDD §6.5 ceiling before a copy was allocated.
    #[error("plugin_payload_too_large: {field} is {len} bytes; max {max}")]
    PayloadTooLarge {
        /// Field or frame that exceeded its ceiling.
        field: &'static str,
        /// Observed byte length.
        len: usize,
        /// Enforced ceiling.
        max: usize,
    },
    /// A required security or catalog field was empty, omitted, or unknown.
    #[error("plugin_registration_invalid: {0}")]
    RegistrationInvalid(&'static str),
    /// Guest catalog digest did not match the host-computed digest.
    #[error("plugin_catalog_digest_mismatch")]
    CatalogDigestMismatch,
    /// Native tool metadata rejected the mapped spec.
    #[error("plugin_tool_spec_invalid: {0}")]
    ToolSpecInvalid(&'static str),
}

impl WitMapError {
    /// Stable error code for host diagnostics.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PayloadTooLarge { .. } => "plugin_payload_too_large",
            Self::RegistrationInvalid(_) => "plugin_registration_invalid",
            Self::CatalogDigestMismatch => "plugin_catalog_digest_mismatch",
            Self::ToolSpecInvalid(_) => "plugin_tool_spec_invalid",
        }
    }
}
