//! Map MCP `input_required` / elicitation onto existing HITL types.

use finstack_ai_kernel::{
    ComponentId, ComponentRef, ContentBlock, EffectId, InteractionId, InteractionKind,
    InteractionRequest, TextBlock, Version,
};
use finstack_ai_kernel::{Metadata, RawJson};

use crate::protocol::CallToolResult;
use crate::{MCP_PROTOCOL_VIOLATION, McpError};

const POLICY_COMPONENT: &str = "finstack.tools.mcp";

/// Build a durable [`InteractionRequest`] from an MCP `input_required` result.
///
/// Form when `inputRequests` is a JSON object; `FreeText` otherwise. This does
/// not invent a [`finstack_ai_kernel::RecordBody`]. Sampling stays rejected at the call site.
///
/// # Errors
///
/// Returns [`MCP_PROTOCOL_VIOLATION`] when the request cannot be constructed.
pub(crate) fn interaction_from_input_required(
    effect_id: EffectId,
    result: &CallToolResult,
) -> Result<InteractionRequest, McpError> {
    let (kind, schema_value) = match &result.input_requests {
        Some(schema) if schema.is_object() => (InteractionKind::Form, schema.clone()),
        _ => (
            InteractionKind::FreeText,
            serde_json::json!({"type": "object"}),
        ),
    };
    let prompt = prompt_from_result(result)?;
    let response_schema = RawJson::parse(
        serde_json_canonicalizer::to_vec(&schema_value).map_err(|_| {
            McpError::stable(MCP_PROTOCOL_VIOLATION, "elicitation schema is not json")
        })?,
    )
    .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "elicitation schema is not raw json"))?;
    let component = ComponentId::parse(POLICY_COMPONENT)
        .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "MCP policy component is invalid"))?;
    InteractionRequest::try_new(
        1,
        InteractionId::from_bytes(effect_id.to_bytes()),
        effect_id,
        kind,
        vec![ContentBlock::Text(prompt)],
        response_schema,
        ComponentRef::new(component, None),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        None,
        false,
        Metadata::empty(),
    )
    .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "MCP elicitation request is invalid"))
}

fn prompt_from_result(result: &CallToolResult) -> Result<TextBlock, McpError> {
    let text = result
        .content
        .iter()
        .find_map(|block| match block {
            crate::protocol::ContentBlock::Text { text } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("MCP tool requested additional input");
    TextBlock::try_new(text)
        .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "MCP elicitation prompt is invalid"))
}
