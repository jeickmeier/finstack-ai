//! Bounded strict `RawJson` and top-level-object `Metadata`.
//!
//! Validated values store RFC 8785 canonical UTF-8 bytes in shared [`Bytes`].
//! Equality and digests use those canonical bytes.

use core::fmt;
use std::collections::BTreeSet;

use bytes::Bytes;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::digest::Digest;

/// V1 `RawJson` source-span and canonical-JCS byte ceiling (1 MiB).
pub const RAW_JSON_MAX_BYTES: usize = 1_048_576;
/// V1 `RawJson` nesting depth ceiling.
pub const RAW_JSON_MAX_DEPTH: usize = 32;
/// V1 `Metadata` source-span and canonical-JCS byte ceiling (64 KiB).
pub const METADATA_MAX_BYTES: usize = 64 * 1024;
/// V1 `Metadata` nesting depth ceiling.
pub const METADATA_MAX_DEPTH: usize = 16;
/// V1 maximum top-level metadata members.
pub const METADATA_MAX_MEMBERS: usize = 64;
/// V1 maximum UTF-8 key length for metadata.
pub const METADATA_MAX_KEY_BYTES: usize = 128;

/// Validated UTF-8 JSON stored as shared immutable canonical bytes.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawJson(Bytes);

impl RawJson {
    /// Parse and validate JSON bytes under the `RawJson` ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`RawJsonError`] for oversized input, invalid JSON, duplicate keys,
    /// trailing data, excessive depth, non-finite numbers, or canonical overflow.
    pub fn parse(input: impl AsRef<[u8]>) -> Result<Self, RawJsonError> {
        Self::parse_with_limits(
            input.as_ref(),
            Limits {
                max_bytes: RAW_JSON_MAX_BYTES,
                max_depth: RAW_JSON_MAX_DEPTH,
                require_object: false,
                max_top_level_members: None,
                max_key_bytes: None,
            },
        )
    }

    /// Borrow the canonical UTF-8 JSON bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Borrow the canonical UTF-8 JSON text.
    ///
    /// # Panics
    ///
    /// Panics only if the stored canonical bytes are not UTF-8. Validated
    /// constructors never produce that state.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Canonical bytes are validated UTF-8.
        core::str::from_utf8(&self.0).expect("canonical JSON is UTF-8")
    }

    /// Shared byte buffer (clone-cheap).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    /// Domain-separated digest over the canonical bytes.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::raw_json(self.as_bytes())
    }

    /// Construct from already-validated canonical bytes without re-validation.
    ///
    /// Callers must ensure the bytes are RFC 8785 canonical UTF-8 JSON that
    /// already satisfied the applicable ceilings.
    #[must_use]
    pub(crate) fn from_canonical_unchecked(bytes: Bytes) -> Self {
        Self(bytes)
    }

    fn parse_with_limits(input: &[u8], limits: Limits) -> Result<Self, RawJsonError> {
        if input.len() > limits.max_bytes {
            return Err(RawJsonError::SourceTooLarge {
                len: input.len(),
                max: limits.max_bytes,
            });
        }
        let value = strict_parse_value(input, limits)?;
        if limits.require_object && !value.is_object() {
            return Err(RawJsonError::ExpectedObject);
        }
        if let (Some(max_members), Some(object)) = (limits.max_top_level_members, value.as_object())
        {
            if object.len() > max_members {
                return Err(RawJsonError::TooManyMembers {
                    count: object.len(),
                    max: max_members,
                });
            }
            if let Some(max_key_bytes) = limits.max_key_bytes {
                for key in object.keys() {
                    if key.len() > max_key_bytes {
                        return Err(RawJsonError::KeyTooLong {
                            len: key.len(),
                            max: max_key_bytes,
                        });
                    }
                }
            }
        }
        let canonical = serde_json_canonicalizer::to_vec(&value)
            .map_err(|error| RawJsonError::Canonicalize(error.to_string()))?;
        if canonical.len() > limits.max_bytes {
            return Err(RawJsonError::CanonicalTooLarge {
                len: canonical.len(),
                max: limits.max_bytes,
            });
        }
        Ok(Self(Bytes::from(canonical)))
    }
}

