//! Isolated Wasmtime (T3) component host and compiled-component cache.
//!
//! This crate is the first real isolated plugin leaf. It does not inherit host
//! process authority the way in-process `finstack-ai-wit` adapters do. Those
//! in-process guests are not a sandbox. Constructing [`PluginHost`] is the
//! opt-in; the default SDK bundle does not depend on this crate.
//!
//! Registration still uses [`finstack_ai::ExtensionDescriptor::trusted_in_process`]
//! because [`finstack_ai::ExtensionTrust`] has no isolated variant in this
//! preview. Isolation is a property of this host, not of that descriptor.

#![warn(missing_docs)]

mod adapters;
mod bindings;
mod cache;
mod convert;
mod error;
mod host;
mod instantiate;

pub use adapters::{WasmContextAdapter, WasmPluginExtension, WasmToolsetAdapter};
pub use cache::{
    CacheKeyParts, abi_identity, cache_key, component_digest, engine_fingerprint, host_target,
};
pub use error::{
    PLUGIN_COMPILE_FAILED, PLUGIN_INSTANCE_LIMIT, PLUGIN_INSTANTIATE_FAILED, PLUGIN_TRAP,
    PluginHostError,
};
pub use host::{InstancePolicy, PluginHost, PluginHostConfig, PluginWorld, ReadyWasm};
pub use instantiate::HostState;

#[cfg(test)]
mod fixture_tests;

#[cfg(test)]
mod graph_tests {
    use std::path::PathBuf;
    use std::process::Command;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn tree(args: &[&str]) -> String {
        let output = Command::new("cargo")
            .args(args)
            .current_dir(repo_root())
            .output()
            .expect("cargo tree");
        assert!(
            output.status.success(),
            "cargo tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf8")
    }

    fn assert_bundle_free(tree: &str) {
        assert!(
            !tree.contains("wasmtime"),
            "default bundle must not depend on wasmtime:\n{tree}"
        );
        assert!(
            !tree.contains("finstack-ai-plugin-host"),
            "default bundle must not depend on finstack-ai-plugin-host:\n{tree}"
        );
    }

    #[test]
    fn default_bundles_remain_wasmtime_free() {
        assert_bundle_free(&tree(&[
            "tree",
            "-p",
            "finstack-ai-kernel",
            "-p",
            "finstack-ai-runtime",
            "-p",
            "finstack-ai",
            "--locked",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--edges",
            "normal",
        ]));
        assert_bundle_free(&tree(&[
            "tree",
            "-p",
            "finstack-ai",
            "--locked",
            "--no-default-features",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--edges",
            "normal",
        ]));
        let wit = tree(&[
            "tree",
            "-p",
            "finstack-ai-wit",
            "-p",
            "finstack-ai-native-examples",
            "--locked",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--edges",
            "normal",
        ]);
        assert!(
            !wit.contains("wasmtime"),
            "wit/examples must not depend on wasmtime:\n{wit}"
        );
        assert!(
            !wit.contains("finstack-ai-plugin-host"),
            "wit/examples must not depend on finstack-ai-plugin-host:\n{wit}"
        );
        let host = tree(&[
            "tree",
            "-p",
            "finstack-ai-plugin-host",
            "--locked",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--edges",
            "normal",
        ]);
        assert!(
            host.contains("wasmtime"),
            "plugin-host must depend on wasmtime"
        );
        assert!(
            !host.contains("wasmtime-wasi "),
            "plugin-host must not depend on wasmtime-wasi:\n{host}"
        );
        assert!(
            !host.contains("libloading"),
            "plugin-host must not depend on libloading:\n{host}"
        );
    }
}
