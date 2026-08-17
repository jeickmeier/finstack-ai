//! Provider-opaque payloads and blocks.

use bytes::Bytes;
use serde::de;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use super::BoundedString;
use super::content_block::StrictOpaquePayload;
use super::error::ContentError;
use super::text::{LABEL_MAX_BYTES, TEXT_MAX_BYTES, hex_encode, validated_label};
use crate::raw_json::RawJson;
use core::fmt;
use serde::de::{Deserializer, Visitor};
use std::sync::Arc;

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
pub(super) enum OpaqueEncoding {
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
