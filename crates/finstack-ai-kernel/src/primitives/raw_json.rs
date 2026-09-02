//! Bounded strict `RawJson` and top-level-object `Metadata`.
//!
//! Validated values store RFC 8785 canonical UTF-8 bytes in shared [`Bytes`].
//! Equality and digests use those canonical bytes.

use core::cell::Cell;
use core::fmt;

use bytes::Bytes;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::primitives::Digest;

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
    /// # Arguments
    ///
    /// * `input` - UTF-8 JSON bytes or text. Object members are stored in RFC 8785
    ///   canonical order; the original key order is not preserved.
    ///
    /// # Errors
    ///
    /// Returns [`RawJsonError`] for oversized input, invalid JSON, duplicate keys,
    /// trailing data, excessive depth, non-finite numbers, or canonical overflow.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::RawJson;
    ///
    /// let json = RawJson::parse(r#"{"b":1,"a":2}"#).expect("json");
    /// assert_eq!(json.as_str(), r#"{"a":2,"b":1}"#);
    /// ```
    pub fn parse(input: impl AsRef<[u8]>) -> Result<Self, RawJsonError> {
        Self::parse_with_limits(input.as_ref(), RAW_JSON_LIMITS)
    }

    /// Borrow the canonical UTF-8 JSON bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Borrow the canonical UTF-8 JSON text.
    ///
    /// Constructors store RFC 8785 canonical UTF-8. If that invariant is
    /// broken the empty string is returned instead of panicking.
    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0).unwrap_or_default()
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
        // Source span is the actual input. Streaming `charge_bytes` uses
        // reconstructed token sizes (`f64::to_string`, unescaped strings) and
        // would mis-classify compact scientific source as `SourceTooLarge`
        // before the canonical JCS ceiling can apply.
        let value = strict_parse_value(
            input,
            Limits {
                max_bytes: usize::MAX,
                ..limits
            },
        )?;
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
            // Human/diagnostic JSON emits the structured value. Stored bytes are
            // already canonical, so stream them straight through instead of
            // rebuilding a `serde_json::Value` to emit the same tokens.
            crate::primitives::CanonicalJson(self.as_bytes()).serialize(serializer)
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
        deserializer.deserialize_any(RawJsonVisitor)
    }
}

struct RawJsonVisitor;

impl<'de> Visitor<'de> for RawJsonVisitor {
    type Value = RawJson;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("canonical JSON bytes or a JSON value")
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        RawJson::parse(value).map_err(E::custom)
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        RawJson::parse(value).map_err(E::custom)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        scalar_json(&value)
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        scalar_json(&value)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        scalar_json(&value)
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        scalar_json(&value)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        scalar_json(&value)
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        scalar_json(&value)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        RawJson::parse(b"null").map_err(E::custom)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let used = Cell::new(0);
        let value = StrictValue::root(RAW_JSON_LIMITS, &used)
            .deserialize(de::value::SeqAccessDeserializer::new(seq))?;
        raw_json_from_value(&value, RAW_JSON_MAX_BYTES).map_err(de::Error::custom)
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let used = Cell::new(0);
        let value = StrictValue::root(RAW_JSON_LIMITS, &used)
            .deserialize(de::value::MapAccessDeserializer::new(map))?;
        raw_json_from_value(&value, RAW_JSON_MAX_BYTES).map_err(de::Error::custom)
    }
}

fn scalar_json<E: de::Error>(value: &impl Serialize) -> Result<RawJson, E> {
    let source = serde_json::to_string(value).map_err(E::custom)?;
    RawJson::parse(source).map_err(E::custom)
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
    /// # Arguments
    ///
    /// * `input` - UTF-8 JSON object bytes or text. Arrays and scalars are
    ///   rejected; member count and key length use the metadata ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`RawJsonError`] for the same failure classes as [`RawJson::parse`],
    /// plus non-object roots and metadata member/key limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::Metadata;
    ///
    /// let meta = Metadata::parse(r#"{"trace":"abc"}"#).expect("metadata");
    /// assert_eq!(meta.as_raw_json().as_str(), r#"{"trace":"abc"}"#);
    /// assert!(Metadata::parse("[]").is_err());
    /// ```
    pub fn parse(input: impl AsRef<[u8]>) -> Result<Self, RawJsonError> {
        RawJson::parse_with_limits(input.as_ref(), METADATA_LIMITS).map(Self)
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
        // contract section 6.2: human JSON emits the object; canonical CBOR carries JCS bytes.
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Metadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(MetadataVisitor)
    }
}

