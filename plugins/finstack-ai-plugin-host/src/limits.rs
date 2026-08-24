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

// Host-owned ceilings. A manifest's `resource_limits` may lower a limit but
// never raise one past these, because the manifest is untrusted package data:
// `validate_wire` rejects only zero values, and neither the manifest digest nor
// the ed25519 signature covers `resource_limits`, so those fields can be
// rewritten by anyone who can edit the file.
//
// These are deliberately NOT the `DEFAULT_*` values above. Defaults are what a
// manifest gets for saying nothing; ceilings are the most it may ask for.
// Shipped manifests already request more than the defaults — the
// filesystem-sandbox reference asks for 16 tables and 16 instances against
// defaults of 1 — so clamping to the defaults would break them.

/// Largest fuel budget a manifest may request.
pub const CEILING_FUEL: u64 = 1_000_000_000;
/// Largest linear-memory ceiling a manifest may request (256 MiB).
pub const CEILING_MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// Largest per-store table count a manifest may request.
pub const CEILING_TABLES: usize = 64;
/// Largest per-store instance count a manifest may request.
pub const CEILING_INSTANCES: usize = 64;
/// Longest call timeout a manifest may request (60s).
pub const CEILING_CALL_TIMEOUT_MS: u64 = 60_000;

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

#[cfg(test)]
impl EffectiveLimits {
    /// Compile-tolerant host default for crate tests.
    ///
    /// First instantiate under parallel `cargo test --workspace` load can
    /// exceed [`DEFAULT_CALL_TIMEOUT_MS`] and get reported as timeout.
    #[must_use]
    pub const fn for_tests() -> Self {
        Self {
            fuel: DEFAULT_FUEL,
            max_memory_bytes: DEFAULT_MEMORY_BYTES,
            max_tables: DEFAULT_TABLES,
            max_instances: DEFAULT_INSTANCES,
            call_timeout_ms: 60_000,
        }
    }
}

/// Merge manifest optional fields onto host defaults, clamped to host ceilings.
///
/// An omitted field takes the host default. A present field is honoured only up
/// to the matching `CEILING_*` constant: the manifest may lower a limit, never
/// raise one past what this host permits.
#[must_use]
pub fn effective_limits(manifest: &PluginManifest, defaults: EffectiveLimits) -> EffectiveLimits {
    let Some(limits) = manifest.resource_limits.as_ref() else {
        return defaults;
    };
    EffectiveLimits {
        fuel: limits
            .fuel
            .map_or(defaults.fuel, |value| value.min(CEILING_FUEL)),
        max_memory_bytes: limits
            .max_memory_bytes
            .and_then(|value| usize::try_from(value).ok())
            .map_or(defaults.max_memory_bytes, |value| {
                value.min(CEILING_MEMORY_BYTES)
            }),
        max_tables: limits
            .max_tables
            .and_then(|value| usize::try_from(value).ok())
            .map_or(defaults.max_tables, |value| value.min(CEILING_TABLES)),
        max_instances: limits
            .max_instances
            .and_then(|value| usize::try_from(value).ok())
            .map_or(defaults.max_instances, |value| value.min(CEILING_INSTANCES)),
        call_timeout_ms: limits.call_timeout_ms.min(CEILING_CALL_TIMEOUT_MS),
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
    use super::{
        CEILING_CALL_TIMEOUT_MS, CEILING_FUEL, CEILING_INSTANCES, CEILING_MEMORY_BYTES,
        CEILING_TABLES, DEFAULT_CALL_TIMEOUT_MS, DEFAULT_FUEL, effective_limits,
    };
    use finstack_ai_wit::{manifest_digest_hex, parse_manifest};

    fn manifest_with_limits(limits: &serde_json::Value) -> finstack_ai_wit::PluginManifest {
        let identity = "finstack.plugin.echo.toolset";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds, &["logging".to_owned()])
            .expect("digest");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "resource_limits": limits,
            "digest": digest
        }))
        .expect("json");
        parse_manifest(&bytes).expect("manifest")
    }

    #[test]
    fn a_manifest_cannot_raise_a_limit_past_the_host_ceiling() {
        // `resource_limits` is outside both the manifest digest and the
        // signature, so an untrusted package can name any value it likes.
        // Values stay under 2^53 because the manifest wire format rejects
        // integers a JS-safe double cannot represent; they are still a total
        // escalation past defaults of 1M fuel and 16 MiB.
        let manifest = manifest_with_limits(&serde_json::json!({
            "max_output_bytes": 1024,
            "call_timeout_ms": 86_400_000_u64,
            "fuel": 9_000_000_000_000_u64,
            "max_memory_bytes": 4_294_967_296_u64,
            "max_tables": 100_000_u64,
            "max_instances": 100_000_u64,
        }));
        let effective = effective_limits(&manifest, super::EffectiveLimits::default());
        assert_eq!(effective.fuel, CEILING_FUEL);
        assert_eq!(effective.max_memory_bytes, CEILING_MEMORY_BYTES);
        assert_eq!(effective.max_tables, CEILING_TABLES);
        assert_eq!(effective.max_instances, CEILING_INSTANCES);
        assert_eq!(effective.call_timeout_ms, CEILING_CALL_TIMEOUT_MS);
    }

    #[test]
    fn a_manifest_may_still_lower_a_limit() {
        let manifest = manifest_with_limits(&serde_json::json!({
            "max_output_bytes": 1024,
            "call_timeout_ms": 1_000,
            "fuel": 8_000,
            "max_memory_bytes": 65_536,
            "max_tables": 2,
            "max_instances": 2,
        }));
        let effective = effective_limits(&manifest, super::EffectiveLimits::default());
        assert_eq!(effective.fuel, 8_000);
        assert_eq!(effective.max_memory_bytes, 65_536);
        assert_eq!(effective.max_tables, 2);
        assert_eq!(effective.max_instances, 2);
        assert_eq!(effective.call_timeout_ms, 1_000);
    }

    #[test]
    fn shipped_reference_manifest_values_survive_the_clamp() {
        // filesystem-sandbox ships max_tables/max_instances of 16 against
        // defaults of 1; the ceiling must not be the default.
        let manifest = manifest_with_limits(&serde_json::json!({
            "max_output_bytes": 65_536,
            "call_timeout_ms": 5_000,
            "max_tables": 16,
            "max_instances": 16,
        }));
        let effective = effective_limits(&manifest, super::EffectiveLimits::default());
        assert_eq!(effective.max_tables, 16);
        assert_eq!(effective.max_instances, 16);
        assert_eq!(effective.call_timeout_ms, 5_000);
        assert_eq!(
            effective.fuel, DEFAULT_FUEL,
            "omitted field keeps the default"
        );
    }

    #[test]
    fn omitted_resource_limits_use_host_defaults() {
        let identity = "finstack.plugin.echo.toolset";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds, &["logging".to_owned()])
            .expect("digest");
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