impl fmt::Debug for RawJson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawJson").field(&self.as_str()).finish()
    }
}

impl fmt::Display for RawJson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for RawJson {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            // Human/diagnostic JSON emits the structured value.
            let value: Value =
                serde_json::from_slice(self.as_bytes()).map_err(serde::ser::Error::custom)?;
            value.serialize(serializer)
        } else {
            // Durable CBOR carries the canonical JCS UTF-8 bytes as a byte string.
            serializer.serialize_bytes(self.as_bytes())
        }
    }
}

impl<'de> Deserialize<'de> for RawJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            // Preserve source text (including duplicate keys) before strict parse.
            let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
            Self::parse(raw.get().as_bytes()).map_err(serde::de::Error::custom)
        } else {
            let bytes = Vec::<u8>::deserialize(deserializer)?;
            Self::parse(bytes).map_err(serde::de::Error::custom)
        }
    }
}

/// Bounded top-level JSON object metadata.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Metadata(RawJson);

impl Metadata {
    /// Empty object metadata (`{}`).
    #[must_use]
    pub fn empty() -> Self {
        Self(RawJson::from_canonical_unchecked(Bytes::from_static(b"{}")))
    }

    /// Parse metadata requiring a top-level object and metadata ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`RawJsonError`] for the same failure classes as [`RawJson::parse`],
    /// plus non-object roots and metadata member/key limits.
    pub fn parse(input: impl AsRef<[u8]>) -> Result<Self, RawJsonError> {
        let raw = RawJson::parse_with_limits(
            input.as_ref(),
            Limits {
                max_bytes: METADATA_MAX_BYTES,
                max_depth: METADATA_MAX_DEPTH,
                require_object: true,
                max_top_level_members: Some(METADATA_MAX_MEMBERS),
                max_key_bytes: Some(METADATA_MAX_KEY_BYTES),
            },
        )?;
        Ok(Self(raw))
    }

    /// Borrow the underlying [`RawJson`].
    #[must_use]
    pub fn as_raw_json(&self) -> &RawJson {
        &self.0
    }

    /// Canonical UTF-8 JSON bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Canonical UTF-8 JSON text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Domain-separated digest over the canonical bytes.
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.0.digest()
    }
}

impl Default for Metadata {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Metadata").field(&self.as_str()).finish()
    }
}

impl fmt::Display for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Metadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // TDD §6.2: human JSON emits the object; canonical CBOR carries JCS bytes.
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Metadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
            Self::parse(raw.get().as_bytes()).map_err(serde::de::Error::custom)
        } else {
            let bytes = Vec::<u8>::deserialize(deserializer)?;
            Self::parse(bytes).map_err(serde::de::Error::custom)
        }
    }
}

/// Raw JSON / metadata validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RawJsonError {
    /// Source span exceeded the byte ceiling before parsing.
    #[error("JSON source span too large: {len} > {max}")]
    SourceTooLarge {
        /// Observed source length.
        len: usize,
        /// Configured ceiling.
        max: usize,
    },
    /// Canonical JCS bytes exceeded the byte ceiling.
    #[error("canonical JSON too large: {len} > {max}")]
    CanonicalTooLarge {
        /// Observed canonical length.
        len: usize,
        /// Configured ceiling.
        max: usize,
    },
    /// Nesting exceeded the depth ceiling.
    #[error("JSON nesting too deep: depth {depth} > {max}")]
    TooDeep {
        /// Observed depth.
        depth: usize,
        /// Configured ceiling.
        max: usize,
    },
    /// Object contained a duplicate key.
    #[error("duplicate object key: {key}")]
    DuplicateKey {
        /// Duplicate key text.
        key: String,
    },
    /// Trailing data followed a complete JSON value.
    #[error("trailing JSON data")]
    TrailingData,
    /// Metadata required a top-level object.
    #[error("metadata requires a top-level JSON object")]
    ExpectedObject,
    /// Metadata exceeded the top-level member ceiling.
    #[error("too many metadata members: {count} > {max}")]
    TooManyMembers {
        /// Observed member count.
        count: usize,
        /// Configured ceiling.
        max: usize,
    },
    /// Metadata key exceeded the UTF-8 byte ceiling.
    #[error("metadata key too long: {len} > {max}")]
    KeyTooLong {
        /// Observed key length.
        len: usize,
        /// Configured ceiling.
        max: usize,
    },
    /// Underlying JSON parse failure.
    #[error("invalid JSON: {0}")]
    Parse(String),
    /// Canonicalization failure.
    #[error("JSON canonicalization failed: {0}")]
    Canonicalize(String),
}

