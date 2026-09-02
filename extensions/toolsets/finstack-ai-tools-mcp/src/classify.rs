//! Conservative classification and catalog freeze helpers.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{Digest, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId};
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolDeferralSupport, ToolSpec,
};

use crate::protocol::{ListPage, ListPromptsResult, ListToolsResult, Prompt, ResultType, Tool};
use crate::transport::McpTransport;
use crate::{
    MCP_LIMIT_EXCEEDED, MCP_PROTOCOL_VIOLATION, MCP_RESULT_UNSUPPORTED, McpConfig, McpError,
};

const MAX_LIST_PAGES: usize = 64;
/// Serialized-size cap for one server-provided tool schema. Like the
/// stdio `MAX_LINE_BYTES` cap, this is a fixed fail-closed bound with no
/// runtime override: an untrusted server must not be able to inflate the
/// frozen catalog with an arbitrarily large `inputSchema`/`outputSchema`.
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// Nesting-depth cap for one server-provided tool schema. Bounds the
/// recursive schema walks (`$ref` scan, canonicalization) against an
/// adversarially deep document.
const MAX_SCHEMA_DEPTH: usize = 32;
const TOOL_ID_PREFIX: &str = "mcp.";

/// Walk every page of a cursor-paginated `method`, validating each item
/// with `accept` before it is kept. An `optional` catalog answers `-32601`
/// (method not found) with an empty list instead of an error.
pub(crate) async fn list_all<P: ListPage>(
    transport: &dyn McpTransport,
    method: &str,
    optional: bool,
    mut accept: impl FnMut(&P::Item) -> Result<(), McpError>,
) -> Result<Vec<P::Item>, McpError> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_LIST_PAGES {
        let params = match &cursor {
            Some(cursor) => serde_json::json!({ "cursor": cursor }),
            None => serde_json::json!({}),
        };
        let value = match transport.request(method, params).await {
            Ok(value) => value,
            Err(error) if optional && error.jsonrpc_code() == Some(-32601) => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(error),
        };
        let page: P = serde_json::from_value(value).map_err(|error| {
            McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                format!("{method} result is invalid: {error}"),
            )
        })?;
        let (result_type, page_items, next_cursor) = page.into_parts();
        if result_type != ResultType::Complete {
            return Err(McpError::stable(
                MCP_RESULT_UNSUPPORTED,
                format!("{method} resultType is not complete"),
            ));
        }
        for item in page_items {
            accept(&item)?;
            items.push(item);
        }
        match next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(items),
        }
    }
    Err(McpError::stable(
        MCP_PROTOCOL_VIOLATION,
        format!("{method} exceeded the page cap"),
    ))
}

pub(crate) async fn enumerate_catalog(transport: &dyn McpTransport) -> Result<Vec<Tool>, McpError> {
    let mut seen = BTreeSet::new();
    list_all::<ListToolsResult>(transport, "tools/list", false, |tool| {
        enforce_schema_limits(&tool.input_schema)?;
        reject_network_refs(&tool.input_schema)?;
        if let Some(output) = &tool.output_schema {
            enforce_schema_limits(output)?;
            reject_network_refs(output)?;
        }
        if tool.name.is_empty() {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "tools/list returned an empty tool name",
            ));
        }
        if !seen.insert(tool.name.clone()) {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "tools/list returned a duplicate tool name",
            ));
        }
        Ok(())
    })
    .await
}

pub(crate) async fn enumerate_prompts(
    transport: &dyn McpTransport,
) -> Result<Vec<Prompt>, McpError> {
    let mut seen = BTreeSet::new();
    list_all::<ListPromptsResult>(transport, "prompts/list", true, |prompt| {
        if prompt.name.is_empty() {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "prompts/list returned an empty prompt name",
            ));
        }
        if !seen.insert(prompt.name.clone()) {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "prompts/list returned a duplicate prompt name",
            ));
        }
        Ok(())
    })
    .await
}

pub(crate) fn catalog_digest(tools: &[Tool]) -> Digest {
    let triples = tools
        .iter()
        .map(|tool| {
            serde_json::json!([
                tool.name,
                tool.input_schema,
                tool.output_schema
                    .clone()
                    .unwrap_or(serde_json::Value::Null)
            ])
        })
        .collect::<Vec<_>>();
    let bytes = serde_json_canonicalizer::to_vec(&triples).unwrap_or_else(|_| Vec::from(b"[]"));
    Digest::raw_json(&bytes)
}

pub(crate) fn invocation_digest(server_identity: &str, tools: &[Tool]) -> Digest {
    let names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "server": server_identity,
        "tools": names,
        "catalog": catalog_digest(tools).to_string(),
    });
    let bytes = serde_json_canonicalizer::to_vec(&payload).unwrap_or_else(|_| Vec::from(b"{}"));
    Digest::raw_json(&bytes)
}

