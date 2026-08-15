//! Host-owned fuel, memory, table, and instance ceilings.

use finstack_ai_wit::PluginManifest;
use wasmtime::{StoreLimits, StoreLimitsBuilder};

/// Experimental host defaults. Not a published SLA.
pub const DEFAULT_FUEL: u64 = 1_000_000;
/// 16 MiB linear memory.
pub const DEFAULT_MEMORY_BYTES: usize = 16 * 1024 * 1024;
/// One table per store.
pub const DEFAULT_TABLES: usize = 1;
/// One instance per store. Distinct from `max_concurrent_instances`.
pub const DEFAULT_INSTANCES: usize = 1;
/// Five seconds when the manifest omits `resource_limits`.
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 5_000;

/// Resolved ceilings applied to one store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveLimits {
    /// Fuel units granted to the store.
    pub fuel: u64,
    /// Linear-memory ceiling in bytes.
    pub max_memory_bytes: usize,
    /// Per-store table ceiling.
    pub max_tables: usize,
    /// Per-store instance ceiling.
    pub max_instances: usize,
    /// Call timeout used to populate construction/call deadlines.
    pub call_timeout_ms: u64,
}

impl Default for EffectiveLimits {
    fn default() -> Self {
        Self {
            fuel: DEFAULT_FUEL,
            max_memory_bytes: DEFAULT_MEMORY_BYTES,
            max_tables: DEFAULT_TABLES,
            max_instances: DEFAULT_INSTANCES,
            call_timeout_ms: DEFAULT_CALL_TIMEOUT_MS,
        }
    }
}

/// Merge manifest optional fields onto host defaults.
#[must_use]
pub fn effective_limits(manifest: &PluginManifest, defaults: EffectiveLimits) -> EffectiveLimits {
    let Some(limits) = manifest.resource_limits.as_ref() else {
        return defaults;
    };
    EffectiveLimits {
        fuel: limits.fuel.unwrap_or(defaults.fuel),
        max_memory_bytes: limits
            .max_memory_bytes
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(defaults.max_memory_bytes),
        max_tables: limits
            .max_tables
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(defaults.max_tables),
        max_instances: limits
            .max_instances
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(defaults.max_instances),
        call_timeout_ms: limits.call_timeout_ms,
    }
}

/// Build Wasmtime store limits from the effective ceilings.
#[must_use]
pub fn store_limits(limits: EffectiveLimits) -> StoreLimits {
    StoreLimitsBuilder::new()
        .memory_size(limits.max_memory_bytes)
        .tables(limits.max_tables)
        .instances(limits.max_instances)
        .memories(1)
        .trap_on_grow_failure(true)
        .build()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CALL_TIMEOUT_MS, DEFAULT_FUEL, effective_limits};
    use finstack_ai_wit::{manifest_digest_hex, parse_manifest};

    #[test]
    fn omitted_resource_limits_use_host_defaults() {
        let identity = "finstack.plugin.echo.toolset";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json");
        let manifest = parse_manifest(&bytes).expect("manifest");
        let effective = effective_limits(&manifest, super::EffectiveLimits::default());
        assert_eq!(effective.fuel, DEFAULT_FUEL);
        assert_eq!(effective.call_timeout_ms, DEFAULT_CALL_TIMEOUT_MS);
    }
}
