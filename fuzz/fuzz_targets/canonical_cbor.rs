#![no_main]

use finstack_ai_protocol::{decode_value, encode_value};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = decode_value(data) {
        let encoded = encode_value(&value).expect("accepted canonical value must re-encode");
        assert_eq!(encoded, data, "accepted CBOR was not byte-canonical");
    }
});
