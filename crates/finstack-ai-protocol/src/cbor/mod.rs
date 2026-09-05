//! Canonical-CBOR limits, encode/decode, and diagnostic JSON projection.

mod de;
mod diagnostic;
mod ser;
mod value;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ProtocolError;

pub(crate) use de::from_canonical;
pub use diagnostic::{from_diagnostic_json, to_diagnostic_json, to_diagnostic_jsonl};
pub use value::{CanonicalValue, decode_value, encode_value};

/// Canonical record-envelope byte ceiling (contract section 6.5).
pub const CANONICAL_ENVELOPE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Atomic append-batch byte ceiling, excluding backend overhead.
pub const APPEND_BATCH_MAX_BYTES: usize = 16 * 1024 * 1024;
/// Individual text or byte string ceiling.
pub const CANONICAL_STRING_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Definite-array item ceiling.
pub const CANONICAL_ARRAY_MAX_ITEMS: usize = 4096;
/// Definite-map entry ceiling.
pub const CANONICAL_MAP_MAX_ENTRIES: usize = 256;
/// Canonical-CBOR nesting-depth ceiling.
pub const CANONICAL_NESTING_DEPTH: usize = 32;

/// Encode `value` to canonical-CBOR bytes.
///
/// # Errors
///
/// Returns limit, profile, or typed-codec failures.
///
/// # Examples
///
/// ```
/// use finstack_ai_protocol::{decode, encode};
///
/// let bytes = encode(&42_u64).expect("encode");
/// assert_eq!(bytes, [0x18, 42]);
/// assert_eq!(decode::<u64>(&bytes).expect("decode"), 42);
/// ```
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let bytes = encode_value(&ser::to_canonical(value)?)?;
    if bytes.len() > CANONICAL_ENVELOPE_MAX_BYTES {
        return Err(ProtocolError::limit(
            "canonical_envelope",
            CANONICAL_ENVELOPE_MAX_BYTES,
        ));
    }
    Ok(bytes)
}

