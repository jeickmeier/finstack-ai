//! Host-side ed25519 verification over the plugin-manifest contract manifest digest payload.

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use finstack_ai_wit::{PluginManifest, PluginSignature, manifest_signing_payload};

use crate::error::PluginHostError;

/// Host signature policy. This is not kernel behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignaturePolicy {
    /// Unsigned packages are allowed. Development/fixtures only; this is an
    /// explicit opt-in, never a production posture. Malformed metadata still
    /// fails parse.
    Permissive,
    /// A present signature must verify against a configured trust root.
    /// This is the default.
    #[default]
    Strict,
}

/// Verify `manifest.signature` against `trust_roots` under `policy`.
///
/// # Errors
///
/// Returns [`PluginHostError::SignatureUntrusted`] for unsigned Strict
/// packages, unknown keys, or failed verification.
pub fn verify_manifest(
    manifest: &PluginManifest,
    policy: SignaturePolicy,
    trust_roots: &BTreeMap<String, [u8; 32]>,
) -> Result<(), PluginHostError> {
    match (policy, manifest.signature.as_ref()) {
        (SignaturePolicy::Permissive, None) => Ok(()),
        (SignaturePolicy::Strict, None) => Err(PluginHostError::SignatureUntrusted(
            "unsigned package".into(),
        )),
        (_, Some(signature)) => verify_present(manifest, signature, trust_roots),
    }
}

fn verify_present(
    manifest: &PluginManifest,
    signature: &PluginSignature,
    trust_roots: &BTreeMap<String, [u8; 32]>,
) -> Result<(), PluginHostError> {
    if signature.algorithm != "ed25519" {
        return Err(PluginHostError::SignatureUntrusted(
            "unsupported signature algorithm".into(),
        ));
    }
    let Some(root) = trust_roots.get(&signature.key_id) else {
        return Err(PluginHostError::SignatureUntrusted(
            "unknown signature key_id".into(),
        ));
    };
    let payload = manifest_signing_payload(
        manifest.identity.as_str(),
        &manifest.version,
        &manifest.worlds,
        &manifest.permissions,
    )
    .map_err(|error| PluginHostError::from_map(&error))?;
    let verifying = VerifyingKey::from_bytes(root).map_err(|_| {
        PluginHostError::SignatureUntrusted("trust root is not a valid ed25519 key".into())
    })?;
    let sig_bytes = decode_hex(&signature.signature)?;
    let sig_array: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| PluginHostError::SignatureUntrusted("signature is not 64 bytes".into()))?;
    let sig = Signature::from_bytes(&sig_array);
    verifying
        .verify(&payload, &sig)
        .map_err(|_| PluginHostError::SignatureUntrusted("signature does not verify".into()))
}

