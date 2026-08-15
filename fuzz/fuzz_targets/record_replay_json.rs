#![no_main]

use finstack_ai_kernel::{CommittedBatch, Kernel, RunEvent};
use libfuzzer_sys::fuzz_target;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayInput {
    batches: Vec<CommittedBatch>,
}

fn replay(batches: &[CommittedBatch]) -> (Kernel, Vec<CommittedBatch>, Vec<RunEvent>) {
    let mut kernel = Kernel::default();
    let mut accepted = Vec::new();
    let mut events = Vec::new();
    for batch in batches {
        let before = kernel.state().clone();
        let hash_before = before.state_hash().expect("valid replay prefix hashes");
        match kernel.apply(batch, events.len() as u64) {
            Ok(derived) => {
                accepted.push(batch.clone());
                events.extend(derived.iter().cloned());
            }
            Err(_) => {
                assert_eq!(kernel.state(), &before, "rejected batch changed state");
                assert_eq!(
                    kernel.state().state_hash().expect("rejected state hashes"),
                    hash_before,
                    "rejected batch changed state hash"
                );
                break;
            }
        }
    }
    (kernel, accepted, events)
}

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<ReplayInput>(data) else {
        return;
    };
    let (full, accepted, full_events) = replay(&input.batches);
    let (replayed, replayed_accepted, replayed_events) = replay(&accepted);
    assert_eq!(
        accepted, replayed_accepted,
        "accepted replay prefix changed"
    );
    assert_eq!(full.state(), replayed.state(), "full replay state changed");
    assert_eq!(full_events, replayed_events, "derived event order changed");
    assert_eq!(
        full.state().state_hash().expect("full state hashes"),
        replayed
            .state()
            .state_hash()
            .expect("replayed state hashes"),
        "full replay hash changed"
    );
});
