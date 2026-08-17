//! Tagged `ContentBlock` enum and deserialize machinery.

use serde::de::{self, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::error::ContentError;
use super::opaque::OpaqueBlock;
use super::text::CONTENT_MAX_ITEMS;
use super::tool::{ToolCallBlock, ToolResultBlock};
use super::{JsonBlock, MediaRef, TextBlock};
use core::fmt;

/// Canonical provider-neutral content block.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text(TextBlock),
    /// Structured JSON.
    Json(JsonBlock),
    /// Image by blob reference.
    Image(MediaRef),
    /// Audio by blob reference.
    Audio(MediaRef),
    /// File/document by blob reference.
    File(MediaRef),
    /// Assistant tool call.
    ToolCall(ToolCallBlock),
    /// Tool result associated with a tool call.
    ToolResult(ToolResultBlock),
    /// Provider-opaque extension payload.
    Opaque(OpaqueBlock),
}

fn reject_oversized_content_hint<E>(hint: Option<usize>) -> Result<(), E>
where
    E: de::Error,
{
    if let Some(length) = hint
        && length > CONTENT_MAX_ITEMS
    {
        return Err(E::custom(ContentError::TooManyItems {
            len: length,
            max: CONTENT_MAX_ITEMS,
        }));
    }
    Ok(())
}

fn reject_trailing_content<'de, A>(sequence: &mut A) -> Result<(), A::Error>
where
    A: SeqAccess<'de>,
{
    if sequence.next_element::<IgnoredAny>()?.is_some() {
        return Err(de::Error::custom(ContentError::TooManyItems {
            len: CONTENT_MAX_ITEMS + 1,
            max: CONTENT_MAX_ITEMS,
        }));
    }
    Ok(())
}

pub(super) struct ToolResultContentItems(Vec<ContentBlock>);

impl ToolResultContentItems {
    pub(super) fn into_inner(self) -> Vec<ContentBlock> {
        self.0
    }
}

impl<'de> Deserialize<'de> for ToolResultContentItems {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Do not branch on `is_human_readable()`. Internally tagged
        // `ContentBlock` uses serde's ContentDeserializer, which reports
        // human-readable even on canonical CBOR (see `BoundedString`).
        deserializer.deserialize_seq(ToolResultContentVisitor)
    }
}

struct ToolResultContentVisitor;

impl<'de> Visitor<'de> for ToolResultContentVisitor {
    type Value = ToolResultContentItems;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded array of non-tool content blocks")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        reject_oversized_content_hint::<A::Error>(sequence.size_hint())?;
        let capacity = sequence.size_hint().unwrap_or(0).min(CONTENT_MAX_ITEMS);
        let mut content = Vec::with_capacity(capacity);
        while content.len() < CONTENT_MAX_ITEMS {
            let Some(block) = sequence.next_element::<ContentBlock>()? else {
                return Ok(ToolResultContentItems(content));
            };
            if matches!(
                block,
                ContentBlock::ToolCall(_) | ContentBlock::ToolResult(_)
            ) {
                return Err(de::Error::custom(ContentError::NestedToolBlock));
            }
            content.push(block);
        }
        reject_trailing_content(&mut sequence)?;
        Ok(ToolResultContentItems(content))
    }
}

impl ContentBlock {
    /// Stable kind name used in diagnostics and fixtures.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Json(_) => "json",
            Self::Image(_) => "image",
            Self::Audio(_) => "audio",
            Self::File(_) => "file",
            Self::ToolCall(_) => "tool_call",
            Self::ToolResult(_) => "tool_result",
            Self::Opaque(_) => "opaque",
        }
    }
}
