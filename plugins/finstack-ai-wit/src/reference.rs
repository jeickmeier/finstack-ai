//! In-process reference toolset that implements the generated `toolset` export.

use crate::error::WitMapError;
use crate::generated::{CallContext, GuestToolset, PluginError, ToolCatalog, ToolResult, ToolSpec};
use crate::limits::{MAX_RAW_JSON_BYTES, reject_before_allocation};
use crate::mapping::catalog_digest_hex;

const ADD_ID: &str = "finstack.plugin.add";
const ECHO_ID: &str = "finstack.plugin.echo";

/// Two-tool in-process guest used to prove A02 without Wasmtime.
#[derive(Debug, Default)]
pub struct ReferenceToolset;

impl ReferenceToolset {
    /// Construct the reference guest.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn tools() -> [ToolSpec; 2] {
        [add_spec(), echo_spec()]
    }
}

impl GuestToolset for ReferenceToolset {
    fn list_tools(&self) -> Result<ToolCatalog, PluginError> {
        let tools = Self::tools().to_vec();
        let digest = catalog_digest_hex(&tools).map_err(|error| map_error(&error))?;
        Ok(ToolCatalog { digest, tools })
    }

    fn call(
        &self,
        context: &CallContext,
        tool_id: &str,
        args_json: &[u8],
    ) -> Result<ToolResult, PluginError> {
        reject_before_allocation(args_json, MAX_RAW_JSON_BYTES, "args-json")
            .map_err(|error| map_error(&error))?;
        if context.tenant_scope.is_empty() || context.authorization_decision_id.is_empty() {
            return Err(plugin_error(
                "plugin_call_context_invalid",
                "sanitized call-context is incomplete",
            ));
        }
        match tool_id {
            ADD_ID => add(args_json),
            ECHO_ID => echo(args_json),
            _ => Err(plugin_error("plugin_unknown_tool", "unknown tool id")),
        }
    }
}

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
    let args: AddArgs = serde_json::from_slice(args_json)
        .map_err(|_| plugin_error("plugin_arguments_invalid", "add arguments are invalid"))?;
    let sum = args
        .a
        .checked_add(args.b)
        .ok_or_else(|| plugin_error("plugin_arguments_invalid", "add overflowed"))?;
    encode_result(&serde_json::json!({ "sum": sum }))
}

fn echo(args_json: &[u8]) -> Result<ToolResult, PluginError> {
    let args: EchoArgs = serde_json::from_slice(args_json)
        .map_err(|_| plugin_error("plugin_arguments_invalid", "echo arguments are invalid"))?;
    reject_before_allocation(args.text.as_bytes(), MAX_RAW_JSON_BYTES, "echo.text")
        .map_err(|error| map_error(&error))?;
    encode_result(&serde_json::json!({ "text": args.text }))
}

fn encode_result(value: &serde_json::Value) -> Result<ToolResult, PluginError> {
    let content_json = serde_json::to_vec(value)
        .map_err(|_| plugin_error("plugin_result_invalid", "result encoding failed"))?;
    reject_before_allocation(&content_json, MAX_RAW_JSON_BYTES, "content-json")
        .map_err(|error| map_error(&error))?;
    Ok(ToolResult {
        content_json,
        is_error: false,
    })
}

fn map_error(error: &WitMapError) -> PluginError {
    plugin_error(error.code(), &error.to_string())
}

fn plugin_error(code: &str, message: &str) -> PluginError {
    PluginError {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable: false,
    }
}

#[derive(serde::Deserialize)]
struct AddArgs {
    a: i64,
    b: i64,
}

#[derive(serde::Deserialize)]
struct EchoArgs {
    text: String,
}

#[cfg(test)]
mod tests {
    use super::{ADD_ID, ECHO_ID, ReferenceToolset};
    use crate::generated::{CallContext, GuestToolset};
    use crate::limits::MAX_RAW_JSON_BYTES;
    use crate::mapping::register_catalog;

    fn context() -> CallContext {
        CallContext {
            effect_id: "00000000-0000-0000-0000-000000000004".to_owned(),
            session_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            lane_id: "00000000-0000-0000-0000-000000000002".to_owned(),
            run_id: "00000000-0000-0000-0000-000000000003".to_owned(),
            tenant_scope: "tenant-a".to_owned(),
            principal_issuer: "issuer".to_owned(),
            principal_subject: "subject".to_owned(),
            authorization_decision_id: "decision-v1".to_owned(),
            permitted_scopes: vec!["tenant-a".to_owned()],
            budget_scope_id: None,
            deadline_unix_ms: None,
        }
    }

    #[test]
    fn lists_and_executes_two_tools_in_process() {
        let toolset = ReferenceToolset::new();
        let catalog = toolset.list_tools().expect("list");
        assert_eq!(catalog.tools.len(), 2);
        let native = register_catalog(&catalog).expect("register");
        assert_eq!(native.len(), 2);
        let added = toolset
            .call(&context(), ADD_ID, br#"{"a":2,"b":3}"#)
            .expect("add");
        assert_eq!(added.content_json, br#"{"sum":5}"#);
        let echoed = toolset
            .call(&context(), ECHO_ID, br#"{"text":"ok"}"#)
            .expect("echo");
        assert_eq!(echoed.content_json, br#"{"text":"ok"}"#);
    }

    #[test]
    fn oversized_args_are_rejected_before_parse() {
        let payload = vec![b'x'; MAX_RAW_JSON_BYTES + 1];
        let error = ReferenceToolset::new()
            .call(&context(), ADD_ID, &payload)
            .expect_err("oversize");
        assert_eq!(error.code, "plugin_payload_too_large");
    }
}
