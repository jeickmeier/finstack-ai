//! Streaming JSON transcoding for stored canonical bytes.
//!
//! [`RawJson`](crate::RawJson) and [`Metadata`](crate::Metadata) already hold
//! RFC 8785 canonical UTF-8. Emitting them through a `serde_json::Value` would
//! parse the bytes into a DOM and rebuild it just to produce the same tokens.
//! [`CanonicalJson`] instead drives a `serde_json` deserializer straight into
//! the target serializer, so no intermediate tree is allocated.
//!
//! The token stream is identical to the DOM path, which keeps every canonical
//! projection and frozen digest byte-for-byte unchanged.

use core::cell::RefCell;
use core::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{self, Serialize, SerializeMap, SerializeSeq, Serializer};

/// Serializes stored canonical JSON bytes without materializing a DOM.
pub(crate) struct CanonicalJson<'a>(pub(crate) &'a [u8]);

impl Serialize for CanonicalJson<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(self.0);
        deserializer
            .deserialize_any(TranscodeVisitor(serializer))
            .map_err(ser::Error::custom)
    }
}

/// Adapter serializing whatever a deserializer yields, exactly once.
///
/// `Serialize::serialize` takes `&self` but a `Deserializer` is consumed by
/// value, so the deserializer is parked in a `RefCell` and taken on first use.
struct TranscodeOnce<D>(RefCell<Option<D>>);

impl<D> TranscodeOnce<D> {
    fn new(deserializer: D) -> Self {
        Self(RefCell::new(Some(deserializer)))
    }
}

impl<'de, D> Serialize for TranscodeOnce<D>
where
    D: Deserializer<'de>,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let deserializer = self
            .0
            .borrow_mut()
            .take()
            .ok_or_else(|| ser::Error::custom("transcoded value serialized more than once"))?;
        deserializer
            .deserialize_any(TranscodeVisitor(serializer))
            .map_err(ser::Error::custom)
    }
}

/// Forwards each deserialized token to the wrapped serializer.
struct TranscodeVisitor<S>(S);

fn forward<E, T>(result: Result<T, impl fmt::Display>) -> Result<T, E>
where
    E: de::Error,
{
    result.map_err(E::custom)
}

impl<'de, S> Visitor<'de> for TranscodeVisitor<S>
where
    S: Serializer,
{
    type Value = S::Ok;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        forward(self.0.serialize_bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        forward(self.0.serialize_i64(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        forward(self.0.serialize_u64(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        forward(self.0.serialize_f64(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        forward(self.0.serialize_str(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        forward(self.0.serialize_unit())
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        forward(self.0.serialize_unit())
    }

    fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut sequence = forward(self.0.serialize_seq(None))?;
        while access
            .next_element_seed(ElementSeed(&mut sequence))?
            .is_some()
        {}
        forward(sequence.end())
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut map = forward(self.0.serialize_map(None))?;
        while access.next_key_seed(KeySeed(&mut map))?.is_some() {
            access.next_value_seed(ValueSeed(&mut map))?;
        }
        forward(map.end())
    }
}

struct ElementSeed<'a, S>(&'a mut S);

impl<'de, S> DeserializeSeed<'de> for ElementSeed<'_, S>
where
    S: SerializeSeq,
{
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        forward(self.0.serialize_element(&TranscodeOnce::new(deserializer)))
    }
}

struct KeySeed<'a, S>(&'a mut S);

impl<'de, S> DeserializeSeed<'de> for KeySeed<'_, S>
where
    S: SerializeMap,
{
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        forward(self.0.serialize_key(&TranscodeOnce::new(deserializer)))
    }
}

struct ValueSeed<'a, S>(&'a mut S);

impl<'de, S> DeserializeSeed<'de> for ValueSeed<'_, S>
where
    S: SerializeMap,
{
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        forward(self.0.serialize_value(&TranscodeOnce::new(deserializer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every canonical shape must transcode to the same bytes the DOM path
    /// produced, or frozen digests would move.
    #[track_caller]
    fn assert_matches_dom(canonical: &str) {
        let via_dom = serde_json_canonicalizer::to_vec(
            &serde_json::from_str::<serde_json::Value>(canonical).expect("parse"),
        )
        .expect("dom canonicalize");
        let via_transcode =
            serde_json_canonicalizer::to_vec(&CanonicalJson(canonical.as_bytes())).expect("stream");
        assert_eq!(
            String::from_utf8(via_transcode).expect("utf8"),
            String::from_utf8(via_dom).expect("utf8"),
            "transcoded output diverged for {canonical}"
        );
    }

    #[test]
    fn transcoding_matches_dom_serialization_for_every_json_shape() {
        for canonical in [
            "{}",
            "[]",
            "null",
            "true",
            "false",
            "0",
            "-1",
            "1.5",
            "1e+30",
            "9007199254740993",
            "-9007199254740993",
            r#""""#,
            r#""text""#,
            r#""quote\" backslash\\ newline\n tab\t""#,
            r#""unicode é 中""#,
            r#"{"a":1,"b":[1,2,3],"c":{"d":null}}"#,
            r#"[{"a":[[]]},[{}],[[[1]]]]"#,
            r#"{"nested":{"deep":{"deeper":{"value":[true,false,null]}}}}"#,
        ] {
            assert_matches_dom(canonical);
        }
    }

    #[test]
    fn transcoding_preserves_stored_key_order() {
        // Stored bytes are already JCS-sorted; transcoding must not reorder or
        // drop keys even though the formatter re-sorts what it receives.
        let canonical = r#"{"a":1,"b":2,"z":3}"#;
        assert_matches_dom(canonical);
        let out = serde_json_canonicalizer::to_vec(&CanonicalJson(canonical.as_bytes()))
            .expect("canonicalize");
        assert_eq!(String::from_utf8(out).expect("utf8"), canonical);
    }

    #[test]
    fn transcoding_into_plain_serde_json_matches_the_source() {
        let canonical = r#"{"a":[1,2],"b":"text"}"#;
        let out = serde_json::to_string(&CanonicalJson(canonical.as_bytes())).expect("serialize");
        assert_eq!(out, canonical);
    }
}
