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
/// Lockfile JSON, version, or path is invalid.
pub const PLUGIN_LOCK_INVALID: &str = "plugin_lock_invalid";
/// Two lockfile entries share an identity.
pub const PLUGIN_LOCK_DUPLICATE: &str = "plugin_lock_duplicate";
/// A disabled lock entry was asked to load.
pub const PLUGIN_LOCK_DISABLED: &str = "plugin_lock_disabled";
/// Component bytes or parsed manifest digest do not match the lock.
pub const PLUGIN_LOCK_DIGEST_MISMATCH: &str = "plugin_lock_digest_mismatch";
/// Lockfile, component, or manifest path is missing.
pub const PLUGIN_LOCK_NOT_FOUND: &str = "plugin_lock_not_found";
/// Guest-supplied failure, namespaced so it can never impersonate a host code.
pub const PLUGIN_GUEST_ERROR: &str = "plugin_guest_error";
/// Lockfile JSON, version, or path is invalid.
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
    /// The guest returned a `plugin-error`. Codes in the host-reserved
    /// `plugin_` namespace are demoted under [`PLUGIN_GUEST_ERROR`] with the
    /// original text kept in the message; other guest domain codes pass
    /// through verbatim, so a guest can never impersonate e.g.
    /// `plugin_signature_untrusted`.
    #[error("{code}: {message}")]
    Guest {
        /// Guest-supplied non-secret failure code.
        code: String,
        /// Guest-supplied non-secret explanation.
        message: String,
    },
    /// Host-side manifest, catalog, or payload mapping failed. The message
    /// always begins with a stable code emitted by this host.
    #[error("{0}")]
    Mapped(String),
    /// Lockfile JSON, version, or path failed closed.
    #[error("plugin_lock_invalid: {0}")]
    LockInvalid(String),
    /// Duplicate identity in one lockfile.
    #[error("plugin_lock_duplicate")]
    LockDuplicate,
    /// Caller asked to load a disabled lock entry.
    #[error("plugin_lock_disabled")]
    LockDisabled,
    /// Component or manifest digest did not match the lock pin.
    #[error("plugin_lock_digest_mismatch: {0}")]
    LockDigestMismatch(String),
    /// Lockfile or a locked path was missing.
    #[error("plugin_lock_not_found")]
    LockNotFound,
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
            Self::Guest { .. } => PLUGIN_GUEST_ERROR,
            Self::LockInvalid(_) => PLUGIN_LOCK_INVALID,
            Self::LockDuplicate => PLUGIN_LOCK_DUPLICATE,
            Self::LockDisabled => PLUGIN_LOCK_DISABLED,
            Self::LockDigestMismatch(_) => PLUGIN_LOCK_DIGEST_MISMATCH,
            Self::LockNotFound => PLUGIN_LOCK_NOT_FOUND,
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
        PLUGIN_LOCK_DIGEST_MISMATCH, PLUGIN_LOCK_DISABLED, PLUGIN_LOCK_DUPLICATE,
        PLUGIN_LOCK_INVALID, PLUGIN_LOCK_NOT_FOUND, PLUGIN_PERMISSION_DENIED,
        PLUGIN_RESOURCE_LIMIT, PLUGIN_SIGNATURE_UNTRUSTED, PLUGIN_TRAP, PluginHostError,
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
        assert_eq!(
            PluginHostError::LockInvalid("scheme".into()).code(),
            PLUGIN_LOCK_INVALID
        );
        assert_eq!(PluginHostError::LockDuplicate.code(), PLUGIN_LOCK_DUPLICATE);
        assert_eq!(PluginHostError::LockDisabled.code(), PLUGIN_LOCK_DISABLED);
        assert_eq!(
            PluginHostError::LockDigestMismatch("component".into()).code(),
            PLUGIN_LOCK_DIGEST_MISMATCH
        );
        assert_eq!(PluginHostError::LockNotFound.code(), PLUGIN_LOCK_NOT_FOUND);
    }
}
