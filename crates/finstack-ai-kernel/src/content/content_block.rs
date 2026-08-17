//! Tagged `ContentBlock` enum and deserialize machinery.

use serde::de::{self, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::ToolCallId;
use crate::raw_json::RawJson;

use super::blob::BlobRef;
use super::blob::MediaRef;
use super::error::ContentError;
use super::opaque::{OpaqueBlock, OpaqueEncoding, OpaquePayload};
use super::text::{CONTENT_MAX_ITEMS, LABEL_MAX_BYTES};
use super::text::{TEXT_MAX_BYTES, hex_decode};
use super::tool::{ToolCallBlock, ToolResultBlock};
use super::{BoundedString, JsonBlock, TextBlock};
use core::fmt;

/// Canonical provider-neutral content block.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
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

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ContentKind {
    Text,
    Json,
    Image,
    Audio,
    File,
    ToolCall,
    ToolResult,
    Opaque,
}

#[derive(Deserialize)]
struct ContentKindProbe {
    kind: ContentKind,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaggedTextBlock {
    kind: ContentKind,
    text: BoundedString<TEXT_MAX_BYTES>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaggedJsonBlock {
    kind: ContentKind,
    value: StrictRawJson,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaggedMediaRef {
    kind: ContentKind,
    blob: BlobRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaggedToolCallBlock {
    kind: ContentKind,
    tool_call_id: ToolCallId,
    tool_name: BoundedString<LABEL_MAX_BYTES>,
    arguments: StrictRawJson,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaggedToolResultBlock {
    kind: ContentKind,
    tool_call_id: ToolCallId,
    content: ToolResultContentItems,
    #[serde(default)]
    is_error: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaggedOpaqueBlock {
    kind: ContentKind,
    media_type: BoundedString<LABEL_MAX_BYTES>,
    payload: StrictOpaquePayload,
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

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NonToolContentBlock {
    Text(TextBlock),
    Json(JsonBlock),
    Image(MediaRef),
    Audio(MediaRef),
    File(MediaRef),
    Opaque(OpaqueBlock),
}

impl From<NonToolContentBlock> for ContentBlock {
    fn from(value: NonToolContentBlock) -> Self {
        match value {
            NonToolContentBlock::Text(block) => Self::Text(block),
            NonToolContentBlock::Json(block) => Self::Json(block),
            NonToolContentBlock::Image(block) => Self::Image(block),
            NonToolContentBlock::Audio(block) => Self::Audio(block),
            NonToolContentBlock::File(block) => Self::File(block),
            NonToolContentBlock::Opaque(block) => Self::Opaque(block),
        }
    }
}

impl<'de> Deserialize<'de> for ToolResultContentItems {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            return deserializer.deserialize_seq(HumanToolResultContentVisitor);
        }
        deserializer.deserialize_seq(BinaryToolResultContentVisitor)
    }
}

struct HumanToolResultContentVisitor;

impl<'de> Visitor<'de> for HumanToolResultContentVisitor {
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
            let Some(raw) = sequence.next_element::<Box<serde_json::value::RawValue>>()? else {
                return Ok(ToolResultContentItems(content));
            };
            let probe: ContentKindProbe =
                serde_json::from_str(raw.get()).map_err(de::Error::custom)?;
            if matches!(probe.kind, ContentKind::ToolCall | ContentKind::ToolResult) {
                return Err(de::Error::custom(ContentError::NestedToolBlock));
            }
            let block = deserialize_human_content_block(raw.get()).map_err(de::Error::custom)?;
            content.push(block);
        }
        reject_trailing_content(&mut sequence)?;
        Ok(ToolResultContentItems(content))
    }
}

struct BinaryToolResultContentVisitor;

impl<'de> Visitor<'de> for BinaryToolResultContentVisitor {
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
            let Some(block) = sequence.next_element::<NonToolContentBlock>()? else {
                return Ok(ToolResultContentItems(content));
            };
            content.push(block.into());
        }
        reject_trailing_content(&mut sequence)?;
        Ok(ToolResultContentItems(content))
    }
}

struct StrictRawJson(RawJson);

impl<'de> Deserialize<'de> for StrictRawJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
        RawJson::parse(raw.get().as_bytes())
            .map(Self)
            .map_err(de::Error::custom)
    }
}

pub(super) struct StrictOpaquePayload(pub(super) OpaquePayload);

