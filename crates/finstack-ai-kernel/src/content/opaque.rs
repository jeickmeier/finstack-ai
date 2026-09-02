//! Provider-opaque payloads and blocks.

use bytes::Bytes;
use core::fmt;
use std::sync::Arc;

use serde::de::{self, Deserializer, IntoDeserializer, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use super::BoundedString;
use super::error::ContentError;
use super::text::{LABEL_MAX_BYTES, TEXT_MAX_BYTES, hex_decode, hex_encode, validated_label};
use crate::primitives::RawJson;

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
        let human_readable = serializer.is_human_readable();
        let mut state = serializer.serialize_struct("OpaquePayload", 2)?;
        match self {
            Self::Bytes(bytes) if human_readable => {
                state.serialize_field("encoding", "bytes")?;
                state.serialize_field("data_hex", &hex_encode(bytes))?;
            }
            Self::Bytes(bytes) => {
                state.serialize_field("encoding", "bytes")?;
                state.serialize_field("data", &BinaryByteRef(bytes))?;
            }
            Self::Json(value) => {
                state.serialize_field("encoding", "json")?;
                state.serialize_field("data", value)?;
            }
        }
        state.end()
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

enum OpaqueData {
    Bytes(Bytes),
    Json(RawJson),
}

impl<'de> Deserialize<'de> for OpaqueData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Internally tagged content uses ContentDeserializer, which reports
        // `is_human_readable() == true` even on canonical CBOR. Accept both
        // binary byte strings and human JSON values.
        deserializer.deserialize_any(OpaqueDataVisitor)
    }
}

struct OpaqueDataVisitor;

impl OpaqueDataVisitor {
    fn bounded_bytes<E>(value: &[u8]) -> Result<OpaqueData, E>
    where
        E: de::Error,
    {
        if value.len() > TEXT_MAX_BYTES {
            return Err(E::custom(ContentError::BytesTooLarge {
                len: value.len(),
                max: TEXT_MAX_BYTES,
            }));
        }
        Ok(OpaqueData::Bytes(Bytes::copy_from_slice(value)))
    }
}

impl<'de> Visitor<'de> for OpaqueDataVisitor {
    type Value = OpaqueData;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("opaque payload bytes or a JSON value")
    }

    fn visit_borrowed_bytes<E>(self, value: &'de [u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Self::bounded_bytes(value)
    }

    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Self::bounded_bytes(value)
    }

    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Self::bounded_bytes(&value)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawJson::deserialize(value.into_deserializer()).map(OpaqueData::Json)
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawJson::deserialize(value.into_deserializer()).map(OpaqueData::Json)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawJson::deserialize(value.into_deserializer()).map(OpaqueData::Json)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawJson::deserialize(value.into_deserializer()).map(OpaqueData::Json)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawJson::deserialize(value.into_deserializer()).map(OpaqueData::Json)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawJson::deserialize(value.into_deserializer()).map(OpaqueData::Json)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawJson::deserialize(().into_deserializer()).map(OpaqueData::Json)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }

    fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        RawJson::deserialize(de::value::SeqAccessDeserializer::new(seq)).map(OpaqueData::Json)
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        RawJson::deserialize(de::value::MapAccessDeserializer::new(map)).map(OpaqueData::Json)
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
            #[serde(default)]
            data_hex: Option<BoundedString<{ TEXT_MAX_BYTES * 2 }>>,
            #[serde(default)]
            data: Option<OpaqueData>,
        }

        let wire = Wire::deserialize(deserializer)?;
        match wire.encoding {
            OpaqueEncoding::Bytes => match (wire.data_hex, wire.data) {
                (Some(hex), None) => {
                    let bytes = hex_decode(&hex.into_inner()).map_err(de::Error::custom)?;
                    Self::bytes(bytes).map_err(de::Error::custom)
                }
                (None, Some(OpaqueData::Bytes(bytes))) => {
                    Self::bytes(bytes).map_err(de::Error::custom)
                }
                (Some(_), Some(_)) | (None, Some(OpaqueData::Json(_))) => Err(de::Error::custom(
                    "bytes opaque payload must not contain data",
                )),
                (None, None) => Err(de::Error::custom(
                    "bytes opaque payload requires data or data_hex",
                )),
            },
            OpaqueEncoding::Json => {
                if wire.data_hex.is_some() {
                    return Err(de::Error::custom(
                        "JSON opaque payload must not contain data_hex",
                    ));
                }
                match wire.data {
                    Some(OpaqueData::Json(value)) => Ok(Self::json(value)),
                    Some(OpaqueData::Bytes(bytes)) => RawJson::parse(bytes)
                        .map(Self::json)
                        .map_err(de::Error::custom),
                    None => Err(de::Error::missing_field("data")),
                }
            }
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
    /// # Arguments
    ///
    /// * `media_type` - Namespaced media-type label (non-empty, no NUL).
    /// * `payload` - [`OpaquePayload::Bytes`] or [`OpaquePayload::Json`].
    ///
    /// # Errors
    ///
    /// Returns [`ContentError`] when `media_type` is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{OpaqueBlock, OpaquePayload, RawJson};
    ///
    /// let block = OpaqueBlock::try_new(
    ///     "application/vnd.example+json",
    ///     OpaquePayload::json(RawJson::parse(r#"{"k":1}"#).expect("json")),
    /// )
    /// .expect("opaque");
    /// assert_eq!(block.media_type(), "application/vnd.example+json");
    /// ```
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
