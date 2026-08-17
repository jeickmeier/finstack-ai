//! Text/JSON blocks and shared label/hex helpers.

use core::fmt;
use std::sync::Arc;

use bytes::Bytes;
use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::RawJson;

use super::BoundedString;
use super::error::ContentError;
use serde::Deserializer;

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

pub(super) fn validated_label(value: &str, field: &'static str) -> Result<Arc<str>, ContentError> {
    if !label_is_valid(value) {
        return Err(ContentError::InvalidLabel { field });
    }
    Ok(Arc::<str>::from(value))
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}")).expect("string write");
    }
    out
}

pub(super) fn hex_decode(input: &str) -> Result<Bytes, ContentError> {
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
