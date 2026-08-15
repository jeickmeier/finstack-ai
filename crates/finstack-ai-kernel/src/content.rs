//! Content blocks and blob references (TDD §7.1–§7.2).

use core::fmt;
use std::sync::Arc;

use bytes::Bytes;
use serde::de::{self, IgnoredAny, SeqAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::digest::Digest;
use crate::ids::ToolCallId;
use crate::raw_json::RawJson;

/// V1 individual text/byte-string ceiling (4 MiB; TDD §6.5).
pub const TEXT_MAX_BYTES: usize = 4 * 1024 * 1024;
/// V1 content-array item ceiling (TDD §6.5).
pub const CONTENT_MAX_ITEMS: usize = 4_096;
/// V1 ceiling for media-type, blob-id, tool-name, and similar short labels.
pub const LABEL_MAX_BYTES: usize = 256;

/// Whether a semantic label is non-empty, bounded, and NUL-free.
///
/// # Examples
///
/// ```
/// assert!(finstack_ai_kernel::label_is_valid("gpt-4"));
/// assert!(!finstack_ai_kernel::label_is_valid(""));
/// ```
#[must_use]
pub fn label_is_valid(value: &str) -> bool {
    !value.is_empty() && value.len() <= LABEL_MAX_BYTES && !value.as_bytes().contains(&0)
}

/// Decode one ASCII hex nibble.
///
/// # Examples
///
/// ```
/// assert_eq!(finstack_ai_kernel::hex_nibble(b'a'), Some(10));
/// assert_eq!(finstack_ai_kernel::hex_nibble(b'x'), None);
/// ```
#[must_use]
pub const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) struct BoundedString<const MAX: usize>(String);

impl<const MAX: usize> BoundedString<MAX> {
    pub(crate) fn into_inner(self) -> String {
        self.0
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for BoundedString<MAX> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoundedStringVisitor<const MAX: usize>;

        impl<const MAX: usize> Visitor<'_> for BoundedStringVisitor<MAX> {
            type Value = BoundedString<MAX>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a UTF-8 string no longer than {MAX} bytes")
            }

            fn visit_borrowed_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(value)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() > MAX {
                    return Err(E::custom(format_args!(
                        "string length {} exceeds max {MAX}",
                        value.len()
                    )));
                }
                Ok(BoundedString(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() > MAX {
                    return Err(E::custom(format_args!(
                        "string length {} exceeds max {MAX}",
                        value.len()
                    )));
                }
                Ok(BoundedString(value))
            }
        }

        // Internally tagged content uses serde's ContentDeserializer, which
        // reports `is_human_readable() == true` even on canonical CBOR. Accept
        // a decoded string from either JSON or CBOR.
        deserializer.deserialize_any(BoundedStringVisitor::<MAX>)
    }
}

pub(crate) struct ContentItems(Vec<ContentBlock>);

impl ContentItems {
    pub(crate) fn into_inner(self) -> Vec<ContentBlock> {
        self.0
    }
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

impl<'de> Deserialize<'de> for ContentItems {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ContentItemsVisitor;

        impl<'de> Visitor<'de> for ContentItemsVisitor {
            type Value = ContentItems;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    formatter,
                    "an array containing at most {CONTENT_MAX_ITEMS} content blocks"
                )
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                reject_oversized_content_hint::<A::Error>(sequence.size_hint())?;

                let capacity = sequence.size_hint().unwrap_or(0).min(CONTENT_MAX_ITEMS);
                let mut content = Vec::with_capacity(capacity);
                while content.len() < CONTENT_MAX_ITEMS {
                    let Some(block) = sequence.next_element()? else {
                        return Ok(ContentItems(content));
                    };
                    content.push(block);
                }
                reject_trailing_content(&mut sequence)?;
                Ok(ContentItems(content))
            }
        }

        deserializer.deserialize_seq(ContentItemsVisitor)
    }
}

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
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: BoundedString<LABEL_MAX_BYTES>,
            media_type: BoundedString<LABEL_MAX_BYTES>,
            length: u64,
            #[serde(default)]
            digest: Option<Digest>,
            #[serde(default)]
            name: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.id.into_inner(),
            wire.media_type.into_inner(),
            wire.length,
            wire.digest,
            wire.name.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}

