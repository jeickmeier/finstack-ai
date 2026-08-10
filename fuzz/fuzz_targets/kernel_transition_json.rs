#![no_main]

use finstack_ai_kernel::{CommittedBatch, Kernel, KernelInput, TransitionEnv};
use libfuzzer_sys::fuzz_target;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransitionInput {
    batches: Vec<CommittedBatch>,
    environment: TransitionEnv,
    input: KernelInput,
}

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<TransitionInput>(data) else {
        return;
    };
    let mut kernel = Kernel::default();
    for batch in &input.batches {
        if kernel.apply(batch, 0).is_err() {
            return;
        }
    }
    let state_before = kernel.state().clone();
    let hash_before = state_before.state_hash().expect("decoded state hashes");
    let first = kernel.decide(&input.environment, input.input.clone());
    let second = kernel.decide(&input.environment, input.input);
    assert_eq!(first, second, "repeated decision changed");
    assert_eq!(kernel.state(), &state_before, "decision mutated state");
    assert_eq!(
        kernel
            .state()
            .state_hash()
            .expect("post-decision state hashes"),
        hash_before,
        "decision changed state hash"
    );

    let mut replayed = Kernel::default();
    for batch in &input.batches {
        replayed
            .apply(batch, 0)
            .expect("accepted prefix must replay");
    }
    assert_eq!(
        kernel.state(),
        replayed.state(),
        "prefix replay changed state"
    );
});
