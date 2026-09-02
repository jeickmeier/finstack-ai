//! Tool-call and tool-result content blocks.

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::RawJson;
use crate::primitives::ToolCallId;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_call_id: Option<Arc<str>>,
}

impl ToolCallBlock {
    /// Construct a tool-call block.
    ///
    /// # Arguments
    ///
    /// * `tool_call_id` - Stable identity for this assistant call.
    /// * `tool_name` - Model-facing tool name (non-empty, ≤ [`LABEL_MAX_BYTES`], no NUL).
    /// * `arguments` - Canonical JSON arguments object or value.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when `tool_name` is empty, oversized, or contains NUL.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{RawJson, ToolCallBlock, ToolCallId};
    ///
    /// let call = ToolCallBlock::try_new(
    ///     ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
    ///     "search.web",
    ///     RawJson::parse("{}").expect("json"),
    /// )
    /// .expect("call");
    /// assert_eq!(call.tool_name(), "search.web");
    /// ```
    pub fn try_new(
        tool_call_id: ToolCallId,
        tool_name: impl AsRef<str>,
        arguments: RawJson,
    ) -> Result<Self, ContentError> {
        Self::try_new_with_provider_call_id(tool_call_id, tool_name, arguments, None::<&str>)
    }

    /// Construct a tool-call block that preserves a provider-native call id.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when `tool_name` or `provider_call_id` is empty,
    /// oversized, or contains NUL.
    pub fn try_new_with_provider_call_id(
        tool_call_id: ToolCallId,
        tool_name: impl AsRef<str>,
        arguments: RawJson,
        provider_call_id: Option<impl AsRef<str>>,
    ) -> Result<Self, ContentError> {
        let tool_name = validated_label(tool_name.as_ref(), "tool_name")?;
        let provider_call_id = provider_call_id
            .map(|value| validated_label(value.as_ref(), "provider_call_id"))
            .transpose()?;
        Ok(Self {
            tool_call_id,
            tool_name,
            arguments,
            provider_call_id,
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

    /// Provider-native call identity, when the model supplied one.
    #[must_use]
    pub fn provider_call_id(&self) -> Option<&str> {
        self.provider_call_id.as_deref()
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
            #[serde(default)]
            provider_call_id: Option<BoundedString<LABEL_MAX_BYTES>>,
        }

        let wire = BinaryWire::deserialize(deserializer)?;
        Self::try_new_with_provider_call_id(
            wire.tool_call_id,
            wire.tool_name.into_inner(),
            wire.arguments,
            wire.provider_call_id.map(BoundedString::into_inner),
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
    /// # Arguments
    ///
    /// * `tool_call_id` - Identity of the matching assistant tool call.
    /// * `content` - Nested content blocks. Must not contain tool-call or
    ///   tool-result blocks, and must stay within the v1 item ceiling.
    /// * `is_error` - Whether this result is a tool-level error payload.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when nested content is oversized or contains nested
    /// tool-call/tool-result blocks.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{ContentBlock, TextBlock, ToolCallId, ToolResultBlock};
    ///
    /// let result = ToolResultBlock::try_new(
    ///     ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
    ///     vec![ContentBlock::Text(TextBlock::try_new("ok").expect("text"))],
    ///     false,
    /// )
    /// .expect("result");
    /// assert!(!result.is_error());
    /// ```
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