fn decode_hex(input: &str) -> Result<Vec<u8>, PluginHostError> {
    if !input.len().is_multiple_of(2) {
        return Err(PluginHostError::SignatureUntrusted(
            "signature hex is malformed".into(),
        ));
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let hi = finstack_ai_kernel::hex_nibble(bytes[index]).ok_or_else(|| {
            PluginHostError::SignatureUntrusted("signature hex is malformed".into())
        })?;
        let lo = finstack_ai_kernel::hex_nibble(bytes[index + 1]).ok_or_else(|| {
            PluginHostError::SignatureUntrusted("signature hex is malformed".into())
        })?;
        out.push((hi << 4) | lo);
        index += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{SignaturePolicy, verify_manifest};
    use ed25519_dalek::{Signer, SigningKey};
    use finstack_ai_wit::{
        PluginManifest, PluginSignature, manifest_digest_hex, manifest_signing_payload,
        parse_manifest,
    };
    use std::collections::BTreeMap;
    use std::fmt::Write;

    fn signed_manifest(secret: &[u8; 32], key_id: &str) -> (PluginManifest, [u8; 32]) {
        let signing = SigningKey::from_bytes(secret);
        let identity = "finstack.plugin.echo.toolset";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds, &["logging".to_owned()])
            .expect("digest");
        let payload = manifest_signing_payload(identity, "0.0.4", &worlds, &["logging".to_owned()])
            .expect("payload");
        let signature = signing.sign(&payload);
        let bytes = serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest,
            "signature": {
                "algorithm": "ed25519",
                "key_id": key_id,
                "signature": encode_hex(&signature.to_bytes()),
            }
        }))
        .expect("json");
        (
            parse_manifest(&bytes).expect("manifest"),
            signing.verifying_key().to_bytes(),
        )
    }

    fn unsigned_manifest() -> PluginManifest {
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
        parse_manifest(&bytes).expect("manifest")
    }

    #[test]
    fn default_policy_is_strict() {
        assert_eq!(SignaturePolicy::default(), SignaturePolicy::Strict);
    }

    #[test]
    fn permissive_allows_unsigned_and_strict_rejects() {
        let manifest = unsigned_manifest();
        verify_manifest(&manifest, SignaturePolicy::Permissive, &BTreeMap::new())
            .expect("permissive");
        assert_eq!(
            verify_manifest(&manifest, SignaturePolicy::Strict, &BTreeMap::new())
                .expect_err("strict")
                .code(),
            "plugin_signature_untrusted"
        );
    }

    #[test]
    fn strict_accepts_a_configured_root() {
        let (manifest, public) = signed_manifest(&[7; 32], "test-root");
        let mut roots = BTreeMap::new();
        roots.insert("test-root".into(), public);
        verify_manifest(&manifest, SignaturePolicy::Strict, &roots).expect("verify");
        verify_manifest(&manifest, SignaturePolicy::Permissive, &roots).expect("permissive");
    }

    #[test]
    fn wrong_key_is_untrusted_in_both_modes() {
        let (manifest, _) = signed_manifest(&[7; 32], "test-root");
        let other = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        let mut roots = BTreeMap::new();
        roots.insert("test-root".into(), other);
        assert_eq!(
            verify_manifest(&manifest, SignaturePolicy::Permissive, &roots)
                .expect_err("bad")
                .code(),
            "plugin_signature_untrusted"
        );
        let _unused = PluginSignature {
            algorithm: "ed25519".into(),
            key_id: "x".into(),
            signature: "00".into(),
        };
    }

    #[test]
    fn materialize_a03_fixtures() {
        use std::path::PathBuf;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/manifests");
        std::fs::create_dir_all(&root).expect("mkdir");
        let identity = "finstack.plugin.echo.toolset";
        let worlds = vec!["toolset-plugin".to_owned()];
        let digest = manifest_digest_hex(identity, "0.0.4", &worlds, &["logging".to_owned()])
            .expect("digest");
        let unsigned = serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest
        });
        std::fs::write(
            root.join("unsigned.json"),
            serde_json::to_vec_pretty(&unsigned).expect("json"),
        )
        .expect("write unsigned");

        let signing = SigningKey::from_bytes(&[7; 32]);
        let payload = manifest_signing_payload(identity, "0.0.4", &worlds, &["logging".to_owned()])
            .expect("payload");
        let signature = signing.sign(&payload);
        let valid = serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest,
            "signature": {
                "algorithm": "ed25519",
                "key_id": "test-root",
                "signature": encode_hex(&signature.to_bytes()),
            }
        });
        std::fs::write(
            root.join("signed-valid.json"),
            serde_json::to_vec_pretty(&valid).expect("json"),
        )
        .expect("write valid");
        std::fs::write(
            root.join("test-root.pub.hex"),
            format!("{}\n", encode_hex(&signing.verifying_key().to_bytes())),
        )
        .expect("write root");

        let other = SigningKey::from_bytes(&[9; 32]);
        let wrong = other.sign(&payload);
        let untrusted = serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest,
            "signature": {
                "algorithm": "ed25519",
                "key_id": "test-root",
                "signature": encode_hex(&wrong.to_bytes()),
            }
        });
        std::fs::write(
            root.join("signed-untrusted.json"),
            serde_json::to_vec_pretty(&untrusted).expect("json"),
        )
        .expect("write untrusted");
    }

    #[test]
    fn checked_in_manifest_fixtures() {
        use std::path::PathBuf;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/manifests");
        let unsigned =
            parse_manifest(&std::fs::read(root.join("unsigned.json")).expect("unsigned fixture"))
                .expect("unsigned parses");
        verify_manifest(&unsigned, SignaturePolicy::Permissive, &BTreeMap::new())
            .expect("permissive unsigned");
        assert_eq!(
            verify_manifest(&unsigned, SignaturePolicy::Strict, &BTreeMap::new())
                .expect_err("strict unsigned")
                .code(),
            "plugin_signature_untrusted"
        );

        let valid = parse_manifest(
            &std::fs::read(root.join("signed-valid.json")).expect("signed-valid fixture"),
        )
        .expect("signed-valid parses");
        let public =
            hex_to_32(&std::fs::read_to_string(root.join("test-root.pub.hex")).expect("root"));
        let mut roots = BTreeMap::new();
        roots.insert("test-root".into(), public);
        verify_manifest(&valid, SignaturePolicy::Strict, &roots).expect("strict valid");
        verify_manifest(&valid, SignaturePolicy::Permissive, &roots).expect("permissive valid");

        let untrusted = parse_manifest(
            &std::fs::read(root.join("signed-untrusted.json")).expect("untrusted fixture"),
        )
        .expect("untrusted parses");
        assert_eq!(
            verify_manifest(&untrusted, SignaturePolicy::Permissive, &roots)
                .expect_err("wrong key")
                .code(),
            "plugin_signature_untrusted"
        );
        assert_eq!(
            verify_manifest(&untrusted, SignaturePolicy::Strict, &roots)
                .expect_err("strict wrong key")
                .code(),
            "plugin_signature_untrusted"
        );
    }

    fn hex_to_32(input: &str) -> [u8; 32] {
        let trimmed = input.trim();
        let bytes = super::decode_hex(trimmed).expect("hex");
        bytes.try_into().expect("32 bytes")
    }

    fn encode_hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
    }
}