struct MetadataVisitor;

impl<'de> Visitor<'de> for MetadataVisitor {
    type Value = Metadata;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("canonical metadata bytes or a JSON object")
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        Metadata::parse(value).map_err(E::custom)
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        Metadata::parse(value).map_err(E::custom)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Metadata::parse(value).map_err(E::custom)
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Metadata::parse(value).map_err(E::custom)
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let used = Cell::new(0);
        let value = StrictValue::root(METADATA_LIMITS, &used)
            .deserialize(de::value::MapAccessDeserializer::new(map))?;
        if !value.is_object() {
            return Err(de::Error::custom(RawJsonError::ExpectedObject));
        }
        raw_json_from_value(&value, METADATA_MAX_BYTES)
            .map(Metadata)
            .map_err(de::Error::custom)
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

const RAW_JSON_LIMITS: Limits = Limits {
    max_bytes: RAW_JSON_MAX_BYTES,
    max_depth: RAW_JSON_MAX_DEPTH,
    require_object: false,
    max_top_level_members: None,
    max_key_bytes: None,
};

const METADATA_LIMITS: Limits = Limits {
    max_bytes: METADATA_MAX_BYTES,
    max_depth: METADATA_MAX_DEPTH,
    require_object: true,
    max_top_level_members: Some(METADATA_MAX_MEMBERS),
    max_key_bytes: Some(METADATA_MAX_KEY_BYTES),
};

fn raw_json_from_value(value: &Value, max_bytes: usize) -> Result<RawJson, RawJsonError> {
    let canonical = serde_json_canonicalizer::to_vec(&value)
        .map_err(|error| RawJsonError::Canonicalize(error.to_string()))?;
    if canonical.len() > max_bytes {
        return Err(RawJsonError::CanonicalTooLarge {
            len: canonical.len(),
            max: max_bytes,
        });
    }
    Ok(RawJson(Bytes::from(canonical)))
}

fn strict_parse_value(input: &[u8], limits: Limits) -> Result<Value, RawJsonError> {
    let mut de = serde_json::Deserializer::from_slice(input);
    let used = Cell::new(0);
    let value = StrictValue::root(limits, &used)
        .deserialize(&mut de)
        .map_err(|error| map_de_error(&error))?;
    de.end().map_err(|_| RawJsonError::TrailingData)?;
    Ok(value)
}

/// Recover the typed ceiling error that `StrictValue` reported through
/// `serde::de::Error::custom` (format: `<prefix><a> > <b>[ at line ...]`).
fn map_de_error(error: &serde_json::Error) -> RawJsonError {
    let text = error.to_string();
    let detail = |prefix: &str| {
        text.strip_prefix(prefix)
            .map(|rest| rest.split(" at line ").next().unwrap_or(rest))
    };
    let pair = |prefix: &str| -> Option<(usize, usize)> {
        let (left, right) = detail(prefix)?.split_once(" > ")?;
        Some((left.parse().ok()?, right.parse().ok()?))
    };
    if let Some(key) = detail("duplicate object key: ") {
        return RawJsonError::DuplicateKey {
            key: key.to_owned(),
        };
    }
    if let Some((depth, max)) = pair("JSON nesting too deep: ") {
        return RawJsonError::TooDeep { depth, max };
    }
    if let Some((len, max)) = pair("JSON source span too large: ") {
        return RawJsonError::SourceTooLarge { len, max };
    }
    if let Some((len, max)) = pair("metadata key too long: ") {
        return RawJsonError::KeyTooLong { len, max };
    }
    if let Some((count, max)) = pair("too many metadata members: ") {
        return RawJsonError::TooManyMembers { count, max };
    }
    RawJsonError::Parse(text)
}

/// Seed and visitor that materializes one JSON value while enforcing depth,
/// source-span, key-length, and top-level-member ceilings as it streams.
struct StrictValue<'a> {
    depth: usize,
    limits: Limits,
    used: &'a Cell<usize>,
}

impl<'a> StrictValue<'a> {
    const fn root(limits: Limits, used: &'a Cell<usize>) -> Self {
        Self {
            depth: 0,
            limits,
            used,
        }
    }

