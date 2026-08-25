//! Kernel-on-WASM coverage probes executed by `mise run coverage-wasm`.

#![cfg(target_arch = "wasm32")]

use finstack_ai_kernel::{Digest, RawJson, RunId, Timestamp};
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn canonical_json_and_digest_execute_in_wasm() {
    let value = RawJson::parse(br#"{"b":1,"a":2}"#).expect("canonical JSON");
    assert_eq!(value.as_str(), r#"{"a":2,"b":1}"#);
    let digest = Digest::raw_json(value.as_bytes());
    assert_eq!(Digest::from_hex(&digest.to_hex()).expect("digest"), digest);
}

#[wasm_bindgen_test]
fn typed_ids_and_time_execute_in_wasm() {
    let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run id");
    assert_eq!(
        run.to_canonical_string(),
        "01234567-89ab-7cde-89ab-0123456789ab"
    );
    let timestamp = Timestamp::from_unix_ms(1_700_000_000_000).expect("timestamp");
    assert_eq!(timestamp.as_unix_ms(), 1_700_000_000_000);
}
