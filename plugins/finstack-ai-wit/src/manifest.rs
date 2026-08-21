//! Host-side experimental plugin manifest. This is not a WIT record.

use std::collections::BTreeSet;

use finstack_ai_kernel::{ComponentId, Digest, RawJson};
use serde::Deserialize;
use serde_json::Value;

use crate::error::WitMapError;
use crate::limits::{MAX_RAW_JSON_BYTES, reject_before_allocation};

const ALLOWED_VERSIONS: [&str; 2] = ["0.0.4", "1.0.0"];
const ALLOWED_WORLDS: [&str; 2] = ["toolset-plugin", "context-plugin"];
const ALLOWED_PERMISSIONS: [&str; 8] = [
    "logging",
    "blobs",
    "http",
    "filesystem",
    "network",
    "secrets",
    "clock",
    "random",
];
const IDENTITY_PREFIX: &str = "finstack.plugin.";

/// Optional signature metadata stored on the manifest. Host-side verify is signature-verification contract.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSignature {
    /// Signature algorithm identifier.
    pub algorithm: String,
    /// Non-secret key identifier.
    pub key_id: String,
    /// Detached signature bytes encoded as text.
    pub signature: String,
}

/// Optional host-side resource ceilings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginResourceLimits {
    /// Maximum guest output bytes for one call.
    pub max_output_bytes: u64,
    /// Host call deadline in milliseconds.
    pub call_timeout_ms: u64,
    /// Optional linear-memory ceiling in bytes.
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,
    /// Optional per-store table ceiling.
    #[serde(default)]
    pub max_tables: Option<u32>,
    /// Optional per-store instance ceiling.
    #[serde(default)]
    pub max_instances: Option<u32>,
    /// Optional fuel budget for one store.
    #[serde(default)]
    pub fuel: Option<u64>,
}

/// Fail-closed experimental plugin package metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    /// Namespaced component identity.
    pub identity: ComponentId,
    /// Package version. Must be `0.0.4` or `1.0.0`.
    pub version: String,
    /// Declared exact worlds.
    pub worlds: Vec<String>,
    /// Requested host capabilities.
    pub permissions: Vec<String>,
    /// Configuration schema object.
    pub configuration_schema: Vec<u8>,
    /// Host-verified in-process digest.
    pub digest: String,
    /// Optional unsigned-or-present signature metadata.
    pub signature: Option<PluginSignature>,
    /// Optional host-side ceilings.
    pub resource_limits: Option<PluginResourceLimits>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestWire {
    identity: String,
    version: String,
    worlds: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default = "empty_object")]
    configuration_schema: Value,
    digest: String,
    #[serde(default)]
    signature: Option<PluginSignature>,
    #[serde(default)]
    resource_limits: Option<PluginResourceLimits>,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Parse and validate one experimental plugin manifest.
///
/// # Errors
///
/// Returns [`WitMapError`] for omitted/unknown fields, incompatible
/// interfaces, undeclared worlds, digest mismatch, or payload ceilings.
pub fn parse_manifest(bytes: &[u8]) -> Result<PluginManifest, WitMapError> {
    reject_before_allocation(bytes, MAX_RAW_JSON_BYTES, "manifest-json")?;
    let wire: ManifestWire = serde_json::from_slice(bytes)
        .map_err(|_| WitMapError::ManifestInvalid("manifest JSON is invalid"))?;
    validate_wire(wire)
}

/// Validate an already-decoded manifest value.
///
/// # Errors
///
/// Same as [`parse_manifest`].
pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), WitMapError> {
    let expected = manifest_digest_hex(
        manifest.identity.as_str(),
        &manifest.version,
        &manifest.worlds,
    )?;
    if manifest.digest != expected {
        return Err(WitMapError::ManifestDigestMismatch);
    }
    Ok(())
}

/// Reject duplicate identities across a candidate set.
///
/// # Errors
///
/// Returns [`WitMapError::ManifestInvalid`] when two manifests share an identity.
pub fn reject_duplicate_identities(manifests: &[PluginManifest]) -> Result<(), WitMapError> {
    let mut seen = BTreeSet::new();
    for manifest in manifests {
        if !seen.insert(manifest.identity.as_str()) {
            return Err(WitMapError::ManifestInvalid("duplicate plugin identity"));
        }
    }
    Ok(())
}