/// Media content referenced by [`BlobRef`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        #[serde(deny_unknown_fields)]
        struct Wire {
            text: BoundedString<TEXT_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text.into_inner()).map_err(de::Error::custom)
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
        #[serde(deny_unknown_fields)]
        struct BinaryWire {
            value: RawJson,
        }

        let wire = BinaryWire::deserialize(deserializer)?;
        Ok(Self::new(wire.value))
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
        if !serializer.is_human_readable() {
            let mut state = serializer.serialize_struct("OpaquePayload", 2)?;
            match self {
                Self::Bytes(bytes) => {
                    state.serialize_field("encoding", "bytes")?;
                    state.serialize_field("data", &BinaryByteRef(bytes))?;
                }
                Self::Json(value) => {
                    state.serialize_field("encoding", "json")?;
                    state.serialize_field("data", value)?;
                }
            }
            return state.end();
        }

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

struct BinaryByteRef<'a>(&'a [u8]);

impl Serialize for BinaryByteRef<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(self.0)
    }
}

struct BinaryBytes(Bytes);

impl<'de> Deserialize<'de> for BinaryBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BinaryBytesVisitor;

        impl<'de> Visitor<'de> for BinaryBytesVisitor {
            type Value = BinaryBytes;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a byte string")
            }

            fn visit_borrowed_bytes<E>(self, value: &'de [u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_bytes(value)
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() > TEXT_MAX_BYTES {
                    return Err(E::custom(ContentError::BytesTooLarge {
                        len: value.len(),
                        max: TEXT_MAX_BYTES,
                    }));
                }
                Ok(BinaryBytes(Bytes::copy_from_slice(value)))
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() > TEXT_MAX_BYTES {
                    return Err(E::custom(ContentError::BytesTooLarge {
                        len: value.len(),
                        max: TEXT_MAX_BYTES,
                    }));
                }
                Ok(BinaryBytes(Bytes::from(value)))
            }
        }

        deserializer.deserialize_byte_buf(BinaryBytesVisitor)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum OpaqueEncoding {
    Bytes,
    Json,
}

impl<'de> Deserialize<'de> for OpaquePayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            encoding: OpaqueEncoding,
            data: BinaryBytes,
        }

        if deserializer.is_human_readable() {
            return StrictOpaquePayload::deserialize(deserializer).map(|payload| payload.0);
        }

        let wire = Wire::deserialize(deserializer)?;
        match wire.encoding {
            OpaqueEncoding::Bytes => Self::bytes(wire.data.0).map_err(de::Error::custom),
            OpaqueEncoding::Json => RawJson::parse(wire.data.0)
                .map(Self::json)
                .map_err(de::Error::custom),
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
        #[serde(deny_unknown_fields)]
        struct Wire {
            media_type: BoundedString<LABEL_MAX_BYTES>,
            payload: OpaquePayload,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.media_type.into_inner(), wire.payload).map_err(de::Error::custom)
    }
}

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

struct ToolResultContentItems(Vec<ContentBlock>);

