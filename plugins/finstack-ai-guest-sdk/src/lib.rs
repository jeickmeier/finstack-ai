//! Author-facing helpers for experimental `@0.0.4` toolset and context guests.
//!
//! This crate does not link Wasmtime and does not depend on the isolated host.
//! Guests invoke [`toolset_plugin!`] or [`context_plugin!`] in their own crate
//! so `export!` stays in the cdylib. Host-import calls stay in the guest after
//! those macros expand.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
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

/// Pinned `wit-bindgen` re-export so guests do not list the crate themselves.
pub use wit_bindgen;

/// Pinned `wit-bindgen` crate version used by the guest macros.
pub const WIT_BINDGEN_VERSION: &str = "0.57.1";
/// Experimental WIT package version guests must pin unless they retarget.
pub const WIT_PACKAGE_VERSION: &str = "0.0.4";
/// Frozen WIT package version for guests that retarget to `@1.0.0`.
pub const WIT_PACKAGE_VERSION_V1: &str = "1.0.0";

/// Individual text or byte-string ceiling (contract section 6.5).
pub const MAX_STRING_BYTES: usize = 4 * 1024 * 1024;
/// `RawJson` / args / result / catalog ceiling (contract section 6.5).
pub const MAX_RAW_JSON_BYTES: usize = 1_048_576;
/// Metadata object ceiling (contract section 6.5).
pub const MAX_METADATA_BYTES: usize = 64 * 1024;

/// Stable guest-side failure. Map into the generated WIT `plugin-error`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct GuestError {
    /// Stable code string.
    pub code: String,
    /// Non-secret diagnostic message.
    pub message: String,
    /// Whether the host may retry the same call.
    pub retryable: bool,
}

/// Construct a non-retryable [`GuestError`].
#[must_use]
pub fn plugin_error(code: &str, message: &str) -> GuestError {
    GuestError {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

/// Reject an empty sanitized tenant scope or authorization decision id.
///
/// # Errors
///
/// Returns `plugin_call_context_invalid` when either field is empty.
pub fn require_sanitized_context(
    tenant_scope: &str,
    authorization_decision_id: &str,
) -> Result<(), GuestError> {
    if tenant_scope.is_empty() || authorization_decision_id.is_empty() {
        return Err(plugin_error(
            "plugin_call_context_invalid",
            "sanitized call-context is incomplete",
        ));
    }
    Ok(())
}

/// Catalog fields used to compute the host-verified digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpecParts {
    /// Tool identity.
    pub id: String,
    /// Model-visible name.
    pub model_name: String,
    /// Human title.
    pub title: String,
    /// Human description.
    pub description: String,
    /// Input JSON Schema bytes.
    pub input_schema_json: Vec<u8>,
    /// Optional output JSON Schema bytes.
    pub output_schema_json: Option<Vec<u8>>,
    /// `parallel`, `sequential`, or `barrier`.
    pub execution_mode: String,
    /// Side-effect class string.
    pub side_effect: String,
    /// Retry-safety string.
    pub retry_safety: String,
    /// Approval-policy JSON bytes.
    pub approval_policy_json: Vec<u8>,
    /// Per-tool result ceiling.
    pub max_result_bytes: u64,
    /// Bounded metadata JSON bytes.
    pub metadata_json: Vec<u8>,
}

/// Compute the host-verified catalog digest for `tools`.
///
/// The algorithm matches `finstack_ai_wit::catalog_digest_hex`: JCS over
/// `{ "tools": [ canonical tool objects ] }`, then the `raw-json` digest.
///
/// # Errors
///
/// Returns `plugin_registration_invalid` or `plugin_payload_too_large` when
/// a field is not JSON or the catalog exceeds [`MAX_RAW_JSON_BYTES`].
pub fn catalog_digest(tools: &[ToolSpecParts]) -> Result<String, GuestError> {
    let mut items = Vec::with_capacity(tools.len());
    for tool in tools {
        items.push(canonical_tool_value(tool)?);
    }
    let encoded = serde_json::to_vec(&serde_json::json!({ "tools": items })).map_err(|_| {
        plugin_error(
            "plugin_registration_invalid",
            "catalog canonicalization failed",
        )
    })?;
    reject_len(&encoded, MAX_RAW_JSON_BYTES, "catalog-json")?;
    let value: serde_json::Value = serde_json::from_slice(&encoded)
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog JSON is invalid"))?;
    let canonical = serde_json_canonicalizer::to_vec(&value)
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog JCS failed"))?;
    Ok(raw_json_digest_hex(&canonical))
}

/// Deserialize tool or context arguments after a size check.
///
/// # Errors
///
/// Returns `plugin_payload_too_large` or `plugin_arguments_invalid`.
pub fn parse_args<T: serde::de::DeserializeOwned>(args_json: &[u8]) -> Result<T, GuestError> {
    reject_len(args_json, MAX_RAW_JSON_BYTES, "args-json")?;
    serde_json::from_slice(args_json)
        .map_err(|_| plugin_error("plugin_arguments_invalid", "arguments are invalid"))
}

/// Serialize a JSON result after a size check.
///
/// # Errors
///
/// Returns `plugin_result_invalid` or `plugin_payload_too_large`.
pub fn encode_json_result<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, GuestError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| plugin_error("plugin_result_invalid", "result encoding failed"))?;
    reject_len(&bytes, MAX_RAW_JSON_BYTES, "content-json")?;
    Ok(bytes)
}

