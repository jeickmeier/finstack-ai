//! Experimental `@0.0.4` WIT types, host imports, and in-process guest bindings.
//!
//! Bindings are generated from the checked-in WIT packages by
//! `tools/wit_bindgen/generate.py`. This crate maps those values onto native
//! runtime types in-process. It does not instantiate Wasmtime.

#![warn(missing_docs)]

pub mod adapters;
pub mod context_mapping;
pub mod error;
pub mod generated;
pub mod host;
pub mod inventory;
pub mod lifecycle;
pub mod limits;
pub mod manifest;
pub mod mapping;
pub mod reference;

pub use adapters::{WitContextAdapter, WitPluginExtension, WitToolsetAdapter};
pub use context_mapping::{encode_guest_item, map_budget, map_context_item, map_query};
pub use error::WitMapError;
pub use generated::{
    AI_CONTEXT_PACKAGE, BlobRef, CONTEXT_FUNCS, CONTEXT_WORLD_EXPORTS, CRATE_VERSION, CallContext,
    ContextBudget as WitContextBudget, ContextItem as WitContextItem, ContextQuery,
    GuestContextProvider, GuestToolset, HOST_IMPORTS, HostBlobs, HostLogging, Level, PluginError,
    TOOLSET_FUNCS, TOOLSET_WORLD_EXPORTS, ToolCatalog, ToolResult, ToolSpec,
};
pub use host::{CeilingBlobStore, RecordingLogger};
pub use inventory::assert_experimental_surface;
pub use lifecycle::{
    NoopPluginHooks, PluginGuestHooks, PluginLifecycle, PluginLifecycleError, honor_deadline,
    merge_call_deadline,
};
pub use limits::{
    MAX_METADATA_BYTES, MAX_RAW_JSON_BYTES, MAX_STRING_BYTES, reject_before_allocation,
    reject_declared_len,
};
pub use manifest::{
    PluginManifest, PluginResourceLimits, PluginSignature, manifest_digest_hex,
    manifest_signing_payload, parse_manifest, reject_duplicate_identities, validate_manifest,
};
pub use mapping::{catalog_digest_hex, map_tool_spec, register_catalog, sanitize_call_context};
pub use reference::{ReferenceContextProvider, ReferenceToolset};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::{
        CONTEXT_FUNCS, CONTEXT_WORLD_EXPORTS, TOOLSET_FUNCS, TOOLSET_WORLD_EXPORTS,
        assert_experimental_surface,
    };

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn generated_bindings_match_checked_in_wit() {
        let status = Command::new("uv")
            .args([
                "run",
                "--no-project",
                "python",
                "tools/wit_bindgen/generate.py",
                "--check",
            ])
            .current_dir(repo_root())
            .status()
            .expect("run check-wit");
        assert!(status.success(), "mise run check-wit equivalent failed");
        assert_experimental_surface().expect("surface");
        assert_eq!(TOOLSET_WORLD_EXPORTS, ["toolset"]);
        assert_eq!(TOOLSET_FUNCS, ["list-tools", "call"]);
        assert_eq!(CONTEXT_WORLD_EXPORTS, ["context-provider"]);
        assert_eq!(CONTEXT_FUNCS, ["collect"]);
    }

    #[test]
    fn compatibility_fixtures_document_exact_world_rules() {
        let root = repo_root().join("fixtures/compatibility/wit/v0.0.4");
        let packages =
            std::fs::read_to_string(root.join("packages/valid--published-packages.json"))
                .expect("packages fixture");
        assert!(packages.contains("finstack:ai-types@0.0.4"));
        assert!(packages.contains("finstack:ai-context@0.0.4"));
        let blocked =
            std::fs::read_to_string(root.join("packages/invalid--v1-package-blocked.wit"))
                .expect("v1 fixture");
        assert!(blocked.contains("@1.0.0"));
        let world = std::fs::read_to_string(root.join("world/valid--toolset-exports.json"))
            .expect("world fixture");
        assert!(world.contains("list-tools"));
        let context_world = std::fs::read_to_string(root.join("world/valid--context-exports.json"))
            .expect("context world fixture");
        assert!(context_world.contains("context-provider"));
        assert!(context_world.contains("collect"));
        let nested = std::fs::read_to_string(root.join("world/invalid--nested-agent-export.wit"))
            .expect("nested fixture");
        assert!(nested.contains("export agent"));
        let middleware =
            std::fs::read_to_string(root.join("world/invalid--undeclared-middleware-world.wit"))
                .expect("middleware fixture");
        assert!(middleware.contains("middleware"));
        let context =
            std::fs::read_to_string(root.join("call-context/roundtrip--sanitized-fields.json"))
                .expect("context fixture");
        assert!(context.contains("authorization-decision-id"));
        assert!(context.contains("\"attempt\""));
        assert!(context.contains("\"never\""));
        let manifest = std::fs::read_to_string(root.join("manifest/valid--context-reference.json"))
            .expect("manifest fixture");
        assert!(manifest.contains("finstack.plugin.reference.context"));
        let elevation =
            std::fs::read_to_string(root.join("item-json/invalid--trusted-self-elevation.json"))
                .expect("elevation fixture");
        assert!(elevation.contains("trusted_application"));
        let private =
            std::fs::read_to_string(root.join("item-json/invalid--private-suspension.json"))
                .expect("private fixture");
        assert!(private.contains("interaction"));
    }
}
