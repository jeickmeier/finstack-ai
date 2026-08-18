//! Shared remote-child route arguments for native routing and wasm-host stubs.

/// Explicit remote route forwarded by language bindings.
///
/// Native construction builds [`finstack_ai_remote_child::RemoteChildInvoker`].
/// wasm-host methods exist and return [`crate::AGENT_RUN_UNSUPPORTED_PLAN`].
pub struct RemoteChildRouteSpec {
    /// Loopback `host:port` or `unix:/absolute/path`.
    pub endpoint: String,
    /// Remote service component id.
    pub service: String,
    /// Opaque non-secret route handle label.
    pub route: String,
    /// Optional Bearer token. Never read from the environment.
    pub token: Option<String>,
}
