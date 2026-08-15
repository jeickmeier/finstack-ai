//! Experimental `@0.0.4` WIT types, host imports, and toolset guest bindings.
//!
//! Bindings are generated from the checked-in WIT packages by
//! `tools/wit_bindgen/generate.py`. This crate maps those values onto native
//! runtime types in-process. It does not instantiate Wasmtime.

#![warn(missing_docs)]

pub mod error;
pub mod generated;
pub mod host;
pub mod inventory;
pub mod limits;
pub mod mapping;
pub mod reference;

pub use error::WitMapError;
pub use generated::{
    BlobRef, CRATE_VERSION, CallContext, GuestToolset, HOST_IMPORTS, HostBlobs, HostLogging, Level,
    PluginError, TOOLSET_FUNCS, TOOLSET_WORLD_EXPORTS, ToolCatalog, ToolResult, ToolSpec,
};
pub use host::{CeilingBlobStore, RecordingLogger};
pub use inventory::assert_experimental_surface;
pub use limits::{
    MAX_METADATA_BYTES, MAX_RAW_JSON_BYTES, MAX_STRING_BYTES, reject_before_allocation,
    reject_declared_len,
};
pub use mapping::{catalog_digest_hex, map_tool_spec, register_catalog, sanitize_call_context};
pub use reference::ReferenceToolset;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::{TOOLSET_FUNCS, TOOLSET_WORLD_EXPORTS, assert_experimental_surface};

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
    }

    #[test]
    fn compatibility_fixtures_document_exact_world_rules() {
        let root = repo_root().join("fixtures/compatibility/wit/v0.0.4");
        let packages =
            std::fs::read_to_string(root.join("packages/valid--published-packages.json"))
                .expect("packages fixture");
        assert!(packages.contains("finstack:ai-types@0.0.4"));
        let blocked =
            std::fs::read_to_string(root.join("packages/invalid--v1-package-blocked.wit"))
                .expect("v1 fixture");
        assert!(blocked.contains("@1.0.0"));
        let world = std::fs::read_to_string(root.join("world/valid--toolset-exports.json"))
            .expect("world fixture");
        assert!(world.contains("list-tools"));
        let nested = std::fs::read_to_string(root.join("world/invalid--nested-agent-export.wit"))
            .expect("nested fixture");
        assert!(nested.contains("export agent"));
        let context =
            std::fs::read_to_string(root.join("call-context/roundtrip--sanitized-fields.json"))
                .expect("context fixture");
        assert!(context.contains("authorization-decision-id"));
        assert!(context.contains("\"attempt\""));
        assert!(context.contains("\"never\""));
    }
}