/// Serialize a JSON Schema or metadata object after a size check.
///
/// # Errors
///
/// Returns `plugin_registration_invalid` or `plugin_payload_too_large`.
pub fn schema_bytes(value: &serde_json::Value) -> Result<Vec<u8>, GuestError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| plugin_error("plugin_registration_invalid", "schema encoding failed"))?;
    reject_len(&bytes, MAX_RAW_JSON_BYTES, "schema-json")?;
    Ok(bytes)
}

/// WIT logging levels. Guests pass the generated enum after [`toolset_plugin!`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Trace.
    Trace,
    /// Debug.
    Debug,
    /// Info.
    Info,
    /// Warn.
    Warn,
    /// Error.
    Error,
}

/// Reject an oversized log message before the guest calls the host import.
///
/// # Errors
///
/// Returns `plugin_payload_too_large` when `message` exceeds
/// [`MAX_STRING_BYTES`].
pub fn reject_log_message(message: &str) -> Result<(), GuestError> {
    reject_len(message.as_bytes(), MAX_STRING_BYTES, "log.message")
}

/// Fixture tenant scope used by guest-sdk tests and templates.
#[must_use]
pub const fn fixture_tenant_scope() -> &'static str {
    "tenant-a"
}

/// Fixture authorization decision id used by guest-sdk tests and templates.
#[must_use]
pub const fn fixture_decision_id() -> &'static str {
    "decision-v1"
}

/// Expand `wit_bindgen::generate!` for `toolset-plugin` from the crate `wit/`
/// directory.
#[macro_export]
macro_rules! toolset_plugin {
    () => {
        $crate::wit_bindgen::generate!({
            world: "toolset-plugin",
            path: "wit",
            generate_all,
            runtime_path: "finstack_ai_guest_sdk::wit_bindgen::rt",
        });
    };
}

/// Expand `wit_bindgen::generate!` for `context-plugin` from the crate `wit/`
/// directory.
#[macro_export]
macro_rules! context_plugin {
    () => {
        $crate::wit_bindgen::generate!({
            world: "context-plugin",
            path: "wit",
            generate_all,
            runtime_path: "finstack_ai_guest_sdk::wit_bindgen::rt",
        });
    };
}

fn canonical_tool_value(spec: &ToolSpecParts) -> Result<serde_json::Value, GuestError> {
    reject_len(spec.id.as_bytes(), MAX_STRING_BYTES, "tool.id")?;
    reject_len(
        spec.model_name.as_bytes(),
        MAX_STRING_BYTES,
        "tool.model-name",
    )?;
    reject_len(spec.title.as_bytes(), MAX_STRING_BYTES, "tool.title")?;
    reject_len(
        spec.description.as_bytes(),
        MAX_STRING_BYTES,
        "tool.description",
    )?;
    reject_len(
        spec.input_schema_json.as_slice(),
        MAX_RAW_JSON_BYTES,
        "input-schema-json",
    )?;
    if let Some(output) = spec.output_schema_json.as_deref() {
        reject_len(output, MAX_RAW_JSON_BYTES, "output-schema-json")?;
    }
    reject_len(
        spec.approval_policy_json.as_slice(),
        MAX_RAW_JSON_BYTES,
        "approval-policy-json",
    )?;
    reject_len(
        spec.metadata_json.as_slice(),
        MAX_METADATA_BYTES,
        "metadata-json",
    )?;
    Ok(serde_json::json!({
        "id": spec.id,
        "model-name": spec.model_name,
        "title": spec.title,
        "description": spec.description,
        "input-schema-json": parse_json(&spec.input_schema_json)?,
        "output-schema-json": spec
            .output_schema_json
            .as_deref()
            .map(parse_json)
            .transpose()?,
        "execution-mode": spec.execution_mode,
        "side-effect": spec.side_effect,
        "retry-safety": spec.retry_safety,
        "approval-policy-json": parse_json(&spec.approval_policy_json)?,
        "max-result-bytes": spec.max_result_bytes,
        "metadata-json": parse_json(&spec.metadata_json)?,
    }))
}

fn parse_json(bytes: &[u8]) -> Result<serde_json::Value, GuestError> {
    serde_json::from_slice(bytes)
        .map_err(|_| plugin_error("plugin_registration_invalid", "tool JSON is invalid"))
}

fn reject_len(bytes: &[u8], max: usize, field: &'static str) -> Result<(), GuestError> {
    if bytes.len() > max {
        return Err(plugin_error("plugin_payload_too_large", field));
    }
    Ok(())
}

