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
/// Requested permission is not in the host grant set.
pub const PLUGIN_PERMISSION_DENIED: &str = "plugin_permission_denied";
/// Fuel, memory, table, or instance exhaustion.
pub const PLUGIN_RESOURCE_LIMIT: &str = "plugin_resource_limit";
/// Unsigned or untrusted package under the configured signature policy.
pub const PLUGIN_SIGNATURE_UNTRUSTED: &str = "plugin_signature_untrusted";

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
    /// Manifest requested a capability the host does not offer.
    #[error("plugin_permission_denied: {0}")]
    PermissionDenied(String),
    /// Fuel or store-limiter exhaustion contained without aborting the host.
    #[error("plugin_resource_limit: {0}")]
    ResourceLimit(String),
    /// Signature policy rejected the package.
    #[error("plugin_signature_untrusted: {0}")]
    SignatureUntrusted(String),
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
    pub fn code(&self) -> &str {
        match self {
            Self::CompileFailed(_) => PLUGIN_COMPILE_FAILED,
            Self::InstantiateFailed(_) => PLUGIN_INSTANTIATE_FAILED,
            Self::Trap(_) => PLUGIN_TRAP,
            Self::InstanceLimit => PLUGIN_INSTANCE_LIMIT,
            Self::Timeout => "plugin_lifecycle_timeout",
            Self::PermissionDenied(_) => PLUGIN_PERMISSION_DENIED,
            Self::ResourceLimit(_) => PLUGIN_RESOURCE_LIMIT,
            Self::SignatureUntrusted(_) => PLUGIN_SIGNATURE_UNTRUSTED,
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

fn mapped_code(message: &str) -> &str {
    if let Some(code) = message
        .split_once(':')
        .map(|(code, _)| code)
        .filter(|code| is_stable_code(code))
    {
        return code;
    }
    if is_stable_code(message) {
        return message;
    }
    "plugin_registration_invalid"
}

fn is_stable_code(code: &str) -> bool {
    !code.is_empty()
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::{
        PLUGIN_PERMISSION_DENIED, PLUGIN_RESOURCE_LIMIT, PLUGIN_SIGNATURE_UNTRUSTED, PLUGIN_TRAP,
        PluginHostError,
    };

    #[test]
    fn trap_limit_and_policy_codes_are_stable() {
        assert_eq!(
            PluginHostError::Trap("unreachable".into()).code(),
            PLUGIN_TRAP
        );
        assert_eq!(
            PluginHostError::InstanceLimit.code(),
            "plugin_instance_limit"
        );
        assert_eq!(PluginHostError::Timeout.code(), "plugin_lifecycle_timeout");
        assert_eq!(
            PluginHostError::PermissionDenied("filesystem".into()).code(),
            PLUGIN_PERMISSION_DENIED
        );
        assert_eq!(
            PluginHostError::ResourceLimit("fuel".into()).code(),
            PLUGIN_RESOURCE_LIMIT
        );
        assert_eq!(
            PluginHostError::SignatureUntrusted("unsigned".into()).code(),
            PLUGIN_SIGNATURE_UNTRUSTED
        );
        assert_eq!(
            PluginHostError::Mapped("calculator_arithmetic_error: division_by_zero".into()).code(),
            "calculator_arithmetic_error"
        );
        assert_eq!(
            PluginHostError::Mapped("plugin_catalog_digest_mismatch".into()).code(),
            "plugin_catalog_digest_mismatch"
        );
    }
}