#[derive(Clone, Copy)]
struct Limits {
    max_bytes: usize,
    max_depth: usize,
    require_object: bool,
    max_top_level_members: Option<usize>,
    max_key_bytes: Option<usize>,
}

fn strict_parse_value(input: &[u8], limits: Limits) -> Result<Value, RawJsonError> {
    let mut de = serde_json::Deserializer::from_slice(input);
    let value = StrictValue {
        depth: 0,
        max_depth: limits.max_depth,
        max_key_bytes: limits.max_key_bytes,
    }
    .deserialize(&mut de)
    .map_err(|error| map_de_error(&error))?;
    de.end().map_err(|_| RawJsonError::TrailingData)?;
    Ok(value)
}

fn map_de_error(error: &serde_json::Error) -> RawJsonError {
    let text = error.to_string();
    if let Some(rest) = text.strip_prefix("duplicate object key: ") {
        let key = rest.split(" at line ").next().unwrap_or(rest).to_owned();
        return RawJsonError::DuplicateKey { key };
    }
    if let Some(rest) = text.strip_prefix("JSON nesting too deep: ") {
        // format: depth X > Y[ at line ...]
        let head = rest.split(" at line ").next().unwrap_or(rest);
        if let Some((depth, max)) = head.split_once(" > ")
            && let (Ok(depth), Ok(max)) = (depth.parse(), max.parse())
        {
            return RawJsonError::TooDeep { depth, max };
        }
    }
    if let Some(rest) = text.strip_prefix("metadata key too long: ") {
        let head = rest.split(" at line ").next().unwrap_or(rest);
        if let Some((len, max)) = head.split_once(" > ")
            && let (Ok(len), Ok(max)) = (len.parse(), max.parse())
        {
            return RawJsonError::KeyTooLong { len, max };
        }
    }
    RawJsonError::Parse(text)
}

struct StrictValue {
    depth: usize,
    max_depth: usize,
    max_key_bytes: Option<usize>,
}

impl<'de> DeserializeSeed<'de> for StrictValue {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictVisitor {
            depth: self.depth,
            max_depth: self.max_depth,
            max_key_bytes: self.max_key_bytes,
        })
    }
}

