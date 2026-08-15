//! Test-only `toolset-plugin` guest. Not a published SDK or template.

wit_bindgen::generate!({
    world: "toolset-plugin",
    path: "../../../wit",
    generate_all,
});

use exports::finstack::ai_toolset::toolset::{Guest, ToolCatalog, ToolResult, ToolSpec};
use finstack::ai_types::types::{CallContext, PluginError};

const ADD_ID: &str = "finstack.plugin.add";
const ECHO_ID: &str = "finstack.plugin.echo";

struct EchoToolset;

impl Guest for EchoToolset {
    fn list_tools() -> Result<ToolCatalog, PluginError> {
        let tools = vec![add_spec(), echo_spec()];
        let digest = catalog_digest(&tools)?;
        Ok(ToolCatalog { digest, tools })
    }

    fn call(context: CallContext, tool_id: String, args_json: Vec<u8>) -> Result<ToolResult, PluginError> {
        if context.tenant_scope.is_empty() || context.authorization_decision_id.is_empty() {
            return Err(plugin_error(
                "plugin_call_context_invalid",
                "sanitized call-context is incomplete",
            ));
        }
        match tool_id.as_str() {
            ADD_ID => add(&args_json),
            ECHO_ID => echo(&args_json),
            _ => Err(plugin_error("plugin_unknown_tool", "unknown tool id")),
        }
    }
}

export!(EchoToolset);

fn add_spec() -> ToolSpec {
    ToolSpec {
        id: ADD_ID.to_owned(),
        model_name: "add".to_owned(),
        title: "Add".to_owned(),
        description: "Add two integers".to_owned(),
        input_schema_json: br#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"]}"#.to_vec(),
        output_schema_json: Some(br#"{"type":"object"}"#.to_vec()),
        execution_mode: "sequential".to_owned(),
        side_effect: "read_only".to_owned(),
        retry_safety: "safe_to_retry".to_owned(),
        approval_policy_json: br#"{"requirement":"not_required"}"#.to_vec(),
        max_result_bytes: 1024,
        metadata_json: b"{}".to_vec(),
    }
}

fn echo_spec() -> ToolSpec {
    ToolSpec {
        id: ECHO_ID.to_owned(),
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

fn add(args_json: &[u8]) -> Result<ToolResult, PluginError> {
    let args: serde_json::Value = serde_json::from_slice(args_json)
        .map_err(|_| plugin_error("plugin_arguments_invalid", "add arguments are invalid"))?;
    let a = args
        .get("a")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| plugin_error("plugin_arguments_invalid", "add arguments are invalid"))?;
    let b = args
        .get("b")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| plugin_error("plugin_arguments_invalid", "add arguments are invalid"))?;
    let sum = a
        .checked_add(b)
        .ok_or_else(|| plugin_error("plugin_arguments_invalid", "add overflowed"))?;
    encode_result(&serde_json::json!({ "sum": sum }))
}

fn echo(args_json: &[u8]) -> Result<ToolResult, PluginError> {
    let args: serde_json::Value = serde_json::from_slice(args_json)
        .map_err(|_| plugin_error("plugin_arguments_invalid", "echo arguments are invalid"))?;
    let text = args
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| plugin_error("plugin_arguments_invalid", "echo arguments are invalid"))?;
    encode_result(&serde_json::json!({ "text": text }))
}

fn encode_result(value: &serde_json::Value) -> Result<ToolResult, PluginError> {
    let content_json = serde_json::to_vec(value)
        .map_err(|_| plugin_error("plugin_result_invalid", "result encoding failed"))?;
    Ok(ToolResult {
        content_json,
        is_error: false,
    })
}

fn catalog_digest(tools: &[ToolSpec]) -> Result<String, PluginError> {
    let mut items = Vec::new();
    for tool in tools {
        items.push(serde_json::json!({
            "id": tool.id,
            "model-name": tool.model_name,
            "title": tool.title,
            "description": tool.description,
            "input-schema-json": parse_json(&tool.input_schema_json)?,
            "output-schema-json": tool
                .output_schema_json
                .as_deref()
                .map(parse_json)
                .transpose()?,
            "execution-mode": tool.execution_mode,
            "side-effect": tool.side_effect,
            "retry-safety": tool.retry_safety,
            "approval-policy-json": parse_json(&tool.approval_policy_json)?,
            "max-result-bytes": tool.max_result_bytes,
            "metadata-json": parse_json(&tool.metadata_json)?,
        }));
    }
    let encoded = serde_json::to_vec(&serde_json::json!({ "tools": items }))
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog canonicalization failed"))?;
    let value: serde_json::Value = serde_json::from_slice(&encoded)
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog JSON is invalid"))?;
    let canonical = serde_json_canonicalizer::to_vec(&value)
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog JCS failed"))?;
    Ok(raw_json_digest_hex(&canonical))
}

fn parse_json(bytes: &[u8]) -> Result<serde_json::Value, PluginError> {
    serde_json::from_slice(bytes)
        .map_err(|_| plugin_error("plugin_registration_invalid", "tool JSON is invalid"))
}

fn raw_json_digest_hex(canonical: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"finstack-ai");
    hasher.update([0]);
    hasher.update(b"raw-json");
    hasher.update([0]);
    hasher.update(1_u32.to_be_bytes());
    hasher.update([0]);
    hasher.update(canonical);
    hasher
        .finalize()
        .iter()
        .fold(String::new(), |mut hex, byte| {
            hex.push_str(&format!("{byte:02x}"));
            hex
        })
}

fn plugin_error(code: &str, message: &str) -> PluginError {
    PluginError {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}
