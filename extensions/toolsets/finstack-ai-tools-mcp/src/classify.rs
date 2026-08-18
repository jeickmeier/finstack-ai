//! Conservative classification and catalog freeze helpers.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, Digest, Metadata, RawJson, RetrySafety, SideEffectClass,
    ToolExecutionMode, ToolId, ToolSpec,
};

use crate::protocol::{ListPromptsResult, ListToolsResult, Prompt, ResultType, Tool};
use crate::transport::McpTransport;
use crate::{MCP_PROTOCOL_VIOLATION, MCP_RESULT_UNSUPPORTED, McpConfig, McpError};

pub(crate) const MAX_LIST_PAGES: usize = 64;
const TOOL_ID_PREFIX: &str = "mcp.";

pub(crate) async fn enumerate_catalog(transport: &dyn McpTransport) -> Result<Vec<Tool>, McpError> {
    let mut tools = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_LIST_PAGES {
        let mut params = serde_json::json!({});
        if let Some(cursor) = cursor.as_ref() {
            params.as_object_mut().expect("object").insert(
                "cursor".to_owned(),
                serde_json::Value::String(cursor.clone()),
            );
        }
        let value = transport.request("tools/list", params).await?;
        let page: ListToolsResult = serde_json::from_value(value).map_err(|error| {
            McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                format!("tools/list result is invalid: {error}"),
            )
        })?;
        if page.result_type != ResultType::Complete {
            return Err(McpError::stable(
                MCP_RESULT_UNSUPPORTED,
                "tools/list resultType is not complete",
            ));
        }
        for tool in page.tools {
            reject_network_refs(&tool.input_schema)?;
            if let Some(output) = &tool.output_schema {
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
            tools.push(tool);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(tools),
        }
    }
    Err(McpError::stable(
        MCP_PROTOCOL_VIOLATION,
        "tools/list exceeded the page cap",
    ))
}

pub(crate) async fn enumerate_prompts(
    transport: &dyn McpTransport,
) -> Result<Vec<Prompt>, McpError> {
    let mut prompts = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_LIST_PAGES {
        let mut params = serde_json::json!({});
        if let Some(cursor) = cursor.as_ref() {
            params.as_object_mut().expect("object").insert(
                "cursor".to_owned(),
                serde_json::Value::String(cursor.clone()),
            );
        }
        let value = match transport.request("prompts/list", params).await {
            Ok(value) => value,
            Err(error) if optional_catalog_missing(&error) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let page: ListPromptsResult = serde_json::from_value(value).map_err(|error| {
            McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                format!("prompts/list result is invalid: {error}"),
            )
        })?;
        if page.result_type != ResultType::Complete {
            return Err(McpError::stable(
                MCP_RESULT_UNSUPPORTED,
                "prompts/list resultType is not complete",
            ));
        }
        for prompt in page.prompts {
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
            prompts.push(prompt);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(prompts),
        }
    }
    Err(McpError::stable(
        MCP_PROTOCOL_VIOLATION,
        "prompts/list exceeded the page cap",
    ))
}

pub(crate) fn optional_catalog_missing(error: &McpError) -> bool {
    let message = error.message();
    message.contains("-32601") || message.contains("Method not found")
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
        model_name: std::sync::Arc::from(tool.name.as_str()),
        title: std::sync::Arc::from(tool.title.as_deref().unwrap_or(tool.name.as_str())),
        description: std::sync::Arc::from(description),
        input_schema,
        output_schema,
        execution: ToolExecutionMode::Sequential,
        side_effect,
        retry_safety,
        approval,
        max_result_bytes: config.inline_result_bytes(),
        metadata,
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

fn reject_network_refs(value: &serde_json::Value) -> Result<(), McpError> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(reference)) = map.get("$ref")
                && is_network_ref(reference)
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

fn is_network_ref(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}