/// Canonical `{identity, version, worlds}` bytes used for digest and signatures.
///
/// # Errors
///
/// Returns [`WitMapError`] when canonicalization exceeds the JSON ceiling.
pub fn manifest_signing_payload(
    identity: &str,
    version: &str,
    worlds: &[String],
) -> Result<Vec<u8>, WitMapError> {
    let mut worlds = worlds.to_vec();
    worlds.sort();
    worlds.dedup();
    let encoded = serde_json::to_vec(&serde_json::json!({
        "identity": identity,
        "version": version,
        "worlds": worlds,
    }))
    .map_err(|_| WitMapError::ManifestInvalid("manifest canonicalization failed"))?;
    reject_before_allocation(&encoded, MAX_RAW_JSON_BYTES, "manifest-digest-json")?;
    let raw = RawJson::parse(&encoded)
        .map_err(|_| WitMapError::ManifestInvalid("manifest digest JSON is invalid"))?;
    Ok(raw.as_bytes().to_vec())
}

/// Host-computed digest over canonical `{identity, version, worlds}`.
///
/// # Errors
///
/// Returns [`WitMapError`] when canonicalization exceeds the JSON ceiling.
pub fn manifest_digest_hex(
    identity: &str,
    version: &str,
    worlds: &[String],
) -> Result<String, WitMapError> {
    let payload = manifest_signing_payload(identity, version, worlds)?;
    Ok(Digest::raw_json(&payload).to_hex())
}

fn validate_wire(wire: ManifestWire) -> Result<PluginManifest, WitMapError> {
    if wire.identity.is_empty() || !wire.identity.starts_with(IDENTITY_PREFIX) {
        return Err(WitMapError::ManifestInvalid(
            "identity must be a finstack.plugin.* component id",
        ));
    }
    let identity = ComponentId::parse(&wire.identity)
        .map_err(|_| WitMapError::ManifestInvalid("identity is not a namespaced component id"))?;
    if !ALLOWED_VERSIONS.contains(&wire.version.as_str()) {
        return Err(WitMapError::ManifestInvalid(
            "version must be 0.0.4 or 1.0.0",
        ));
    }
    if wire.worlds.is_empty() {
        return Err(WitMapError::ManifestInvalid("worlds must be non-empty"));
    }
    let mut worlds = BTreeSet::new();
    for world in &wire.worlds {
        if !ALLOWED_WORLDS.contains(&world.as_str()) {
            return Err(WitMapError::ManifestInvalid("undeclared capability world"));
        }
        if !worlds.insert(world.clone()) {
            return Err(WitMapError::ManifestInvalid("worlds contain a duplicate"));
        }
    }
    let mut permissions = BTreeSet::new();
    for permission in &wire.permissions {
        if !ALLOWED_PERMISSIONS.contains(&permission.as_str()) {
            return Err(WitMapError::ManifestInvalid("undeclared host permission"));
        }
        permissions.insert(permission.clone());
    }
    if !wire.configuration_schema.is_object() {
        return Err(WitMapError::ManifestInvalid(
            "configuration_schema must be a JSON object",
        ));
    }
    let configuration_schema = serde_json::to_vec(&wire.configuration_schema)
        .map_err(|_| WitMapError::ManifestInvalid("configuration_schema is invalid"))?;
    reject_before_allocation(
        &configuration_schema,
        MAX_RAW_JSON_BYTES,
        "configuration_schema",
    )?;
    if wire.digest.is_empty() {
        return Err(WitMapError::ManifestInvalid("digest is omitted"));
    }
    reject_before_allocation(wire.digest.as_bytes(), MAX_RAW_JSON_BYTES, "digest")?;
    if let Some(signature) = &wire.signature
        && (signature.algorithm.is_empty()
            || signature.key_id.is_empty()
            || signature.signature.is_empty())
    {
        return Err(WitMapError::ManifestInvalid(
            "signature metadata is malformed",
        ));
    }
    if let Some(limits) = &wire.resource_limits
        && (limits.max_output_bytes == 0
            || limits.call_timeout_ms == 0
            || limits.max_memory_bytes == Some(0)
            || limits.max_tables == Some(0)
            || limits.max_instances == Some(0)
            || limits.fuel == Some(0))
    {
        return Err(WitMapError::ManifestInvalid(
            "resource limit fields must be non-zero",
        ));
    }
    let expected = manifest_digest_hex(identity.as_str(), &wire.version, &wire.worlds)?;
    if wire.digest != expected {
        return Err(WitMapError::ManifestDigestMismatch);
    }
    Ok(PluginManifest {
        identity,
        version: wire.version,
        worlds: worlds.into_iter().collect(),
        permissions: permissions.into_iter().collect(),
        configuration_schema,
        digest: wire.digest,
        signature: wire.signature,
        resource_limits: wire.resource_limits,
    })
}

#[cfg(test)]
mod tests {
    use super::{manifest_digest_hex, parse_manifest, reject_duplicate_identities};