/// Decode one canonical-CBOR value into `T`.
///
/// Ceilings are enforced on declared lengths before allocation.
///
/// # Errors
///
/// Returns limit, profile, or typed-codec failures.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ProtocolError> {
    de::from_canonical(decode_value(bytes)?)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use finstack_ai_kernel::{CostAmount, Metadata};

    use super::{
        APPEND_BATCH_MAX_BYTES, CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_ENVELOPE_MAX_BYTES,
        CANONICAL_MAP_MAX_ENTRIES, CANONICAL_NESTING_DEPTH, CANONICAL_STRING_MAX_BYTES,
        CanonicalValue, decode, decode_value, encode, encode_value, from_diagnostic_json,
        to_diagnostic_json, to_diagnostic_jsonl,
    };
    use crate::error::ProtocolError;

    #[test]
    fn insertion_order_maps_encode_identically() {
        let mut first = BTreeMap::new();
        first.insert("b", 1_u64);
        first.insert("a", 2_u64);
        let mut second = BTreeMap::new();
        second.insert("a", 2_u64);
        second.insert("b", 1_u64);
        assert_eq!(
            encode(&first).expect("first"),
            encode(&second).expect("second")
        );
    }

    #[test]
    fn integer_boundaries_use_shortest_unsigned_form() {
        assert_eq!(encode(&0_u64).expect("0"), [0x00]);
        assert_eq!(encode(&((1_u64 << 53) - 1)).expect("2^53-1")[0], 0x1b);
        assert_eq!(encode(&(1_u64 << 53)).expect("2^53")[0], 0x1b);
        assert_eq!(encode(&u64::MAX).expect("u64::MAX")[0], 0x1b);
        assert_eq!(
            decode::<u64>(&encode(&u64::MAX).expect("enc")).expect("dec"),
            u64::MAX
        );
    }

    #[test]
    fn float_width_and_negative_zero() {
        let half = encode(&1.0_f64).expect("1.0");
        assert_eq!(half, [0xf9, 0x3c, 0x00]);
        let neg_zero = encode(&-0.0_f64).expect("-0");
        assert_eq!(neg_zero, [0xf9, 0x80, 0x00]);
        let decoded: f64 = decode(&neg_zero).expect("decode -0");
        assert_eq!(decoded.to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn every_finite_binary16_value_round_trips_at_binary16_width() {
        for bits in 0_u16..=u16::MAX {
            let half = half::f16::from_bits(bits);
            let [high, low] = bits.to_be_bytes();
            let bytes = [0xf9, high, low];
            if half.is_finite() {
                let decoded: f64 = decode(&bytes).expect("finite binary16");
                assert_eq!(decoded.to_bits(), half.to_f64().to_bits());
                assert_eq!(encode(&decoded).expect("re-encode"), bytes);
            } else {
                assert_eq!(
                    decode::<f64>(&bytes)
                        .expect_err("non-finite binary16")
                        .code(),
                    "non_finite_float"
                );
            }
        }
    }

    #[test]
    fn non_minimal_integer_length_and_float_encodings_are_rejected() {
        for bytes in [
            &[0x18, 0x17][..],
            &[0x19, 0x00, 0xff],
            &[0x1a, 0x00, 0x00, 0xff, 0xff],
            &[0x1b, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff],
            &[0x58, 0x00],
        ] {
            assert_eq!(
                decode_value(bytes).expect_err("non-minimal width").code(),
                "non_minimal_integer_or_length"
            );
        }

        assert_eq!(
            decode_value(&[0xfa, 0x3f, 0x80, 0x00, 0x00])
                .expect_err("f32 value is exactly binary16")
                .code(),
            "non_minimal_float"
        );
        assert_eq!(
            decode_value(&[0xfb, 0x3f, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
                .expect_err("f64 value is exactly binary16")
                .code(),
            "non_minimal_float"
        );
    }

    #[test]
    fn encode_value_enforces_nesting_and_envelope_limits() {
        let mut value = CanonicalValue::Null;
        for _ in 0..CANONICAL_NESTING_DEPTH {
            value = CanonicalValue::Array(vec![value]);
        }
        encode_value(&value).expect("exact depth");
        let over = CanonicalValue::Array(vec![value]);
        assert!(matches!(
            encode_value(&over),
            Err(ProtocolError::LimitExceeded {
                resource: "nesting_depth",
                limit: CANONICAL_NESTING_DEPTH
            })
        ));

        let oversized = CanonicalValue::Array(
            (0..CANONICAL_ARRAY_MAX_ITEMS)
                .map(|_| CanonicalValue::Bytes(vec![0; 4096]))
                .collect(),
        );
        assert!(matches!(
            encode_value(&oversized),
            Err(ProtocolError::LimitExceeded {
                resource: "canonical_envelope",
                limit: CANONICAL_ENVELOPE_MAX_BYTES
            })
        ));
    }

    #[test]
    fn non_finite_floats_are_rejected() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = encode(&value).expect_err("non-finite");
            assert_eq!(err.code(), "non_finite_float");
        }
    }

    #[test]
    fn bignum_tag_and_overflow_are_rejected() {
        assert_eq!(
            decode_value(&[0xc2, 0x41, 0x01]).expect_err("tag 2").code(),
            "bignum_tag"
        );
        assert_eq!(
            decode_value(&[0xc3, 0x41, 0x01]).expect_err("tag 3").code(),
            "bignum_tag"
        );
        let err = encode(&u128::MAX).expect_err("u128");
        assert_eq!(err.code(), "integer_overflow");
    }

    #[test]
    fn diagnostic_json_preserves_canonical_decimal_micros() {
        let amount = CostAmount::try_new("USD", u64::MAX, "price-v1").expect("cost");
        let json = to_diagnostic_json(&amount).expect("json");
        assert!(json.contains(&format!(r#""micros":"{}"#, u64::MAX)));
        let round: CostAmount = from_diagnostic_json(&json).expect("round-trip");
        assert_eq!(round, amount);
        let jsonl = to_diagnostic_jsonl(std::slice::from_ref(&amount)).expect("jsonl");
        assert_eq!(jsonl, format!("{json}\n"));
    }

    #[test]
    fn metadata_cbor_is_jcs_byte_string() {
        let metadata = Metadata::parse(br#"{"b":1,"a":2}"#).expect("metadata");
        let encoded = encode(&metadata).expect("encode");
        let CanonicalValue::Bytes(bytes) = decode_value(&encoded).expect("value") else {
            panic!("expected byte string");
        };
        assert_eq!(bytes, br#"{"a":2,"b":1}"#);
        let json = to_diagnostic_json(&metadata).expect("json");
        assert!(json.contains("\"a\":2"));
    }

    #[test]
    fn exact_and_one_over_collection_and_nesting_limits() {
        let exact_array = vec![0_u8; CANONICAL_ARRAY_MAX_ITEMS];
        encode(&exact_array).expect("exact array");
        let over_array = vec![0_u8; CANONICAL_ARRAY_MAX_ITEMS + 1];
        assert!(matches!(
            encode(&over_array),
            Err(ProtocolError::LimitExceeded {
                resource: "array_items",
                limit: CANONICAL_ARRAY_MAX_ITEMS
            })
        ));

        let exact_map: BTreeMap<_, _> = (0..CANONICAL_MAP_MAX_ENTRIES)
            .map(|index| (format!("k{index:03}"), 1_u64))
            .collect();
        encode(&exact_map).expect("exact map");
        let over_map: BTreeMap<_, _> = (0..=CANONICAL_MAP_MAX_ENTRIES)
            .map(|index| (format!("k{index:03}"), 1_u64))
            .collect();
        assert!(matches!(
            encode(&over_map),
            Err(ProtocolError::LimitExceeded {
                resource: "map_entries",
                limit: CANONICAL_MAP_MAX_ENTRIES
            })
        ));

        let exact_text = "x".repeat(CANONICAL_STRING_MAX_BYTES);
        encode(&exact_text).expect("exact text");
        let over_text = "x".repeat(CANONICAL_STRING_MAX_BYTES + 1);
        assert!(matches!(
            encode(&over_text),
            Err(ProtocolError::LimitExceeded {
                resource: "text_string",
                limit: CANONICAL_STRING_MAX_BYTES
            })
        ));

        let exact_nest = nested_arrays(CANONICAL_NESTING_DEPTH);
        decode_value(&exact_nest).expect("exact nesting");
        let over_nest = nested_arrays(CANONICAL_NESTING_DEPTH + 1);
        assert!(matches!(
            decode_value(&over_nest),
            Err(ProtocolError::LimitExceeded {
                resource: "nesting_depth",
                limit: CANONICAL_NESTING_DEPTH
            })
        ));
    }

    #[test]
    fn envelope_byte_limit_fails_closed_before_decode() {
        let exact = byte_string_of_total_len(CANONICAL_ENVELOPE_MAX_BYTES);
        assert_eq!(exact.len(), CANONICAL_ENVELOPE_MAX_BYTES);
        let exact_err = decode::<Vec<u8>>(&exact).expect_err("4 MiB string ceiling");
        assert!(
            matches!(
                exact_err,
                ProtocolError::LimitExceeded {
                    resource: "byte_string",
                    limit: CANONICAL_STRING_MAX_BYTES
                }
            ),
            "exact 8 MiB passes the envelope gate and fails a tighter ceiling: {exact_err:?}"
        );
        let over = vec![0xff; CANONICAL_ENVELOPE_MAX_BYTES + 1];
        assert!(matches!(
            decode::<u64>(&over),
            Err(ProtocolError::LimitExceeded {
                resource: "canonical_envelope",
                limit: CANONICAL_ENVELOPE_MAX_BYTES
            })
        ));
        assert_eq!(APPEND_BATCH_MAX_BYTES, 16 * 1024 * 1024);
    }

    fn to_ciborium(value: &CanonicalValue) -> ciborium::value::Value {
        match value {
            CanonicalValue::Null => ciborium::value::Value::Null,
            CanonicalValue::Bool(flag) => ciborium::value::Value::Bool(*flag),
            CanonicalValue::Unsigned(n) => ciborium::value::Value::Integer((*n).into()),
            CanonicalValue::Negative(n) => ciborium::value::Value::from(-1_i128 - i128::from(*n)),
            CanonicalValue::Bytes(bytes) => ciborium::value::Value::Bytes(bytes.clone()),
            CanonicalValue::Text(text) => ciborium::value::Value::Text(text.clone()),
            CanonicalValue::Array(items) => {
                ciborium::value::Value::Array(items.iter().map(to_ciborium).collect())
            }
            CanonicalValue::Map(entries) => ciborium::value::Value::Map(
                entries
                    .iter()
                    .map(|(key, value)| (to_ciborium(key), to_ciborium(value)))
                    .collect(),
            ),
            CanonicalValue::Float(bits) => ciborium::value::Value::Float(f64::from_bits(*bits)),
        }
    }

    #[test]
    fn ciborium_value_round_trips_structurally() {
        let value = CanonicalValue::Map(vec![
            (
                CanonicalValue::Text("b".into()),
                CanonicalValue::Unsigned(1),
            ),
            (
                CanonicalValue::Text("a".into()),
                CanonicalValue::Unsigned(2),
            ),
        ]);
        let encoded = encode_value(&value).expect("encode");
        let mut library = Vec::new();
        ciborium::ser::into_writer(&to_ciborium(&value), &mut library).expect("ciborium write");
        let decoded = decode_value(&encoded).expect("decode project");
        assert_eq!(
            decoded,
            decode_value(&encode_value(&value).expect("re")).expect("re")
        );
        assert!(!library.is_empty());
    }

    fn nested_arrays(depth: usize) -> Vec<u8> {
        let mut bytes = vec![0x81; depth];
        bytes.push(0x00);
        bytes
    }

    fn byte_string_of_total_len(total: usize) -> Vec<u8> {
        let header = 5;
        let payload = total.saturating_sub(header);
        let mut out = Vec::with_capacity(total);
        out.push(0x5a);
        out.extend_from_slice(
            &u32::try_from(payload)
                .expect("payload fits u32")
                .to_be_bytes(),
        );
        out.resize(total, 0);
        out
    }
}
