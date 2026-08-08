//! Content blocks and blob references (TDD §7.1–§7.2).

use core::fmt;
use std::sync::Arc;

use bytes::Bytes;
use serde::de;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

use crate::digest::Digest;
use crate::ids::ToolCallId;
use crate::raw_json::{RawJson, raw_json_from_value};

/// V1 individual text/byte-string ceiling (4 MiB; TDD §6.5).
pub const TEXT_MAX_BYTES: usize = 4 * 1024 * 1024;
/// V1 content-array item ceiling (TDD §6.5).
pub const CONTENT_MAX_ITEMS: usize = 4_096;
/// V1 ceiling for media-type, blob-id, tool-name, and similar short labels.
pub const LABEL_MAX_BYTES: usize = 256;

/// Reference to externally stored media bytes.
///
/// The kernel never dereferences a blob. Large content is represented by this
/// reference rather than inline payload bytes.
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::BlobRef;
///
/// let blob = BlobRef::try_new(
///     "blob-1",
///     "image/png",
///     1_048_576,
///     None,
///     Some("diagram.png"),
/// )
/// .expect("blob");
/// assert_eq!(blob.length(), 1_048_576);
/// assert!(blob.name().is_some());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct BlobRef {
    id: Arc<str>,
    media_type: Arc<str>,
    length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<Arc<str>>,
}

impl BlobRef {
    /// Construct a validated blob reference.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when labels are empty, oversized, or contain NUL.
    pub fn try_new(
        id: impl AsRef<str>,
        media_type: impl AsRef<str>,
        length: u64,
        digest: Option<Digest>,
        name: Option<impl AsRef<str>>,
    ) -> Result<Self, ContentError> {
        let id = validated_label(id.as_ref(), "id")?;
        let media_type = validated_label(media_type.as_ref(), "media_type")?;
        let name = match name {
            Some(value) => Some(validated_label(value.as_ref(), "name")?),
            None => None,
        };
        Ok(Self {
            id,
            media_type,
            length,
            digest,
            name,
        })
    }

    /// Borrow the opaque blob identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Borrow the media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Declared content length in bytes.
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    /// Optional integrity digest over exact blob bytes (`blob-content` domain).
    #[must_use]
    pub const fn digest(&self) -> Option<&Digest> {
        self.digest.as_ref()
    }

    /// Optional display name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            id: String,
            media_type: String,
            length: u64,
            #[serde(default)]
            digest: Option<Digest>,
            #[serde(default)]
            name: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.id,
            wire.media_type,
            wire.length,
            wire.digest,
            wire.name,
        )
        .map_err(de::Error::custom)
    }
}

/// Media content referenced by [`BlobRef`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaRef {
    blob: BlobRef,
}

impl MediaRef {
    /// Wrap a blob reference as media content.
    #[must_use]
    pub fn new(blob: BlobRef) -> Self {
        Self { blob }
    }

    /// Borrow the blob reference.
    #[must_use]
    pub const fn blob(&self) -> &BlobRef {
        &self.blob
    }
}

/// UTF-8 text content block.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct TextBlock {
    text: Arc<str>,
}

impl TextBlock {
    /// Construct a text block under the v1 text ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError::TextTooLarge`] when `text` exceeds [`TEXT_MAX_BYTES`].
    pub fn try_new(text: impl AsRef<str>) -> Result<Self, ContentError> {
        let text = text.as_ref();
        if text.len() > TEXT_MAX_BYTES {
            return Err(ContentError::TextTooLarge {
                len: text.len(),
                max: TEXT_MAX_BYTES,
            });
        }
        Ok(Self {
            text: Arc::<str>::from(text),
        })
    }

    /// Borrow the text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'de> Deserialize<'de> for TextBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            text: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text).map_err(de::Error::custom)
    }
}

/// Structured JSON content block.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct JsonBlock {
    value: RawJson,
}

impl JsonBlock {
    /// Construct from validated JSON.
    #[must_use]
    pub const fn new(value: RawJson) -> Self {
        Self { value }
    }

    /// Borrow the JSON value.
    #[must_use]
    pub const fn value(&self) -> &RawJson {
        &self.value
    }
}

