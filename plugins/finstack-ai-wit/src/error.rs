//! Fail-closed mapping and payload errors for the experimental WIT surface.

use thiserror::Error;

/// Stable registration/mapping failure for dual-major `@0.0.4` / `@1.0.0` WIT values.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WitMapError {
    /// A payload exceeded its contract section 6.5 ceiling before a copy was allocated.
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
    /// Manifest identity, world, permission, or signature metadata is invalid.
    #[error("plugin_registration_invalid: {0}")]
    ManifestInvalid(&'static str),
    /// Guest manifest digest did not match the host-computed digest.
    #[error("plugin_manifest_digest_mismatch")]
    ManifestDigestMismatch,
    /// A context item failed native schema, attribution, or budget checks.
    #[error("plugin_context_item_invalid: {0}")]
    ContextItemInvalid(&'static str),
    /// Item JSON encoded a private suspension or nested-agent protocol.
    #[error("plugin_private_suspension")]
    PrivateSuspension,
}

impl WitMapError {
    /// Stable error code for host diagnostics.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PayloadTooLarge { .. } => "plugin_payload_too_large",
            Self::RegistrationInvalid(_) | Self::ManifestInvalid(_) => {
                "plugin_registration_invalid"
            }
            Self::CatalogDigestMismatch => "plugin_catalog_digest_mismatch",
            Self::ToolSpecInvalid(_) => "plugin_tool_spec_invalid",
            Self::ManifestDigestMismatch => "plugin_manifest_digest_mismatch",
            Self::ContextItemInvalid(_) => "plugin_context_item_invalid",
            Self::PrivateSuspension => "plugin_private_suspension",
        }
    }
}