impl<'de> Deserialize<'de> for StrictOpaquePayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            encoding: OpaqueEncoding,
            #[serde(default)]
            data_hex: Option<BoundedString<{ TEXT_MAX_BYTES * 2 }>>,
            #[serde(default)]
            data: Option<StrictRawJson>,
        }

        let wire = Wire::deserialize(deserializer)?;
        match wire.encoding {
            OpaqueEncoding::Bytes => {
                if wire.data.is_some() {
                    return Err(de::Error::custom(
                        "bytes opaque payload must not contain data",
                    ));
                }
                let hex = wire
                    .data_hex
                    .ok_or_else(|| de::Error::missing_field("data_hex"))?;
                let bytes = hex_decode(&hex.into_inner()).map_err(de::Error::custom)?;
                OpaquePayload::bytes(bytes)
                    .map(Self)
                    .map_err(de::Error::custom)
            }
            OpaqueEncoding::Json => {
                if wire.data_hex.is_some() {
                    return Err(de::Error::custom(
                        "JSON opaque payload must not contain data_hex",
                    ));
                }
                let value = wire.data.ok_or_else(|| de::Error::missing_field("data"))?;
                Ok(Self(OpaquePayload::json(value.0)))
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BinaryContentBlock {
    Text(TextBlock),
    Json(JsonBlock),
    Image(MediaRef),
    Audio(MediaRef),
    File(MediaRef),
    ToolCall(ToolCallBlock),
    ToolResult(ToolResultBlock),
    Opaque(OpaqueBlock),
}

impl From<BinaryContentBlock> for ContentBlock {
    fn from(value: BinaryContentBlock) -> Self {
        match value {
            BinaryContentBlock::Text(block) => Self::Text(block),
            BinaryContentBlock::Json(block) => Self::Json(block),
            BinaryContentBlock::Image(block) => Self::Image(block),
            BinaryContentBlock::Audio(block) => Self::Audio(block),
            BinaryContentBlock::File(block) => Self::File(block),
            BinaryContentBlock::ToolCall(block) => Self::ToolCall(block),
            BinaryContentBlock::ToolResult(block) => Self::ToolResult(block),
            BinaryContentBlock::Opaque(block) => Self::Opaque(block),
        }
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if !deserializer.is_human_readable() {
            return BinaryContentBlock::deserialize(deserializer).map(Into::into);
        }

        let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
        deserialize_human_content_block(raw.get()).map_err(de::Error::custom)
    }
}

fn deserialize_human_content_block(input: &str) -> Result<ContentBlock, String> {
    let probe: ContentKindProbe = serde_json::from_str(input).map_err(|error| error.to_string())?;
    match probe.kind {
        ContentKind::Text => {
            let wire: TaggedTextBlock =
                serde_json::from_str(input).map_err(|error| error.to_string())?;
            let _ = wire.kind;
            TextBlock::try_new(wire.text.into_inner())
                .map(ContentBlock::Text)
                .map_err(|error| error.to_string())
        }
        ContentKind::Json => {
            let wire: TaggedJsonBlock =
                serde_json::from_str(input).map_err(|error| error.to_string())?;
            let _ = wire.kind;
            Ok(ContentBlock::Json(JsonBlock::new(wire.value.0)))
        }
        ContentKind::Image | ContentKind::Audio | ContentKind::File => {
            let wire: TaggedMediaRef =
                serde_json::from_str(input).map_err(|error| error.to_string())?;
            let media = MediaRef::new(wire.blob);
            Ok(match wire.kind {
                ContentKind::Image => ContentBlock::Image(media),
                ContentKind::Audio => ContentBlock::Audio(media),
                ContentKind::File => ContentBlock::File(media),
                _ => unreachable!("matched media kind"),
            })
        }
        ContentKind::ToolCall => {
            let wire: TaggedToolCallBlock =
                serde_json::from_str(input).map_err(|error| error.to_string())?;
            let _ = wire.kind;
            ToolCallBlock::try_new(
                wire.tool_call_id,
                wire.tool_name.into_inner(),
                wire.arguments.0,
            )
            .map(ContentBlock::ToolCall)
            .map_err(|error| error.to_string())
        }
        ContentKind::ToolResult => {
            let wire: TaggedToolResultBlock =
                serde_json::from_str(input).map_err(|error| error.to_string())?;
            let _ = wire.kind;
            ToolResultBlock::try_new(wire.tool_call_id, wire.content.into_inner(), wire.is_error)
                .map(ContentBlock::ToolResult)
                .map_err(|error| error.to_string())
        }
        ContentKind::Opaque => {
            let wire: TaggedOpaqueBlock =
                serde_json::from_str(input).map_err(|error| error.to_string())?;
            let _ = wire.kind;
            OpaqueBlock::try_new(wire.media_type.into_inner(), wire.payload.0)
                .map(ContentBlock::Opaque)
                .map_err(|error| error.to_string())
        }
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
