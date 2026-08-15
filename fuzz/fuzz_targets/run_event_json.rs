#![no_main]

use finstack_ai_kernel::{RunEvent, RunEventClass};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(event) = serde_json::from_slice::<RunEvent>(data) else {
        return;
    };
    assert_eq!(event.kind(), event.body().kind(), "event kind/body drift");
    assert_eq!(
        event.class() == RunEventClass::DurableDerived,
        event.durable_sequence().is_some(),
        "event durability/sequence drift"
    );
    let encoded = serde_json::to_vec(&event).expect("validated event serializes");
    let decoded: RunEvent = serde_json::from_slice(&encoded).expect("event round-trips");
    assert_eq!(event, decoded, "event round-trip changed semantics");
});
