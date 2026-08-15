#![no_main]

use finstack_ai_wit::{
    MAX_RAW_JSON_BYTES, MAX_STRING_BYTES, parse_manifest, reject_before_allocation,
    reject_declared_len, validate_manifest,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if reject_before_allocation(data, MAX_RAW_JSON_BYTES, "manifest-json").is_err() {
        return;
    }
    if data.len() >= 8 {
        let declared = u64::from_be_bytes(data[..8].try_into().expect("declared"));
        let _ = reject_declared_len(declared, MAX_STRING_BYTES, "wit-guest");
    }
    let Ok(manifest) = parse_manifest(data) else {
        return;
    };
    assert!(
        !manifest.permissions.iter().any(|permission| {
            matches!(
                permission.as_str(),
                "wasi" | "ambient" | "preopen" | "inherit-host"
            )
        }),
        "manifest parser accepted an ambient WASI permission"
    );
    let _ = validate_manifest(&manifest);
});
