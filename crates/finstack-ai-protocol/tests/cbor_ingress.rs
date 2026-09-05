//! Canonical ingress bounds and typed container exhaustion.

use finstack_ai_protocol::{decode, decode_value};
#[test]
fn tuple_rejects_extra_elements() {
    assert!(
        decode::<(u8,)>(&[0x82, 1, 2]).is_err(),
        "extra array member was silently discarded"
    );
}
#[test]
fn value_enforces_envelope_limit() {
    let mut bytes = vec![0x83];
    for _ in 0..3 {
        bytes.extend_from_slice(&[0x5a, 0x00, 0x30, 0x00, 0x00]);
        bytes.resize(bytes.len() + 3 * 1024 * 1024, 0);
    }
    assert!(
        decode_value(&bytes).is_err(),
        "9 MiB value bypassed the 8 MiB envelope ceiling"
    );
}

#[test]
fn fixed_arrays_and_tuple_variants_require_exact_arity() {
    #[derive(Debug, serde::Deserialize, PartialEq)]
    enum Choice {
        Pair(u8, u8),
    }
    assert_eq!(decode::<[u8; 2]>(&[0x82, 1, 2]).unwrap(), [1, 2]);
    assert!(decode::<[u8; 2]>(&[0x83, 1, 2, 3]).is_err());
    assert!(decode::<[u8; 2]>(&[0x81, 1]).is_err());
    assert!(decode::<Choice>(&[0xa1, 0x64, b'P', b'a', b'i', b'r', 0x83, 1, 2, 3]).is_err());
    assert_eq!(
        decode::<Choice>(&[0xa1, 0x64, b'P', b'a', b'i', b'r', 0x82, 1, 2]).unwrap(),
        Choice::Pair(1, 2)
    );
}

#[test]
fn value_accepts_exact_envelope_limit() {
    use finstack_ai_protocol::{CANONICAL_ENVELOPE_MAX_BYTES, CanonicalValue, encode_value};
    let half = CANONICAL_ENVELOPE_MAX_BYTES / 2;
    let value = CanonicalValue::Array(vec![
        CanonicalValue::Bytes(vec![0; half - 5]),
        CanonicalValue::Bytes(vec![1; half - 6]),
    ]);
    let bytes = encode_value(&value).expect("exact limit encodes");
    assert_eq!(bytes.len(), CANONICAL_ENVELOPE_MAX_BYTES);
    assert_eq!(decode_value(&bytes).expect("exact limit decodes"), value);
}