    fn charge<E: de::Error>(&self, add: usize) -> Result<(), E> {
        let next = self.used.get().saturating_add(add);
        let max = self.limits.max_bytes;
        if next > max {
            return Err(E::custom(format!(
                "JSON source span too large: {next} > {max}"
            )));
        }
        self.used.set(next);
        Ok(())
    }

    fn enter<E: de::Error>(&self) -> Result<Self, E> {
        let depth = self.depth + 1;
        if depth > self.limits.max_depth {
            return Err(E::custom(format!(
                "JSON nesting too deep: {depth} > {}",
                self.limits.max_depth
            )));
        }
        self.charge(2)?;
        Ok(Self {
            depth,
            limits: self.limits,
            used: self.used,
        })
    }

    fn child(&self) -> Self {
        Self {
            depth: self.depth,
            limits: self.limits,
            used: self.used,
        }
    }
}

impl<'de> DeserializeSeed<'de> for StrictValue<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for StrictValue<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.charge(if value { 4 } else { 5 })?;
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.charge(value.to_string().len())?;
        Ok(Value::from(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.charge(value.to_string().len())?;
        Ok(Value::from(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if !value.is_finite() {
            return Err(E::custom("non-finite JSON number"));
        }
        self.charge(value.to_string().len())?;
        Ok(Value::from(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.charge(value.len().saturating_add(2))?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.charge(value.len().saturating_add(2))?;
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.charge(4)?;
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.charge(4)?;
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let inner = self.enter()?;
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(inner.child())? {
            if !values.is_empty() {
                inner.charge(1)?;
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let inner = self.enter()?;
        let mut object = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if self.depth == 0
                && let Some(max_members) = self.limits.max_top_level_members
                && object.len() >= max_members
            {
                return Err(de::Error::custom(format!(
                    "too many metadata members: {} > {max_members}",
                    object.len().saturating_add(1)
                )));
            }
            if let Some(max_key_bytes) = self.limits.max_key_bytes
                && key.len() > max_key_bytes
            {
                return Err(de::Error::custom(format!(
                    "metadata key too long: {} > {max_key_bytes}",
                    key.len()
                )));
            }
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate object key: {key}")));
            }
            if !object.is_empty() {
                inner.charge(1)?;
            }
            inner.charge(key.len().saturating_add(3))?;
            let value = map.next_value_seed(inner.child())?;
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
        assert!(matches!(
            Metadata::parse(format!("{{{too_many}}}")).expect_err("members"),
            RawJsonError::TooManyMembers {
                count: 65,
                max: METADATA_MAX_MEMBERS
            }
        ));
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

    #[test]
    fn human_map_and_seq_reject_before_full_materialization() {
        let over = format!(r#"{{"k":"{}"}}"#, "a".repeat(RAW_JSON_MAX_BYTES));
        assert!(
            serde_json::from_str::<RawJson>(&over)
                .expect_err("raw map")
                .to_string()
                .contains("source span")
        );
        let over_seq = format!(r#"["{}"]"#, "a".repeat(RAW_JSON_MAX_BYTES));
        assert!(
            serde_json::from_str::<RawJson>(&over_seq)
                .expect_err("raw seq")
                .to_string()
                .contains("source span")
        );
        let deep = "[".repeat(RAW_JSON_MAX_DEPTH + 1) + &"]".repeat(RAW_JSON_MAX_DEPTH + 1);
        assert!(
            serde_json::from_str::<RawJson>(&deep)
                .expect_err("raw depth")
                .to_string()
                .contains("nesting")
        );
        let too_many = (0..=METADATA_MAX_MEMBERS)
            .map(|i| format!(r#""k{i}":1"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            serde_json::from_str::<Metadata>(&format!("{{{too_many}}}"))
                .expect_err("metadata members")
                .to_string()
                .contains("too many metadata members")
        );
    }

    #[test]
    fn metadata_deserialize_uses_metadata_ceilings() {
        let over = format!(r#"{{"k":"{}"}}"#, "a".repeat(METADATA_MAX_BYTES));
        assert!(
            serde_json::from_str::<Metadata>(&over)
                .expect_err("metadata map")
                .to_string()
                .contains("source span")
        );
        let deep = format!(
            "{}{}{}",
            r#"{"k":"#.repeat(METADATA_MAX_DEPTH + 1),
            "1",
            "}".repeat(METADATA_MAX_DEPTH + 1)
        );
        assert!(
            serde_json::from_str::<Metadata>(&deep)
                .expect_err("metadata depth")
                .to_string()
                .contains("nesting")
        );
    }
}