pub(crate) fn to_tool_spec(tool: &Tool, config: &McpConfig) -> Result<ToolSpec, McpError> {
    if tool
        .input_schema
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("object")
    {
        return Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "inputSchema root must be type object",
        ));
    }
    let id = tool_id(&tool.name)?;
    let description = tool
        .description
        .as_deref()
        .or(tool.title.as_deref())
        .unwrap_or(tool.name.as_str());
    let input_schema = raw_json(&tool.input_schema)?;
    let output_schema = tool.output_schema.as_ref().map(raw_json).transpose()?;
    let read_only = config.is_read_only(&tool.name);
    let idempotent = config.is_idempotent(&tool.name);
    let (side_effect, retry_safety, approval) = if read_only {
        (
            SideEffectClass::ReadOnly,
            RetrySafety::SafeToRetry,
            ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
        )
    } else if idempotent {
        (
            SideEffectClass::IdempotentWrite,
            RetrySafety::SafeToRetry,
            ApprovalMetadata {
                requirement: ApprovalRequirement::Required,
                reason: Some(Arc::from(
                    "MCP tool is host-declared idempotent but still approval-required",
                )),
                attributes: Metadata::empty(),
            },
        )
    } else {
        (
            SideEffectClass::NonIdempotentWrite,
            RetrySafety::AtMostOnce,
            ApprovalMetadata {
                requirement: ApprovalRequirement::Required,
                reason: Some(Arc::from(
                    "MCP tools default to side-effecting, not retry-safe, approval-required",
                )),
                attributes: Metadata::empty(),
            },
        )
    };
    let metadata = annotation_metadata(tool.annotations.as_ref());
    let spec = ToolSpec {
        id,
        model_name: Arc::from(tool.name.as_str()),
        title: Arc::from(tool.title.as_deref().unwrap_or(tool.name.as_str())),
        description: Arc::from(description),
        input_schema,
        output_schema,
        execution: ToolExecutionMode::Sequential,
        side_effect,
        retry_safety,
        approval,
        max_result_bytes: config.inline_result_bytes(),
        metadata,
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate().map_err(|error| {
        McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            format!("tool spec is invalid: {error}"),
        )
    })?;
    Ok(spec)
}

fn annotation_metadata(annotations: Option<&crate::protocol::ToolAnnotations>) -> Metadata {
    let Some(annotations) = annotations else {
        return Metadata::empty();
    };
    let value = serde_json::json!({
        "mcp_annotations": {
            "readOnlyHint": annotations.read_only_hint,
            "idempotentHint": annotations.idempotent_hint,
            "destructiveHint": annotations.destructive_hint,
        }
    });
    Metadata::parse(serde_json::to_vec(&value).unwrap_or_else(|_| Vec::from(b"{}")))
        .unwrap_or_else(|_| Metadata::empty())
}

fn tool_id(name: &str) -> Result<ToolId, McpError> {
    let mut sanitized = String::from(TOOL_ID_PREFIX);
    for (index, ch) in name.chars().enumerate() {
        let mapped = match ch {
            'A'..='Z' => ch.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' | '.' | '_' | '-' => ch,
            _ => '-',
        };
        if index == 0 && matches!(mapped, '0'..='9' | '.' | '_' | '-') {
            sanitized.push('t');
        }
        sanitized.push(mapped);
    }
    if sanitized == TOOL_ID_PREFIX {
        sanitized.push_str("tool");
    }
    sanitized.truncate(128);
    ToolId::parse(&sanitized).map_err(|_| {
        McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "MCP tool name is not a valid ToolId",
        )
    })
}

fn raw_json(value: &serde_json::Value) -> Result<RawJson, McpError> {
    let bytes = serde_json_canonicalizer::to_vec(value).map_err(|error| {
        McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            format!("schema canonicalization failed: {error}"),
        )
    })?;
    RawJson::parse(bytes).map_err(|error| {
        McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            format!("schema is not raw json: {error}"),
        )
    })
}

/// Fail closed on a server-provided schema that exceeds the fixed size or
/// nesting-depth bounds. Runs before any recursive walk of the schema so an
/// adversarially deep document never drives unbounded recursion.
fn enforce_schema_limits(value: &serde_json::Value) -> Result<(), McpError> {
    if schema_depth_exceeds(value, MAX_SCHEMA_DEPTH) {
        return Err(McpError::stable(
            MCP_LIMIT_EXCEEDED,
            "tool schema exceeds the nesting-depth limit",
        ));
    }
    let bytes = serde_json::to_vec(value).map_err(|error| {
        McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            format!("tool schema serialization failed: {error}"),
        )
    })?;
    if bytes.len() > MAX_SCHEMA_BYTES {
        return Err(McpError::stable(
            MCP_LIMIT_EXCEEDED,
            "tool schema exceeds the serialized byte limit",
        ));
    }
    Ok(())
}

/// True when `value` nests deeper than `remaining` levels. A leaf is depth 1;
/// recursion stops as soon as the budget is exhausted, so the walk itself is
/// bounded by the cap.
fn schema_depth_exceeds(value: &serde_json::Value, remaining: usize) -> bool {
    if remaining == 0 {
        return true;
    }
    match value {
        serde_json::Value::Object(map) => map
            .values()
            .any(|nested| schema_depth_exceeds(nested, remaining - 1)),
        serde_json::Value::Array(items) => items
            .iter()
            .any(|nested| schema_depth_exceeds(nested, remaining - 1)),
        _ => false,
    }
}

fn reject_network_refs(value: &serde_json::Value) -> Result<(), McpError> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(reference)) = map.get("$ref")
                && (reference.starts_with("http://") || reference.starts_with("https://"))
            {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "network $ref is rejected",
                ));
            }
            for nested in map.values() {
                reject_network_refs(nested)?;
            }
        }
        serde_json::Value::Array(items) => {
            for nested in items {
                reject_network_refs(nested)?;
            }
        }
        _ => {}
    }
    Ok(())
}