struct StrictVisitor {
    depth: usize,
    max_depth: usize,
    max_key_bytes: Option<usize>,
}

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::from(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::from(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if !value.is_finite() {
            return Err(E::custom("non-finite JSON number"));
        }
        Ok(Value::from(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let next_depth = self.depth + 1;
        if next_depth > self.max_depth {
            return Err(de::Error::custom(format!(
                "JSON nesting too deep: {next_depth} > {}",
                self.max_depth
            )));
        }
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(StrictValue {
            depth: next_depth,
            max_depth: self.max_depth,
            max_key_bytes: self.max_key_bytes,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let next_depth = self.depth + 1;
        if next_depth > self.max_depth {
            return Err(de::Error::custom(format!(
                "JSON nesting too deep: {next_depth} > {}",
                self.max_depth
            )));
        }
        let mut object = serde_json::Map::new();
        let mut seen = BTreeSet::<String>::new();
        while let Some(key) = map.next_key::<String>()? {
            if let Some(max_key_bytes) = self.max_key_bytes
                && key.len() > max_key_bytes
            {
                return Err(de::Error::custom(format!(
                    "metadata key too long: {} > {max_key_bytes}",
                    key.len()
                )));
            }
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate object key: {key}")));
            }
            let value = map.next_value_seed(StrictValue {
                depth: next_depth,
                max_depth: self.max_depth,
                max_key_bytes: self.max_key_bytes,
            })?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_canonicalizes_and_shares_bytes_on_clone() {
        let raw = RawJson::parse(r#"{"b":1,"a":2}"#).expect("parse");
        assert_eq!(raw.as_str(), r#"{"a":2,"b":1}"#);
        let clone = raw.clone();
        assert!(core::ptr::eq(
            raw.as_bytes().as_ptr(),
            clone.as_bytes().as_ptr()
        ));
        assert_eq!(raw.digest(), clone.digest());
    }

    #[test]
    fn rejects_duplicates_trailing_data_and_depth() {
        assert!(matches!(
            RawJson::parse(r#"{"a":1,"a":2}"#).expect_err("dup"),
            RawJsonError::DuplicateKey { .. }
        ));
        assert!(matches!(
            RawJson::parse("1 2").expect_err("trail"),
            RawJsonError::TrailingData
        ));
        let deep = "[".repeat(RAW_JSON_MAX_DEPTH + 1) + &"]".repeat(RAW_JSON_MAX_DEPTH + 1);
        assert!(matches!(
            RawJson::parse(deep).expect_err("depth"),
            RawJsonError::TooDeep { .. }
        ));
    }

    #[test]
    fn enforces_exact_and_one_over_source_ceiling() {
        let exact = format!(r#""{}""#, "a".repeat(RAW_JSON_MAX_BYTES - 2));
        assert_eq!(exact.len(), RAW_JSON_MAX_BYTES);
        RawJson::parse(&exact).expect("exact source");
        let over = format!(r#""{}""#, "a".repeat(RAW_JSON_MAX_BYTES - 1));
        assert!(over.len() > RAW_JSON_MAX_BYTES);
        assert!(matches!(
            RawJson::parse(&over).expect_err("over"),
            RawJsonError::SourceTooLarge { .. }
        ));
    }

    #[test]
    fn metadata_preserves_unknown_members_and_limits() {
        let meta = Metadata::parse(r#"{"z":true,"finstack.owner":1}"#).expect("meta");
        assert_eq!(meta.as_str(), r#"{"finstack.owner":1,"z":true}"#);
        assert!(Metadata::parse("[]").is_err());
        let too_many = (0..=METADATA_MAX_MEMBERS)
            .map(|i| format!(r#""k{i}":1"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(Metadata::parse(format!("{{{too_many}}}")).is_err());
        let long_key = format!(r#"{{"{}":1}}"#, "k".repeat(METADATA_MAX_KEY_BYTES + 1));
        assert!(Metadata::parse(long_key).is_err());
        assert_eq!(Metadata::empty().as_str(), "{}");
    }

    #[test]
    fn serde_human_path_rejects_duplicates_and_checks_source_span() {
        #[derive(Debug, Deserialize)]
        struct Wrap {
            #[allow(dead_code)]
            value: RawJson,
        }
        assert!(
            serde_json::from_str::<Wrap>(r#"{"value":{"a":1,"a":2}}"#)
                .expect_err("dup")
                .to_string()
                .contains("duplicate")
        );
        let over = format!(r#"{{"value":"{}"}}"#, "a".repeat(RAW_JSON_MAX_BYTES - 1));
        assert!(
            serde_json::from_str::<Wrap>(&over)
                .expect_err("source")
                .to_string()
                .contains("source span")
        );
        let meta = Metadata::parse(r#"{"a":1}"#).expect("meta");
        assert_eq!(serde_json::to_string(&meta).expect("ser"), r#"{"a":1}"#);
    }
}
