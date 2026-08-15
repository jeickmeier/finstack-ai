#![no_main]

use finstack_ai_kernel::Message;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(message) = serde_json::from_slice::<Message>(data) else {
        return;
    };
    message
        .validate_tool_associations(None)
        .expect("decoded message preserves tool associations");
    let encoded = serde_json::to_vec(&message).expect("validated message serializes");
    let decoded: Message = serde_json::from_slice(&encoded).expect("message round-trips");
    assert_eq!(message, decoded, "message round-trip changed semantics");
});