impl ToolResultContentItems {
    fn into_inner(self) -> Vec<ContentBlock> {
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

struct StrictOpaquePayload(OpaquePayload);

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
    if !label_is_valid(value) {
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
        let hi = hex_nibble(chunk[0]).ok_or(ContentError::InvalidHex)?;
        let lo = hex_nibble(chunk[1]).ok_or(ContentError::InvalidHex)?;
        bytes.push((hi << 4) | lo);
    }
    Ok(Bytes::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ToolCallId;
    use serde::de::value::SeqDeserializer;

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
    fn content_blocks_reject_unknown_and_inapplicable_members() {
        let inline_media = serde_json::from_str::<ContentBlock>(
            r#"{
                "kind":"image",
                "blob":{"id":"b1","media_type":"image/png","length":1},
                "data_hex":"00"
            }"#,
        );
        assert!(inline_media.is_err());

        let extra_text =
            serde_json::from_str::<ContentBlock>(r#"{"kind":"text","text":"hi","extra":true}"#);
        assert!(extra_text.is_err());

        let conflicting_opaque = serde_json::from_str::<ContentBlock>(
            r#"{
                "kind":"opaque",
                "media_type":"application/vnd.example",
                "payload":{
                    "encoding":"bytes",
                    "data_hex":"00",
                    "data":{"also":"present"}
                }
            }"#,
        );
        assert!(conflicting_opaque.is_err());
    }

    #[test]
    fn nested_raw_json_preserves_strict_source_validation() {
        let duplicate = serde_json::from_str::<ContentBlock>(
            r#"{
                "kind":"tool_call",
                "tool_call_id":"01234567-89ab-7cde-89ab-0123456789ab",
                "tool_name":"lookup",
                "arguments":{"q":1,"q":2}
            }"#,
        );
        assert!(duplicate.is_err());

        let escaped = r"\u0061".repeat(crate::raw_json::RAW_JSON_MAX_BYTES / 6 + 1);
        let oversized_source = format!(
            r#"{{
                "kind":"json",
                "value":"{escaped}"
            }}"#
        );
        let oversized = serde_json::from_str::<ContentBlock>(&oversized_source);
        assert!(oversized.is_err());
    }

    #[test]
    fn content_block_accepts_owned_and_reader_json_inputs() {
        let value = serde_json::json!({"kind": "text", "text": "owned"});
        let owned: ContentBlock = serde_json::from_value(value).expect("owned value");
        assert_eq!(owned.kind_name(), "text");

        let input = br#"{"kind":"text","text":"reader"}"#;
        let reader: ContentBlock = serde_json::from_reader(input.as_slice()).expect("reader input");
        assert_eq!(reader.kind_name(), "text");
    }

    #[test]
    fn nested_tool_kind_is_rejected_before_its_payload_is_decoded() {
        let error = serde_json::from_str::<ContentBlock>(
            r#"{
                "kind":"tool_result",
                "tool_call_id":"01234567-89ab-7cde-89ab-0123456789ab",
                "content":[{
                    "kind":"tool_result",
                    "tool_call_id":"01234567-89ab-7cde-89ab-0123456789cd",
                    "content":[{"kind":"reasoning","text":"must not be reached"}]
                }]
            }"#,
        )
        .expect_err("nested tool result");
        assert!(error.to_string().contains("cannot nest"));
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

    #[test]
    fn content_item_size_hint_rejects_before_reading_elements() {
        let items = core::iter::repeat_with(|| -> serde_json::Value {
            panic!("oversized sequence should be rejected before reading an element")
        })
        .take(CONTENT_MAX_ITEMS + 1);
        let deserializer = SeqDeserializer::<_, serde_json::Error>::new(items);
        let Err(error) = ContentItems::deserialize(deserializer) else {
            panic!("oversized sequence unexpectedly succeeded");
        };
        assert!(error.to_string().contains("content item count"));
    }

    #[test]
    fn bounded_string_checks_escaped_length_before_decoding() {
        let exact: BoundedString<4> =
            serde_json::from_str(r#""\u0061\u0061\u0061\u0061""#).expect("exact escapes");
        assert_eq!(exact.into_inner(), "aaaa");

        let emoji: BoundedString<4> =
            serde_json::from_str(r#""\ud83d\ude00""#).expect("surrogate pair");
        assert_eq!(emoji.into_inner(), "😀");

        let over = serde_json::from_str::<BoundedString<4>>(r#""\u0061\u0061\u0061\u0061\u0061""#);
        assert!(over.is_err());
    }
}