fn raw_json_digest_hex(canonical: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(b"finstack-ai");
    hasher.update([0]);
    hasher.update(b"raw-json");
    hasher.update([0]);
    hasher.update(1_u32.to_be_bytes());
    hasher.update([0]);
    hasher.update(canonical);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(HEX[(byte >> 4) as usize] as char);
        hex.push(HEX[(byte & 0x0f) as usize] as char);
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::{
        GuestError, LogLevel, MAX_RAW_JSON_BYTES, MAX_STRING_BYTES, ToolSpecParts,
        WIT_BINDGEN_VERSION, WIT_PACKAGE_VERSION, WIT_PACKAGE_VERSION_V1, catalog_digest,
        encode_json_result, fixture_decision_id, fixture_tenant_scope, parse_args, plugin_error,
        reject_log_message, require_sanitized_context, schema_bytes,
    };
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct EchoArgs {
        text: String,
    }

    fn sample_tool() -> ToolSpecParts {
        ToolSpecParts {
            id: "finstack.plugin.template.echo".to_owned(),
            model_name: "echo".to_owned(),
            title: "Echo".to_owned(),
            description: "Echo a bounded text field".to_owned(),
            input_schema_json:
                br#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#
                    .to_vec(),
            output_schema_json: Some(br#"{"type":"object"}"#.to_vec()),
            execution_mode: "sequential".to_owned(),
            side_effect: "read_only".to_owned(),
            retry_safety: "safe_to_retry".to_owned(),
            approval_policy_json: br#"{"requirement":"not_required"}"#.to_vec(),
            max_result_bytes: 1024,
            metadata_json: b"{}".to_vec(),
        }
    }

    #[test]
    fn versions_are_pinned() {
        assert_eq!(WIT_BINDGEN_VERSION, "0.57.1");
        assert_eq!(WIT_PACKAGE_VERSION, "0.0.4");
        assert_eq!(WIT_PACKAGE_VERSION_V1, "1.0.0");
    }

    #[test]
    fn catalog_digest_is_stable() {
        let digest = catalog_digest(&[sample_tool()]).expect("digest");
        assert_eq!(
            digest,
            "d24b2dbce83bac7cbaa43a2e97d527238fe169cc4ca91dd5b32d8032c32e5bf8"
        );
    }

    #[test]
    fn empty_context_is_rejected() {
        let error = require_sanitized_context("", fixture_decision_id()).expect_err("empty");
        assert_eq!(error.code, "plugin_call_context_invalid");
        require_sanitized_context(fixture_tenant_scope(), fixture_decision_id()).expect("ok");
    }

    #[test]
    fn parse_and_encode_round_trip() {
        let args: EchoArgs = parse_args(br#"{"text":"ok"}"#).expect("parse");
        assert_eq!(args.text, "ok");
        let encoded = encode_json_result(&serde_json::json!({ "text": "ok" })).expect("encode");
        assert_eq!(encoded, br#"{"text":"ok"}"#);
        let schema = schema_bytes(&serde_json::json!({"type":"object"})).expect("schema");
        assert_eq!(schema, br#"{"type":"object"}"#);
    }

    #[test]
    fn guest_sdk_stays_host_free() {
        use std::process::Command;

        let output = Command::new("cargo")
            .args([
                "tree",
                "-p",
                "finstack-ai-guest-sdk",
                "--locked",
                "--prefix",
                "none",
                "--format",
                "{p}",
                "--edges",
                "normal",
            ])
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .output()
            .expect("cargo tree");
        assert!(
            output.status.success(),
            "cargo tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let tree = String::from_utf8(output.stdout).expect("utf8");
        for name in [
            "wasmtime",
            "wasmtime-wasi",
            "finstack-ai-plugin-host",
            "finstack-ai-wit",
            "finstack-ai-kernel",
            "finstack-ai-runtime",
            "tokio",
            "libloading",
        ] {
            assert!(
                !tree
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some(name)),
                "guest-sdk must not depend on {name}:\n{tree}"
            );
        }
        assert!(
            !tree
                .lines()
                .any(|line| line.split_whitespace().next() == Some("finstack-ai")),
            "guest-sdk must not depend on finstack-ai:\n{tree}"
        );
    }

    #[test]
    fn oversized_payloads_are_rejected() {
        let args = vec![b'x'; MAX_RAW_JSON_BYTES + 1];
        assert_eq!(
            parse_args::<EchoArgs>(&args).expect_err("args").code,
            "plugin_payload_too_large"
        );
        let message = "x".repeat(MAX_STRING_BYTES + 1);
        assert_eq!(
            reject_log_message(&message).expect_err("log").code,
            "plugin_payload_too_large"
        );
        let _ = LogLevel::Info;
        let _ = plugin_error("plugin_unknown_tool", "unknown");
        let _ = GuestError {
            code: "x".into(),
            message: "y".into(),
            retryable: false,
        };
    }
}
