//! Read-only filesystem sandbox fixture. Not the trusted native filesystem battery.

#![deny(unsafe_code)]
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

mod bindings {
    #![allow(unsafe_code, reason = "generated WIT and WASI ABI bindings")]

    finstack_ai_guest_sdk::wit_bindgen::generate!({
        world: "filesystem-sandbox",
        path: "wit",
        generate_all,
        runtime_path: "finstack_ai_guest_sdk::wit_bindgen::rt",
    });
}
use bindings::exports::finstack::ai_toolset::toolset::{Guest, ToolCatalog, ToolResult, ToolSpec};
use bindings::finstack::ai_types::types::{CallContext, PluginError};
use bindings::wasi::filesystem::preopens;
use bindings::wasi::filesystem::types::{Descriptor, DescriptorFlags, OpenFlags, PathFlags};
use finstack_ai_guest_sdk::{
    ToolSpecParts, catalog_digest, encode_json_result, parse_args, plugin_error,
    require_sanitized_context,
};
use serde::Deserialize;

const LIST_ID: &str = "finstack.plugin.filesystem.list";
const READ_ID: &str = "finstack.plugin.filesystem.read";
const MAX_READ: u64 = 1024;

struct FilesystemSandbox;

impl Guest for FilesystemSandbox {
    fn list_tools() -> Result<ToolCatalog, PluginError> {
        let tools = vec![list_spec(), read_spec()];
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
        let args: PathArgs = parse_args(&args_json).map_err(map_err)?;
        let path = validate_guest_path(&args.path)?;
        match tool_id.as_str() {
            LIST_ID => list_dir(&path),
            READ_ID => read_file(&path),
            _ => Err(map_err(plugin_error(
                "plugin_unknown_tool",
                "unknown tool id",
            ))),
        }
    }
}

mod generated_export {
    #![allow(unsafe_code, reason = "generated WIT ABI export")]

    use super::{FilesystemSandbox, bindings};

    bindings::export!(FilesystemSandbox with_types_in bindings);
}

#[derive(Deserialize)]
struct PathArgs {
    path: String,
}

fn validate_guest_path(path: &str) -> Result<String, PluginError> {
    if path.is_empty() || path.contains('\0') || path.starts_with('/') || path.contains("..") {
        return Err(map_err(plugin_error(
            "plugin_arguments_invalid",
            "sandbox path is invalid",
        )));
    }
    Ok(path.to_owned())
}

fn list_dir(path: &str) -> Result<ToolResult, PluginError> {
    let dir = open_at(path, true)?;
    let stream = dir
        .read_directory()
        .map_err(|_| map_err(plugin_error("plugin_arguments_invalid", "list failed")))?;
    let mut entries = Vec::new();
    loop {
        match stream.read_directory_entry() {
            Ok(Some(entry)) => entries.push(entry.name),
            Ok(None) => break,
            Err(_) => {
                return Err(map_err(plugin_error(
                    "plugin_arguments_invalid",
                    "list failed",
                )));
            }
        }
    }
    let content_json =
        encode_json_result(&serde_json::json!({ "entries": entries })).map_err(map_err)?;
    Ok(ToolResult {
        content_json,
        is_error: false,
    })
}

fn read_file(path: &str) -> Result<ToolResult, PluginError> {
    let file = open_at(path, false)?;
    let (bytes, _) = file
        .read(MAX_READ, 0)
        .map_err(|_| map_err(plugin_error("plugin_arguments_invalid", "read failed")))?;
    let text = String::from_utf8_lossy(&bytes);
    let content_json =
        encode_json_result(&serde_json::json!({ "bytes": text })).map_err(map_err)?;
    Ok(ToolResult {
        content_json,
        is_error: false,
    })
}

fn open_at(
    path: &str,
    directory: bool,
) -> Result<Descriptor, PluginError> {
    let dirs = preopens::get_directories();
    let Some((root, _)) = dirs.into_iter().next() else {
        return Err(map_err(plugin_error(
            "plugin_arguments_invalid",
            "no preopen",
        )));
    };
    if path == "." {
        return Ok(root);
    }
    let open_flags = if directory {
        OpenFlags::DIRECTORY
    } else {
        OpenFlags::empty()
    };
    root.open_at(PathFlags::empty(), path, open_flags, DescriptorFlags::READ)
        .map_err(|_| map_err(plugin_error("plugin_arguments_invalid", "open failed")))
}

fn list_spec() -> ToolSpec {
    tool_spec(LIST_ID, "list", "List entries under the granted preopen")
}

fn read_spec() -> ToolSpec {
    tool_spec(READ_ID, "read", "Read a file under the granted preopen")
}

fn tool_spec(id: &str, model_name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        id: id.to_owned(),
        model_name: model_name.to_owned(),
        title: model_name.to_owned(),
        description: description.to_owned(),
        input_schema_json:
            br#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#
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
