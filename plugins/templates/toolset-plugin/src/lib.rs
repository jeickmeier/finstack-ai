//! Template `toolset-plugin` guest. Copy this crate to start a new component.

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
    reject_log_message, require_sanitized_context,
};
use serde::Deserialize;

const ECHO_ID: &str = "finstack.plugin.template.echo";

struct TemplateToolset;

impl Guest for TemplateToolset {
    fn list_tools() -> Result<ToolCatalog, PluginError> {
        let spec = echo_spec();
        let digest = catalog_digest(&[to_parts(&spec)]).map_err(map_err)?;
        Ok(ToolCatalog {
            digest,
            tools: vec![spec],
        })
    }

    fn call(
        context: CallContext,
        tool_id: String,
        args_json: Vec<u8>,
    ) -> Result<ToolResult, PluginError> {
        require_sanitized_context(&context.tenant_scope, &context.authorization_decision_id)
            .map_err(map_err)?;
        if reject_log_message("template echo").is_ok() {
            let _ = finstack::ai_host::logging::log(
                &context,
                finstack::ai_host::logging::Level::Info,
                "template echo",
            );
        }
        match tool_id.as_str() {
            ECHO_ID => echo(&args_json),
            _ => Err(map_err(plugin_error(
                "plugin_unknown_tool",
                "unknown tool id",
            ))),
        }
    }
}

export!(TemplateToolset);

#[derive(Deserialize)]
struct EchoArgs {
    text: String,
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
