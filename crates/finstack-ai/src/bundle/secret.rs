//! TM-04 heuristic configuration scan. Credential-free references are not a parser-backed exemption.

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
/// Scans keys and rejects credential-shaped string values. This is a heuristic,
/// not a complete secret detector or config-schema allowlist.
fn contains_secret(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            key_looks_secret(key) || value_looks_secret(value) || contains_secret(value)
        }),
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
    let normalized = key.to_string();
    let mut separated = String::with_capacity(normalized.len() + 4);
    for (index, character) in normalized.chars().enumerate() {
        if index > 0 && character.is_ascii_uppercase() {
            separated.push('_');
        }
        separated.push(character.to_ascii_lowercase());
    }
    let tokens: Vec<&str> = separated
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty() && *token != "ref")
        .collect();
    tokens.iter().any(|token| SECRET_TOKENS.contains(token))
        || tokens
            .windows(2)
            .any(|pair| SECRET_COMPOUNDS.contains(&pair))
}

fn value_looks_secret(value: &serde_json::Value) -> bool {
    let serde_json::Value::String(value) = value else {
        return false;
    };
    let lower = value.to_ascii_lowercase();
    lower.starts_with("sk-") || lower.starts_with("aiza") || lower.starts_with("bearer ")
}
