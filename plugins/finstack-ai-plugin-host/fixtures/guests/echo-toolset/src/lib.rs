//! Test-only `toolset-plugin` guest. Not a published SDK or template.

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

finstack_ai_guest_sdk::toolset_plugin!();

use exports::finstack::ai_toolset::toolset::{Guest, ToolCatalog, ToolResult, ToolSpec};
use finstack::ai_types::types::{CallContext, PluginError};
use finstack_ai_guest_sdk::{
    ToolSpecParts, catalog_digest, encode_json_result, parse_args, plugin_error,
    require_sanitized_context,
};
use serde::Deserialize;

const ADD_ID: &str = "finstack.plugin.add";
const ECHO_ID: &str = "finstack.plugin.echo";

struct EchoToolset;

impl Guest for EchoToolset {
    fn list_tools() -> Result<ToolCatalog, PluginError> {
        let tools = vec![add_spec(), echo_spec()];
        let parts: Vec<ToolSpecParts> = tools.iter().map(to_parts).collect();
        let digest = catalog_digest(&parts).map_err(map_err)?;
        Ok(ToolCatalog { digest, tools })
    }

    fn call(
        context: CallContext,
        tool_id: String,
        args_json: Vec<u8>,
    ) -> Result<ToolResult, PluginError> {
        require_sanitized_context(&context.tenant_scope, &context.authorization_decision_id)
            .map_err(map_err)?;
        match tool_id.as_str() {
            ADD_ID => add(&args_json),
            ECHO_ID => echo(&args_json),
            _ => Err(map_err(plugin_error(
                "plugin_unknown_tool",
                "unknown tool id",
            ))),
        }
    }
}

export!(EchoToolset);

#[derive(Deserialize)]
struct AddArgs {
    a: i64,
    b: i64,
}

#[derive(Deserialize)]
struct EchoArgs {
    text: String,
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
    let args: AddArgs = parse_args(args_json).map_err(map_err)?;
    let sum = args
        .a
        .checked_add(args.b)
        .ok_or_else(|| map_err(plugin_error("plugin_arguments_invalid", "add overflowed")))?;
    let content_json = encode_json_result(&serde_json::json!({ "sum": sum })).map_err(map_err)?;
    Ok(ToolResult {
        content_json,
        is_error: false,
    })
}

fn echo(args_json: &[u8]) -> Result<ToolResult, PluginError> {
    let args: EchoArgs = parse_args(args_json).map_err(map_err)?;
    let content_json =
        encode_json_result(&serde_json::json!({ "text": args.text })).map_err(map_err)?;
    Ok(ToolResult {
        content_json,
        is_error: false,
    })
}

fn to_parts(spec: &ToolSpec) -> ToolSpecParts {
    ToolSpecParts {
        id: spec.id.clone(),
        model_name: spec.model_name.clone(),
        title: spec.title.clone(),
        description: spec.description.clone(),
        input_schema_json: spec.input_schema_json.clone(),
        output_schema_json: spec.output_schema_json.clone(),
        execution_mode: spec.execution_mode.clone(),
        side_effect: spec.side_effect.clone(),
        retry_safety: spec.retry_safety.clone(),
        approval_policy_json: spec.approval_policy_json.clone(),
        max_result_bytes: spec.max_result_bytes,
        metadata_json: spec.metadata_json.clone(),
    }
}

fn map_err(error: finstack_ai_guest_sdk::GuestError) -> PluginError {
    PluginError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}