impl<'de> Deserialize<'de> for JsonBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            value: Value,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = raw_json_from_value(&wire.value).map_err(de::Error::custom)?;
        Ok(Self::new(value))
    }
}

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
        struct Wire {
            tool_call_id: ToolCallId,
            tool_name: String,
            arguments: Value,
        }
        let wire = Wire::deserialize(deserializer)?;
        let arguments = raw_json_from_value(&wire.arguments).map_err(de::Error::custom)?;
        Self::try_new(wire.tool_call_id, wire.tool_name, arguments).map_err(de::Error::custom)
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
        content: impl Into<Arc<[ContentBlock]>>,
        is_error: bool,
    ) -> Result<Self, ContentError> {
        let content = content.into();
        validate_content_items(&content)?;
        for block in content.iter() {
            if matches!(
                block,
                ContentBlock::ToolCall(_) | ContentBlock::ToolResult(_)
            ) {
                return Err(ContentError::NestedToolBlock);
            }
        }
        Ok(Self {
            tool_call_id,
            content,
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
        struct Wire {
            tool_call_id: ToolCallId,
            content: Vec<ContentBlock>,
            #[serde(default)]
            is_error: bool,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.tool_call_id, wire.content, wire.is_error).map_err(de::Error::custom)
    }
}

/// Provider-opaque payload bytes or JSON.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OpaquePayload {
    /// Exact opaque bytes.
    Bytes(Bytes),
    /// Opaque JSON value.
    Json(RawJson),
}

impl OpaquePayload {
    /// Construct a byte payload under the v1 byte-string ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError::BytesTooLarge`] when `bytes` exceeds [`TEXT_MAX_BYTES`].
    pub fn bytes(bytes: impl Into<Bytes>) -> Result<Self, ContentError> {
        let bytes = bytes.into();
        if bytes.len() > TEXT_MAX_BYTES {
            return Err(ContentError::BytesTooLarge {
                len: bytes.len(),
                max: TEXT_MAX_BYTES,
            });
        }
        Ok(Self::Bytes(bytes))
    }

    /// Construct a JSON payload.
    #[must_use]
    pub const fn json(value: RawJson) -> Self {
        Self::Json(value)
    }
}

impl Serialize for OpaquePayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Bytes(bytes) => {
                let mut state = serializer.serialize_struct("OpaquePayload", 2)?;
                state.serialize_field("encoding", "bytes")?;
                state.serialize_field("data_hex", &hex_encode(bytes))?;
                state.end()
            }
            Self::Json(value) => {
                let mut state = serializer.serialize_struct("OpaquePayload", 2)?;
                state.serialize_field("encoding", "json")?;
                state.serialize_field("data", value)?;
                state.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for OpaquePayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            encoding: String,
            #[serde(default)]
            data_hex: Option<String>,
            #[serde(default)]
            data: Option<Value>,
        }
        let wire = Wire::deserialize(deserializer)?;
        match wire.encoding.as_str() {
            "bytes" => {
                let hex = wire
                    .data_hex
                    .ok_or_else(|| de::Error::missing_field("data_hex"))?;
                let bytes = hex_decode(&hex).map_err(de::Error::custom)?;
                Self::bytes(bytes).map_err(de::Error::custom)
            }
            "json" => {
                let value = wire.data.ok_or_else(|| de::Error::missing_field("data"))?;
                let json = raw_json_from_value(&value).map_err(de::Error::custom)?;
                Ok(Self::json(json))
            }
            other => Err(de::Error::custom(format!(
                "unknown opaque encoding {other}"
            ))),
        }
    }
}

/// Provider-specific opaque content that must round-trip when required.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct OpaqueBlock {
    media_type: Arc<str>,
    payload: OpaquePayload,
}

impl OpaqueBlock {
    /// Construct an opaque block.
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when `media_type` is invalid.
    pub fn try_new(
        media_type: impl AsRef<str>,
        payload: OpaquePayload,
    ) -> Result<Self, ContentError> {
        let media_type = validated_label(media_type.as_ref(), "media_type")?;
        Ok(Self {
            media_type,
            payload,
        })
    }

    /// Namespaced media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Opaque payload.
    #[must_use]
    pub const fn payload(&self) -> &OpaquePayload {
        &self.payload
    }
}

impl<'de> Deserialize<'de> for OpaqueBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            media_type: String,
            payload: OpaquePayload,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.media_type, wire.payload).map_err(de::Error::custom)
    }
}

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

