//! Isolated Wasmtime (T3) component host and compiled-component cache.
//!
//! This crate is the first real isolated plugin leaf. It does not inherit host
//! process authority the way in-process `finstack-ai-wit` adapters do. Those
//! in-process guests are not a sandbox. Constructing [`PluginHost`] is the
//! opt-in; the default SDK bundle does not depend on this crate.
//!
//! WASI is deny-by-default: filesystem and network imports are not linked
//! unless the host offers the grant and the concrete resource, and preopen
//! host paths are validated fail-closed against traversal and sensitive
//! roots. Fuel and store limits contain exhaustion. Signature policy is host
//! configuration and defaults to [`SignaturePolicy::Strict`];
//! [`SignaturePolicy::Permissive`] is an explicit opt-in for
//! development/fixtures only.
//!
//! Registration still uses [`finstack_ai::ExtensionDescriptor::trusted_in_process`]
//! because [`finstack_ai::ExtensionTrust`] has no isolated variant in this
//! preview. Isolation is a property of this host, not of that descriptor.

#![warn(missing_docs)]
// Wasmtime component deserialize is an unsafe host boundary.
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

mod adapters;
mod bindings;
mod cache;
mod convert;
mod error;
mod grants;
mod host;
mod instantiate;
mod limits;
mod lockfile;
mod signature;

pub use adapters::{WasmContextAdapter, WasmPluginExtension, WasmToolsetAdapter};
pub use cache::{
    CacheKeyParts, abi_identity, cache_key, component_digest, engine_fingerprint, host_target,
};
pub use error::{
    PLUGIN_COMPILE_FAILED, PLUGIN_INSTANCE_LIMIT, PLUGIN_INSTANTIATE_FAILED,
    PLUGIN_LOCK_DIGEST_MISMATCH, PLUGIN_LOCK_DISABLED, PLUGIN_LOCK_DUPLICATE, PLUGIN_LOCK_INVALID,
    PLUGIN_LOCK_NOT_FOUND, PLUGIN_PERMISSION_DENIED, PLUGIN_RESOURCE_LIMIT,
    PLUGIN_SIGNATURE_UNTRUSTED, PLUGIN_TRAP, PluginHostError,
};
pub use grants::{FilesystemPreopen, GrantResources, validate_preopen_host_path};
pub use host::{InstancePolicy, PluginHost, PluginHostConfig, PluginWorld, ReadyWasm};
pub use instantiate::HostState;
pub use limits::EffectiveLimits;
pub use lockfile::{LockedPlugin, ResolvedPluginLock, resolve_lockfile};
pub use signature::SignaturePolicy;

#[cfg(test)]
mod conformance_tests;

#[cfg(test)]
mod fixture_tests;

#[cfg(test)]
mod reference_tests;

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
        let default_tree = tree(&[
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
        ]);
        assert_bundle_free(&default_tree);
        assert!(
            !has_package(&default_tree, "finstack-ai-guest-sdk"),
            "default bundle must not depend on finstack-ai-guest-sdk:\n{default_tree}"
        );
        let minimal = tree(&[
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
        ]);
        assert_bundle_free(&minimal);
        assert!(
            !has_package(&minimal, "finstack-ai-guest-sdk"),
            "minimal bundle must not depend on finstack-ai-guest-sdk:\n{minimal}"
        );
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
        assert!(
            !has_package(&wit, "finstack-ai-guest-sdk"),
            "wit/examples must not depend on finstack-ai-guest-sdk:\n{wit}"
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
            host.contains("wasmtime-wasi"),
            "plugin-host must depend on wasmtime-wasi:\n{host}"
        );
        assert!(
            !host.contains("libloading"),
            "plugin-host must not depend on libloading:\n{host}"
        );
        assert!(
            !has_package(&host, "finstack-ai-guest-sdk"),
            "plugin-host must not depend on finstack-ai-guest-sdk:\n{host}"
        );
        assert_host_has_no_discovery_clients();
    }

    fn assert_host_has_no_discovery_clients() {
        let direct = tree(&[
            "tree",
            "-p",
            "finstack-ai-plugin-host",
            "--locked",
            "--depth",
            "1",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--edges",
            "normal",
        ]);
        for forbidden in ["reqwest", "ureq", "hyper", "libloading"] {
            assert!(
                !has_package(&direct, forbidden),
                "plugin-host must not take {forbidden} as a direct dependency:\n{direct}"
            );
        }
    }

    fn has_package(tree: &str, name: &str) -> bool {
        tree.lines().any(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|package| package == name)
        })
    }
}
