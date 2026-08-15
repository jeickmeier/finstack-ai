#![no_main]

use finstack_ai_kernel::{CommittedBatch, Kernel, RecordEnvelope};
use finstack_ai_protocol::{decode_snapshot, verify_chain};
use libfuzzer_sys::fuzz_target;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryInput {
    envelopes: Vec<RecordEnvelope>,
}

fn apply_verified(envelopes: &[RecordEnvelope]) {
    match verify_chain(envelopes) {
        Ok(_) => {}
        Err(_) => return,
    }
    let Ok(batch) = CommittedBatch::try_new(
        finstack_ai_kernel::Id::from_bytes([
            0, 0, 0, 0, 0, 0, 0x70, 0, 0x80, 0, 0, 0, 0, 0, 0, 1,
        ]),
        envelopes.first().map_or(1, RecordEnvelope::sequence),
        envelopes.last().map_or(0, RecordEnvelope::sequence),
        envelopes.to_vec(),
    ) else {
        return;
    };
    let mut kernel = Kernel::default();
    let before = kernel.state().clone();
    if kernel.apply(&batch, 0).is_err() {
        assert_eq!(
            kernel.state(),
            &before,
            "rejected recovery prefix changed state"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = serde_json::from_slice::<RecoveryInput>(data) {
        apply_verified(&input.envelopes);
        return;
    }
    if let Ok(batches) = serde_json::from_slice::<Vec<CommittedBatch>>(data) {
        let mut kernel = Kernel::default();
        for batch in &batches {
            if verify_chain(batch.records.as_ref()).is_err() {
                return;
            }
            let before = kernel.state().clone();
            if kernel.apply(batch, 0).is_err() {
                assert_eq!(
                    kernel.state(),
                    &before,
                    "corrupt recovery batch changed state"
                );
                return;
            }
        }
        return;
    }
    if decode_snapshot(data).is_ok() {
        let decoded = decode_snapshot(data).expect("snapshot decoded twice");
        let restored = Kernel::try_restore(decoded.state).expect("validated snapshot restores");
        assert_eq!(
            restored.state().state_hash().expect("restored hashes"),
            decoded.state_hash,
            "snapshot restore accepted a hash mismatch"
        );
    }
});
