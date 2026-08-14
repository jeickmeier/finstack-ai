//! Process-local health and build metadata for the wasm package.

/// Lockstep workspace version exposed to JavaScript consumers.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Binding implementation identity. This is the published package, not the
/// current rustc host target.
pub const IMPLEMENTATION: &str = "wasm";

/// Published compilation target for the npm package.
pub const TARGET: &str = "wasm32-unknown-unknown";

/// Version metadata returned by [`build_metadata`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildMetadata {
    /// Package version string.
    pub version: &'static str,
    /// Engine version string; lockstep with [`Self::version`].
    pub engine_version: &'static str,
    /// Binding implementation identity.
    pub implementation: &'static str,
    /// Published compilation target.
    pub target: &'static str,
}

/// Return the process-local health token.
///
/// This function does not create a runtime, open a store, or spawn work.
#[must_use]
pub const fn health() -> &'static str {
    "ok"
}

/// Return lockstep version metadata for the wasm package.
///
/// This function does not create a runtime, open a store, or spawn work.
#[must_use]
pub const fn build_metadata() -> BuildMetadata {
    BuildMetadata {
        version: ENGINE_VERSION,
        engine_version: ENGINE_VERSION,
        implementation: IMPLEMENTATION,
        target: TARGET,
    }
}

#[cfg(test)]
mod tests {
    use super::{ENGINE_VERSION, build_metadata, health};

    #[test]
    fn health_is_ok_without_side_effects() {
        assert_eq!(health(), "ok");
        let metadata = build_metadata();
        assert_eq!(metadata.version, ENGINE_VERSION);
        assert_eq!(metadata.engine_version, ENGINE_VERSION);
        assert_eq!(metadata.implementation, "wasm");
        assert_eq!(metadata.target, "wasm32-unknown-unknown");
    }
}
