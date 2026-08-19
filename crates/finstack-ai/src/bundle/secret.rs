//! TM-04 config-key scan. Keys ending in `_ref` are allowed.

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, RawJson};

use super::BundleResolutionError;

pub(super) fn ensure_secret_free_config(
    config: &BTreeMap<ComponentId, RawJson>,
) -> Result<(), BundleResolutionError> {
    for value in config.values() {
        let parsed: serde_json::Value =
            serde_json::from_slice(value.as_bytes()).map_err(|error| {
                BundleResolutionError::Invalid {
                    message: Arc::from(error.to_string()),
                }
            })?;
        if contains_secret(&parsed) {
            return Err(BundleResolutionError::Invalid {
                message: Arc::from("secret_material_in_configuration"),
            });
        }
    }
    Ok(())
}

/// Secret-key detector for bundle configuration objects.
///
/// Scans object keys only. String values are ignored to avoid false positives.
/// Keys ending in `_ref` are allowed. Tokens are split on non-alphanumeric
/// characters so `auth` matches `auth_token` but not `oauth` or `author`.
/// Innocuous keys that hold credential values remain a host problem
/// (TM-04 residual); this is not a config-schema allowlist.
fn contains_secret(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .any(|(key, value)| key_looks_secret(key) || contains_secret(value)),
        serde_json::Value::Array(values) => values.iter().any(contains_secret),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}

const SECRET_TOKENS: &[&str] = &[
    "password",
    "token",
    "credential",
    "apikey",
    "bearer",
    "secret",
    "auth",
    "authorization",
    "secretkey",
];

const SECRET_COMPOUNDS: &[&[&str]] = &[&["api", "key"], &["access", "key"], &["private", "key"]];

fn key_looks_secret(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    if normalized.ends_with("_ref") {
        return false;
    }
    let tokens: Vec<&str> = normalized
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    tokens.iter().any(|token| SECRET_TOKENS.contains(token))
        || tokens
            .windows(2)
            .any(|pair| SECRET_COMPOUNDS.contains(&pair))
}
