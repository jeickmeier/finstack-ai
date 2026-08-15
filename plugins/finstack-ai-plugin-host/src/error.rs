//! Stable host-side plugin errors. These codes are not WIT exports.

use finstack_ai_wit::{PluginLifecycleError, WitMapError};
use thiserror::Error;

/// Guest trap / `unreachable` / contained guest panic.
pub const PLUGIN_TRAP: &str = "plugin_trap";
/// Link, instantiate, or unresolved-import failure.
pub const PLUGIN_INSTANTIATE_FAILED: &str = "plugin_instantiate_failed";
/// Compile or deserialize mismatch.
pub const PLUGIN_COMPILE_FAILED: &str = "plugin_compile_failed";
/// Concurrent instance ceiling overflow.
pub const PLUGIN_INSTANCE_LIMIT: &str = "plugin_instance_limit";

/// Fail-closed error for the isolated Wasmtime host.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PluginHostError {
    /// Component bytes could not be compiled or a cached artifact was rejected.
    #[error("plugin_compile_failed: {0}")]
    CompileFailed(String),
    /// Instantiation or linking failed, including unresolved `wasi:*` imports.
    #[error("plugin_instantiate_failed: {0}")]
    InstantiateFailed(String),
    /// Guest trap contained without aborting the host process.
    #[error("plugin_trap: {0}")]
    Trap(String),
    /// `max_concurrent_instances` would be exceeded.
    #[error("plugin_instance_limit")]
    InstanceLimit,
    /// Construction or call cancellation/deadline fired.
    #[error("plugin_lifecycle_timeout")]
    Timeout,
    /// Host configuration rejected before the engine was built.
    #[error("plugin_registration_invalid: {0}")]
    ConfigInvalid(&'static str),
    /// Manifest, catalog, or payload mapping failed.
    #[error("{0}")]
    Mapped(String),
}

impl PluginHostError {
    /// Stable diagnostic code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::CompileFailed(_) => PLUGIN_COMPILE_FAILED,
            Self::InstantiateFailed(_) => PLUGIN_INSTANTIATE_FAILED,
            Self::Trap(_) => PLUGIN_TRAP,
            Self::InstanceLimit => PLUGIN_INSTANCE_LIMIT,
            Self::Timeout => "plugin_lifecycle_timeout",
            Self::ConfigInvalid(_) => "plugin_registration_invalid",
            Self::Mapped(message) => mapped_code(message),
        }
    }

    /// Wrap a WIT mapping failure while preserving its stable code.
    #[must_use]
    pub fn from_map(error: &WitMapError) -> Self {
        Self::Mapped(error.to_string())
    }

    /// Wrap a host-side lifecycle failure.
    #[must_use]
    pub fn from_lifecycle(error: &PluginLifecycleError) -> Self {
        match error {
            PluginLifecycleError::Timeout => Self::Timeout,
            other => Self::Mapped(other.to_string()),
        }
    }
}

fn mapped_code(message: &str) -> &'static str {
    if message.starts_with("plugin_payload_too_large") {
        "plugin_payload_too_large"
    } else if message.starts_with("plugin_manifest_digest_mismatch") {
        "plugin_manifest_digest_mismatch"
    } else if message.starts_with("plugin_catalog_digest_mismatch") {
        "plugin_catalog_digest_mismatch"
    } else if message.starts_with("plugin_initialize_failed") {
        "plugin_initialize_failed"
    } else if message.starts_with("plugin_warmup_failed") {
        "plugin_warmup_failed"
    } else if message.starts_with("plugin_lifecycle_failed") {
        "plugin_lifecycle_failed"
    } else if message.starts_with("plugin_lifecycle_timeout") {
        "plugin_lifecycle_timeout"
    } else {
        "plugin_registration_invalid"
    }
}

#[cfg(test)]
mod tests {
    use super::{PLUGIN_TRAP, PluginHostError};

    #[test]
    fn trap_and_limit_codes_are_stable() {
        assert_eq!(
            PluginHostError::Trap("unreachable".into()).code(),
            PLUGIN_TRAP
        );
        assert_eq!(
            PluginHostError::InstanceLimit.code(),
            "plugin_instance_limit"
        );
        assert_eq!(PluginHostError::Timeout.code(), "plugin_lifecycle_timeout");
    }
}
