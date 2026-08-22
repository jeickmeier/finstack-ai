//! Property tests for the untrusted-input decoders: frame headers and
//! canonical CBOR. Arbitrary bytes must never panic, successful decodes must
//! be canonical, and encode/decode must round-trip.

use finstack_ai_protocol::{
    CanonicalValue, FRAME_LENGTH_BYTES, PRE_AUTH_FRAME_MAX_BYTES, decode_frame, decode_frame_len,
    decode_value, encode_frame, encode_value,
};
use proptest::prelude::*;

/// Generates canonical values small enough that only `duplicate_map_key`
/// can make `encode_value` fail.
fn canonical_value() -> impl Strategy<Value = CanonicalValue> {
    let leaf = prop_oneof![
        Just(CanonicalValue::Null),
        any::<bool>().prop_map(CanonicalValue::Bool),
        any::<u64>().prop_map(CanonicalValue::Unsigned),
        any::<u64>().prop_map(CanonicalValue::Negative),
        prop::collection::vec(any::<u8>(), 0..64).prop_map(CanonicalValue::Bytes),
        ".{0,24}".prop_map(CanonicalValue::Text),
        any::<f64>()
            .prop_filter("finite", |float| float.is_finite())
            .prop_map(|float| CanonicalValue::Float(float.to_bits())),
    ];
    leaf.prop_recursive(4, 48, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(CanonicalValue::Array),
            prop::collection::vec((inner.clone(), inner), 0..4).prop_map(CanonicalValue::Map),
        ]
    })
}

proptest! {
    /// Encoding a generated value, decoding it, and re-encoding it must be a
    /// byte-exact fixed point. Unsorted map entries are sorted by the
    /// encoder, so equality is asserted on the canonical form, not the input.
    #[test]
    fn encode_decode_encode_is_byte_exact(value in canonical_value()) {
        match encode_value(&value) {
            Ok(bytes) => {
                let decoded = decode_value(&bytes).expect("canonical bytes decode");
                let reencoded = encode_value(&decoded).expect("canonical value re-encodes");
                prop_assert_eq!(&reencoded, &bytes);
                prop_assert_eq!(decode_value(&reencoded).expect("idempotent"), decoded);
            }
            Err(error) => prop_assert_eq!(error.code(), "duplicate_map_key"),
        }
    }

    /// The CBOR decoder must never panic on arbitrary bytes, and any accepted
    /// input must already be in canonical form (re-encoding reproduces it).
    #[test]
    fn decode_value_is_total_and_accepts_only_canonical_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        if let Ok(value) = decode_value(&bytes) {
            let reencoded = encode_value(&value).expect("accepted value re-encodes");
            prop_assert_eq!(reencoded, bytes);
        }
    }

    /// Frames round-trip below the ceiling.
    #[test]
    fn frame_round_trips(payload in prop::collection::vec(any::<u8>(), 0..1024)) {
        let frame = encode_frame(&payload, PRE_AUTH_FRAME_MAX_BYTES).expect("encode");
        let decoded = decode_frame(&frame, PRE_AUTH_FRAME_MAX_BYTES).expect("decode");
        prop_assert_eq!(decoded, &payload[..]);
    }

    /// The header decoder accepts a declared length exactly when it is within
    /// the ceiling, and never panics.
    #[test]
    fn decode_frame_len_enforces_the_ceiling(
        header in any::<[u8; FRAME_LENGTH_BYTES]>(),
        ceiling in 0_usize..=PRE_AUTH_FRAME_MAX_BYTES,
    ) {
        let declared = u32::from_be_bytes(header) as usize;
        match decode_frame_len(header, ceiling) {
            Ok(len) => {
                prop_assert_eq!(len, declared);
                prop_assert!(len <= ceiling);
            }
            Err(_) => prop_assert!(declared > ceiling),
        }
    }

    /// The whole-frame decoder must never panic, and must accept exactly the
    /// frames whose declared length matches the remaining bytes and ceiling.
    #[test]
    fn decode_frame_is_total(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        let ceiling = 16_usize;
        match decode_frame(&bytes, ceiling) {
            Ok(payload) => {
                let declared =
                    u32::from_be_bytes(bytes[..FRAME_LENGTH_BYTES].try_into().expect("header"))
                        as usize;
                prop_assert_eq!(payload.len(), declared);
                prop_assert!(declared <= ceiling);
                prop_assert_eq!(payload, &bytes[FRAME_LENGTH_BYTES..]);
            }
            Err(_) => prop_assert!(
                bytes.len() < FRAME_LENGTH_BYTES
                    || u32::from_be_bytes(
                        bytes[..FRAME_LENGTH_BYTES].try_into().expect("header"),
                    ) as usize > ceiling
                    || bytes.len() - FRAME_LENGTH_BYTES
                        != u32::from_be_bytes(
                            bytes[..FRAME_LENGTH_BYTES].try_into().expect("header"),
                        ) as usize
            ),
        }
    }
}