/// Content construction / validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContentError {
    /// Label was empty, oversized, or contained NUL.
    #[error("invalid {field}: empty, oversized, or contains NUL")]
    InvalidLabel {
        /// Field name.
        field: &'static str,
    },
    /// Text exceeded the v1 ceiling.
    #[error("text length {len} exceeds max {max}")]
    TextTooLarge {
        /// Observed UTF-8 byte length.
        len: usize,
        /// Ceiling.
        max: usize,
    },
    /// Opaque bytes exceeded the v1 ceiling.
    #[error("byte length {len} exceeds max {max}")]
    BytesTooLarge {
        /// Observed byte length.
        len: usize,
        /// Ceiling.
        max: usize,
    },
    /// Content array exceeded the v1 item ceiling.
    #[error("content item count {len} exceeds max {max}")]
    TooManyItems {
        /// Observed item count.
        len: usize,
        /// Ceiling.
        max: usize,
    },
    /// Nested tool-call/tool-result inside a tool-result payload.
    #[error("tool_result content cannot nest tool_call or tool_result blocks")]
    NestedToolBlock,
    /// Hex payload was malformed.
    #[error("invalid hex payload")]
    InvalidHex,
}

pub(crate) fn validate_content_items(content: &[ContentBlock]) -> Result<(), ContentError> {
    if content.len() > CONTENT_MAX_ITEMS {
        return Err(ContentError::TooManyItems {
            len: content.len(),
            max: CONTENT_MAX_ITEMS,
        });
    }
    Ok(())
}

fn validated_label(value: &str, field: &'static str) -> Result<Arc<str>, ContentError> {
    if value.is_empty() || value.len() > LABEL_MAX_BYTES || value.as_bytes().contains(&0) {
        return Err(ContentError::InvalidLabel { field });
    }
    Ok(Arc::<str>::from(value))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}")).expect("string write");
    }
    out
}

fn hex_decode(input: &str) -> Result<Bytes, ContentError> {
    if !input.len().is_multiple_of(2) || !input.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ContentError::InvalidHex);
    }
    let mut bytes = Vec::with_capacity(input.len() / 2);
    for chunk in input.as_bytes().chunks_exact(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes.push((hi << 4) | lo);
    }
    Ok(Bytes::from(bytes))
}

fn hex_nibble(byte: u8) -> Result<u8, ContentError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(ContentError::InvalidHex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ToolCallId;

    #[test]
    fn blob_ref_rejects_inline_semantics_and_invalid_labels() {
        let blob = BlobRef::try_new("b1", "image/png", 10, None, None::<&str>).expect("ok");
        let json = serde_json::to_string(&blob).expect("ser");
        assert!(!json.contains("payload"));
        assert!(!json.contains("bytes"));
        assert!(BlobRef::try_new("", "image/png", 1, None, None::<&str>).is_err());
        assert!(BlobRef::try_new("b1", "x".repeat(257), 1, None, None::<&str>).is_err());
    }

    #[test]
    fn large_declared_length_serializes_as_small_reference() {
        let blob = BlobRef::try_new("huge", "application/pdf", 50_000_000, None, Some("doc.pdf"))
            .expect("blob");
        let json = serde_json::to_string(&blob).expect("ser");
        assert!(json.len() < 200);
        assert!(json.contains("50000000"));
        let round: BlobRef = serde_json::from_str(&json).expect("de");
        assert_eq!(round, blob);
    }

    #[test]
    fn content_block_round_trip_and_unknown_kind() {
        let block = ContentBlock::Text(TextBlock::try_new("hi").expect("text"));
        let json = serde_json::to_string(&block).expect("ser");
        assert!(json.contains("\"kind\":\"text\""));
        let round: ContentBlock = serde_json::from_str(&json).expect("de");
        assert_eq!(round, block);
        let err = serde_json::from_str::<ContentBlock>(r#"{"kind":"reasoning","text":"x"}"#);
        assert!(err.is_err());
    }

    #[test]
    fn tool_result_rejects_nested_tool_blocks() {
        let call_id = ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
        let nested = ContentBlock::ToolCall(
            ToolCallBlock::try_new(call_id, "lookup", RawJson::parse("{}").expect("json"))
                .expect("call"),
        );
        let err = ToolResultBlock::try_new(call_id, vec![nested], false).expect_err("nested");
        assert!(matches!(err, ContentError::NestedToolBlock));
    }

    #[test]
    fn text_exact_and_one_over_ceiling() {
        let exact = "a".repeat(TEXT_MAX_BYTES);
        assert!(TextBlock::try_new(&exact).is_ok());
        let over = "a".repeat(TEXT_MAX_BYTES + 1);
        assert!(matches!(
            TextBlock::try_new(&over).expect_err("over"),
            ContentError::TextTooLarge { .. }
        ));
    }
}
