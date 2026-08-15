//! Wasmtime bindgen for the checked-in experimental `@0.0.4` worlds.
//!
//! Guest exports are async so a cancelled call can leave guest code through
//! epoch interruption. Host imports stay synchronous.

/// Bindings for `toolset-plugin`.
pub mod toolset {
    wasmtime::component::bindgen!({
        world: "toolset-plugin",
        path: "wit",
        exports: { default: async },
    });
}

/// Bindings for `context-plugin`. Shared types come from the toolset bindgen.
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
