//! Tool-call and tool-result content blocks.

use serde::de;
use serde::{Deserialize, Serialize};

use crate::ids::ToolCallId;
use crate::raw_json::RawJson;

use super::content_block::ToolResultContentItems;
use super::error::ContentError;
use super::error::validate_content_items;
use super::text::{LABEL_MAX_BYTES, validated_label};
use super::{BoundedString, ContentBlock};
use serde::Deserializer;
use std::sync::Arc;

/// Assistant tool-call content block.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ToolCallBlock {
    tool_call_id: ToolCallId,
    tool_name: Arc<str>,
    arguments: RawJson,
}

impl ToolCallBlock {
    /// Construct a tool-call block.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when `tool_name` is empty, oversized, or contains NUL.
    pub fn try_new(
        tool_call_id: ToolCallId,
        tool_name: impl AsRef<str>,
        arguments: RawJson,
    ) -> Result<Self, ContentError> {
        let tool_name = validated_label(tool_name.as_ref(), "tool_name")?;
        Ok(Self {
            tool_call_id,
            tool_name,
            arguments,
        })
    }

    /// Tool-call identity.
    #[must_use]
    pub const fn tool_call_id(&self) -> &ToolCallId {
        &self.tool_call_id
    }

    /// Model-facing tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Tool arguments JSON.
    #[must_use]
    pub const fn arguments(&self) -> &RawJson {
        &self.arguments
    }
}

impl<'de> Deserialize<'de> for ToolCallBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct BinaryWire {
            tool_call_id: ToolCallId,
            tool_name: BoundedString<LABEL_MAX_BYTES>,
            arguments: RawJson,
        }

        let wire = BinaryWire::deserialize(deserializer)?;
        Self::try_new(
            wire.tool_call_id,
            wire.tool_name.into_inner(),
            wire.arguments,
        )
        .map_err(de::Error::custom)
    }
}

/// Tool-result content block associated with a prior tool call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ToolResultBlock {
    tool_call_id: ToolCallId,
    content: Arc<[ContentBlock]>,
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    is_error: bool,
}

impl ToolResultBlock {
    /// Construct a tool-result block.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when nested content is oversized or contains nested
    /// tool-call/tool-result blocks.
    pub fn try_new(
        tool_call_id: ToolCallId,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Result<Self, ContentError> {
        validate_content_items(&content)?;
        for block in &content {
            if matches!(
                block,
                ContentBlock::ToolCall(_) | ContentBlock::ToolResult(_)
            ) {
                return Err(ContentError::NestedToolBlock);
            }
        }
        Ok(Self {
            tool_call_id,
            content: Arc::from(content),
            is_error,
        })
    }

    /// Associated tool-call identity.
    #[must_use]
    pub const fn tool_call_id(&self) -> &ToolCallId {
        &self.tool_call_id
    }

    /// Result content blocks.
    #[must_use]
    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    /// Whether the tool reported an error outcome.
    #[must_use]
    pub const fn is_error(&self) -> bool {
        self.is_error
    }
}

impl<'de> Deserialize<'de> for ToolResultBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            tool_call_id: ToolCallId,
            content: ToolResultContentItems,
            #[serde(default)]
            is_error: bool,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.tool_call_id, wire.content.into_inner(), wire.is_error)
            .map_err(de::Error::custom)
    }
}
