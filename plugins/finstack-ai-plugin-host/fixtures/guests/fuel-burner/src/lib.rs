//! Test-only fuel-burner guest. `call` loops until the host fuel ceiling trips.

wit_bindgen::generate!({
    world: "toolset-plugin",
    path: "../../../wit",
    generate_all,
});

use exports::finstack::ai_toolset::toolset::{Guest, ToolCatalog, ToolResult, ToolSpec};
use finstack::ai_types::types::{CallContext, PluginError};

struct FuelBurner;

impl Guest for FuelBurner {
    fn list_tools() -> Result<ToolCatalog, PluginError> {
        let tools = vec![ToolSpec {
            id: "finstack.plugin.burn".to_owned(),
            model_name: "burn".to_owned(),
            title: "Burn".to_owned(),
            description: "Burn host fuel".to_owned(),
            input_schema_json: br#"{"type":"object"}"#.to_vec(),
            output_schema_json: Some(br#"{"type":"object"}"#.to_vec()),
            execution_mode: "sequential".to_owned(),
            side_effect: "read_only".to_owned(),
            retry_safety: "safe_to_retry".to_owned(),
            approval_policy_json: br#"{"requirement":"not_required"}"#.to_vec(),
            max_result_bytes: 1024,
            metadata_json: b"{}".to_vec(),
        }];
        let digest = catalog_digest(&tools)?;
        Ok(ToolCatalog { digest, tools })
    }

    fn call(
        _context: CallContext,
        _tool_id: String,
        _args_json: Vec<u8>,
    ) -> Result<ToolResult, PluginError> {
        burn_fuel();
    }
}

export!(FuelBurner);

fn burn_fuel() -> ! {
    loop {
        core::hint::black_box(0_u32);
    }
}

fn catalog_digest(tools: &[ToolSpec]) -> Result<String, PluginError> {
    let mut items = Vec::new();
    for tool in tools {
        items.push(serde_json::json!({
            "id": tool.id,
            "model-name": tool.model_name,
            "title": tool.title,
            "description": tool.description,
            "input-schema-json": serde_json::from_slice::<serde_json::Value>(&tool.input_schema_json)
                .map_err(|_| plugin_error("plugin_registration_invalid", "tool JSON is invalid"))?,
            "output-schema-json": tool
                .output_schema_json
                .as_deref()
                .map(|bytes| serde_json::from_slice::<serde_json::Value>(bytes))
                .transpose()
                .map_err(|_| plugin_error("plugin_registration_invalid", "tool JSON is invalid"))?,
            "execution-mode": tool.execution_mode,
            "side-effect": tool.side_effect,
            "retry-safety": tool.retry_safety,
            "approval-policy-json": serde_json::from_slice::<serde_json::Value>(&tool.approval_policy_json)
                .map_err(|_| plugin_error("plugin_registration_invalid", "tool JSON is invalid"))?,
            "max-result-bytes": tool.max_result_bytes,
            "metadata-json": serde_json::from_slice::<serde_json::Value>(&tool.metadata_json)
                .map_err(|_| plugin_error("plugin_registration_invalid", "tool JSON is invalid"))?,
        }));
    }
    let encoded = serde_json::to_vec(&serde_json::json!({ "tools": items }))
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog canonicalization failed"))?;
    let value: serde_json::Value = serde_json::from_slice(&encoded)
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog JSON is invalid"))?;
    let canonical = serde_json_canonicalizer::to_vec(&value)
        .map_err(|_| plugin_error("plugin_registration_invalid", "catalog JCS failed"))?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"finstack-ai");
    hasher.update([0]);
    hasher.update(b"raw-json");
    hasher.update([0]);
    hasher.update(1_u32.to_be_bytes());
    hasher.update([0]);
    hasher.update(&canonical);
    Ok(hasher.finalize().iter().fold(String::new(), |mut hex, byte| {
        hex.push_str(&format!("{byte:02x}"));
        hex
    }))
}

fn plugin_error(code: &str, message: &str) -> PluginError {
    PluginError {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}
