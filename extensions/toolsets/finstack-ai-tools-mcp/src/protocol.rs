//! Crate-private MCP wire types for revision 2026-07-28.

use serde::{Deserialize, Serialize};

/// Wire protocol revision this client speaks.
pub(crate) const PROTOCOL_VERSION: &str = "2026-07-28";

/// Required per-request metadata. Replaces the removed `initialize` handshake.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Meta {
    #[serde(rename = "io.modelcontextprotocol/protocolVersion")]
    pub(crate) protocol_version: &'static str,
    #[serde(rename = "io.modelcontextprotocol/clientCapabilities")]
    pub(crate) client_capabilities: ClientCapabilities,
    #[serde(
        rename = "io.modelcontextprotocol/clientInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) client_info: Option<Implementation>,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            client_capabilities: ClientCapabilities::default(),
            client_info: Some(Implementation {
                name: "finstack-ai-tools-mcp",
                version: env!("CARGO_PKG_VERSION"),
            }),
        }
    }
}

/// Accurately declared client capabilities. Understating these makes the server
/// reject the request with -32021 `MissingRequiredClientCapability`.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ClientCapabilities {}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Implementation {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
}

/// Result freshness discriminator. An ABSENT value means `Complete`; any
/// UNRECOGNIZED value must be treated as an error, not silently accepted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResultType {
    #[default]
    Complete,
    InputRequired,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListToolsResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) tools: Vec<Tool>,
    /// Opaque. An EMPTY STRING is a valid cursor — terminate only on `None`.
    #[serde(default)]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tool {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    /// REQUIRED by the schema; root MUST be `type: "object"`.
    pub(crate) input_schema: serde_json::Value,
    #[serde(default)]
    pub(crate) output_schema: Option<serde_json::Value>,
    /// HINTS ONLY. Never map directly onto `SideEffectClass` or `RetrySafety`.
    #[serde(default)]
    pub(crate) annotations: Option<ToolAnnotations>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "wire names match the MCP schema")]
pub(crate) struct ToolAnnotations {
    #[serde(default)]
    pub(crate) read_only_hint: Option<bool>,
    #[serde(default)]
    pub(crate) idempotent_hint: Option<bool>,
    #[serde(default)]
    pub(crate) destructive_hint: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListResourcesResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) resources: Vec<Resource>,
    /// Opaque. An EMPTY STRING is a valid cursor — terminate only on `None`.
    #[serde(default)]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Resource {
    pub(crate) uri: String,
    pub(crate) name: String,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted from the wire snapshot; not mapped onto ContextItem authority"
    )]
    pub(crate) title: Option<String>,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted from the wire snapshot; not mapped onto ContextItem authority"
    )]
    pub(crate) description: Option<String>,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted from the wire snapshot; not mapped onto ContextItem authority"
    )]
    pub(crate) mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadResourceResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) contents: Vec<ResourceContents>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceContents {
    pub(crate) uri: String,
    #[serde(default)]
    pub(crate) mime_type: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) blob: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CallToolResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) content: Vec<ContentBlock>,
    #[serde(default)]
    pub(crate) structured_content: Option<serde_json::Value>,
    /// Tool-level error channel. Absent means false. This arrives on a JSON-RPC
    /// SUCCESS response, not in the `error` member.
    #[serde(default)]
    pub(crate) is_error: bool,
    /// Present when `result_type` is `input_required`.
    #[serde(default)]
    pub(crate) input_requests: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListPromptsResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) prompts: Vec<Prompt>,
    #[serde(default)]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Prompt {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListResourceTemplatesResult {
    #[serde(default)]
    pub(crate) result_type: ResultType,
    #[serde(default)]
    pub(crate) resource_templates: Vec<ResourceTemplate>,
    #[serde(default)]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceTemplate {
    pub(crate) name: String,
    pub(crate) uri_template: String,
}

/// Exactly five spec variants plus a forward-compatible fallback.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Audio {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    ResourceLink {
        uri: String,
    },
    EmbeddedResource {
        resource: serde_json::Value,
    },
    #[serde(other)]
    Unknown,
}

pub(crate) fn jsonrpc_request(
    id: &serde_json::Value,
    method: &str,
    params: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

pub(crate) fn content_to_json(block: &ContentBlock) -> serde_json::Value {
    match block {
        ContentBlock::Text { text } => serde_json::json!({"type": "text", "text": text}),
        ContentBlock::Image { data, mime_type } => {
            serde_json::json!({"type": "image", "data": data, "mimeType": mime_type})
        }
        ContentBlock::Audio { data, mime_type } => {
            serde_json::json!({"type": "audio", "data": data, "mimeType": mime_type})
        }
        ContentBlock::ResourceLink { uri } => {
            serde_json::json!({"type": "resource_link", "uri": uri})
        }
        ContentBlock::EmbeddedResource { resource } => {
            serde_json::json!({"type": "embedded_resource", "resource": resource})
        }
        ContentBlock::Unknown => serde_json::json!({"type": "unknown"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_content_block_type_does_not_fail_deserialization() {
        let json = r#"{"type":"future_block","payload":42}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("unknown type must parse");
        assert!(matches!(block, ContentBlock::Unknown));
    }

    #[test]
    fn empty_string_cursor_is_a_valid_cursor() {
        let json = r#"{"resultType":"complete","tools":[],"nextCursor":""}"#;
        let result: ListToolsResult = serde_json::from_str(json).expect("parses");
        assert_eq!(result.next_cursor.as_deref(), Some(""));
    }

    #[test]
    fn absent_result_type_is_treated_as_complete() {
        let json = r#"{"tools":[]}"#;
        let result: ListToolsResult = serde_json::from_str(json).expect("parses");
        assert_eq!(result.result_type, ResultType::Complete);
    }

    #[test]
    fn empty_string_resource_cursor_is_a_valid_cursor() {
        let json = r#"{"resultType":"complete","resources":[],"nextCursor":""}"#;
        let result: ListResourcesResult = serde_json::from_str(json).expect("parses");
        assert_eq!(result.next_cursor.as_deref(), Some(""));
    }
}
