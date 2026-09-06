//! Wasmtime bindgen for checked-in `@0.0.4` and `@1.0.0` worlds.
//!
//! Guest exports are async so a cancelled call can leave guest code through
//! per-store fuel yields. Host imports stay synchronous.

/// Bindings for experimental `@0.0.4` `toolset-plugin`.
pub mod toolset {
    wasmtime::component::bindgen!({
        world: "toolset-plugin",
        path: "wit",
        exports: { default: async },
    });
}

/// Bindings for experimental `@0.0.4` `context-plugin`.
pub mod context {
    wasmtime::component::bindgen!({
        world: "context-plugin",
        path: "wit",
        exports: { default: async },
        with: {
            "finstack:ai-types/types": super::toolset::finstack::ai_types::types,
            "finstack:ai-host/logging": super::toolset::finstack::ai_host::logging,
            "finstack:ai-host/blobs": super::toolset::finstack::ai_host::blobs,
        },
    });
}

/// Bindings for frozen `@1.0.0` worlds.
pub mod v1 {
    /// Bindings for `@1.0.0` `toolset-plugin`.
    pub mod toolset {
        wasmtime::component::bindgen!({
            world: "toolset-plugin",
            path: "wit/v1.0.0",
            exports: { default: async },
        });
    }

    /// Bindings for `@1.0.0` `context-plugin`.
    pub mod context {
        wasmtime::component::bindgen!({
            world: "context-plugin",
            path: "wit/v1.0.0",
            exports: { default: async },
            with: {
                "finstack:ai-types/types": super::toolset::finstack::ai_types::types,
                "finstack:ai-host/logging": super::toolset::finstack::ai_host::logging,
                "finstack:ai-host/blobs": super::toolset::finstack::ai_host::blobs,
            },
        });
    }
}