    fn valid_bytes(identity: &str, worlds: &[&str]) -> Vec<u8> {
        let world_owned: Vec<String> = worlds.iter().map(|world| (*world).to_owned()).collect();
        let digest = manifest_digest_hex(identity, "0.0.4", &world_owned).expect("digest");
        serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging", "blobs"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json")
    }

    #[test]
    fn catalog_permissions_parse_without_sandboxing() {
        let identity = "finstack.plugin.reference.context";
        let worlds = vec!["context-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging", "filesystem", "clock"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json");
        let parsed = parse_manifest(&bytes).expect("catalog names parse");
        assert!(parsed.permissions.contains(&"filesystem".to_owned()));
        assert!(parsed.permissions.contains(&"clock".to_owned()));
    }

    #[test]
    fn valid_context_manifest_parses() {
        let parsed = parse_manifest(&valid_bytes(
            "finstack.plugin.reference.context",
            &["context-plugin"],
        ))
        .expect("valid");
        assert_eq!(
            parsed.identity.as_str(),
            "finstack.plugin.reference.context"
        );
        assert_eq!(parsed.worlds, ["context-plugin"]);
    }

    #[test]
    fn duplicate_identities_fail_closed() {
        let first = parse_manifest(&valid_bytes(
            "finstack.plugin.reference.context",
            &["context-plugin"],
        ))
        .expect("first");
        let second = first.clone();
        assert_eq!(
            reject_duplicate_identities(&[first, second])
                .expect_err("duplicate")
                .code(),
            "plugin_registration_invalid"
        );
    }

    #[test]
    fn incompatible_and_undeclared_worlds_fail_closed() {
        let unknown_major = br#"{"identity":"finstack.plugin.reference.context","version":"2.0.0","worlds":["context-plugin"],"digest":"00"}"#;
        assert_eq!(
            parse_manifest(unknown_major)
                .expect_err("unknown major")
                .code(),
            "plugin_registration_invalid"
        );
        let middleware = valid_bytes("finstack.plugin.reference.context", &["middleware"]);
        assert_eq!(
            parse_manifest(&middleware).expect_err("world").code(),
            "plugin_registration_invalid"
        );
        let mut tampered = parse_manifest(&valid_bytes(
            "finstack.plugin.reference.context",
            &["context-plugin"],
        ))
        .expect("ok");
        tampered.digest = "00".repeat(32);
        assert_eq!(
            super::validate_manifest(&tampered)
                .expect_err("digest")
                .code(),
            "plugin_manifest_digest_mismatch"
        );
    }

    #[test]
    fn compatibility_manifest_fixtures_fail_closed() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/compatibility/wit/v0.0.4/manifest");
        let valid = std::fs::read(root.join("valid--context-reference.json")).expect("valid");
        let parsed = parse_manifest(&valid).expect("valid fixture");
        assert_eq!(
            parsed.identity.as_str(),
            "finstack.plugin.reference.context"
        );
        let historical_v1 =
            std::fs::read(root.join("invalid--incompatible-interface.json")).expect("historical");
        assert_eq!(
            parse_manifest(&historical_v1)
                .expect_err("historical 1.0.0 fixture has a placeholder digest")
                .code(),
            "plugin_manifest_digest_mismatch"
        );
        let v1_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/compatibility/wit/v1.0.0/manifest");
        let valid_v1 = std::fs::read(v1_root.join("valid--context-reference.json")).expect("v1");
        let parsed_v1 = parse_manifest(&valid_v1).expect("1.0.0 is a supported major");
        assert_eq!(parsed_v1.version, "1.0.0");
        let unknown_major =
            std::fs::read(v1_root.join("invalid--unknown-major.json")).expect("unknown major");
        assert_eq!(
            parse_manifest(&unknown_major).expect_err("2.0.0").code(),
            "plugin_registration_invalid"
        );
        let unknown_field =
            std::fs::read(v1_root.join("invalid--unknown-field.json")).expect("unknown field");
        assert_eq!(
            parse_manifest(&unknown_field)
                .expect_err("unknown field")
                .code(),
            "plugin_registration_invalid"
        );
        let duplicates: Vec<serde_json::Value> = serde_json::from_slice(
            &std::fs::read(root.join("invalid--duplicate-identity.json")).expect("dup"),
        )
        .expect("array");
        let parsed = duplicates
            .into_iter()
            .map(|value| {
                let identity = value["identity"].as_str().expect("identity");
                parse_manifest(&valid_bytes(identity, &["context-plugin"])).expect("row")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reject_duplicate_identities(&parsed)
                .expect_err("duplicate fixture")
                .code(),
            "plugin_registration_invalid"
        );
    }
}
