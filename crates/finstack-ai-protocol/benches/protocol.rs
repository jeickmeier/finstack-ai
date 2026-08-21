//! Canonical codec and snapshot microbenchmarks.

use std::collections::BTreeMap;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    APPEND_BATCH_MAX_RECORDS, Digest, EventId, KernelState, LaneId, Metadata,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordId, SessionCreated,
    SessionId, UNIX_EPOCH,
};
use finstack_ai_protocol::{
    CanonicalValue, commit_record, commit_records, decode_value, encode, encode_snapshot,
    encode_value, verify_envelope,
};

fn journal_draft(ordinal: u64) -> RecordDraft {
    let body = RecordBody::SessionCreated(SessionCreated::new(Metadata::empty()));
    let event_count = body
        .derived_event_count(RECORD_KIND_VERSION)
        .expect("event count");
    let events = (0..event_count)
        .map(|index| {
            let mut bytes = [0_u8; 16];
            let index = u64::try_from(index).expect("event index");
            bytes[8..].copy_from_slice(&(ordinal + index).to_be_bytes());
            EventId::from_bytes(bytes)
        })
        .collect();
    let mut record_bytes = [1_u8; 16];
    record_bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::from_bytes(record_bytes),
        SessionId::from_bytes([2; 16]),
        LaneId::from_bytes([3; 16]),
        None,
        UNIX_EPOCH,
        events,
        body,
    )
    .expect("draft")
}

fn protocol_benches(criterion: &mut Criterion) {
    criterion.bench_function("cbor/small", |bencher| {
        bencher.iter(|| encode(&(42_u64, "finstack")).expect("encode"));
    });

    let nested = (0..32).fold(CanonicalValue::Unsigned(1), |value, _| {
        CanonicalValue::Array(vec![value])
    });
    let nested_bytes = encode_value(&nested).expect("nested");
    criterion.bench_function("cbor/nested-32/decode", |bencher| {
        bencher.iter(|| decode_value(&nested_bytes).expect("decode"));
    });

    let map: BTreeMap<_, _> = (0..256)
        .map(|index| (format!("key-{index:03}"), vec![index; 16]))
        .collect();
    criterion.bench_function("cbor/map-256/encode", |bencher| {
        bencher.iter(|| encode(&map).expect("encode"));
    });

    let sequence = (0_u64..4096).collect::<Vec<_>>();
    let sequence_bytes = encode(&sequence).expect("sequence");
    criterion.bench_function("cbor/sequence-4096/decode", |bencher| {
        bencher.iter(|| finstack_ai_protocol::decode::<Vec<u64>>(&sequence_bytes).expect("decode"));
    });

    let state = KernelState::default();
    let head = Digest::raw_json(b"benchmark-head");
    criterion.bench_function("snapshot/default/encode", |bencher| {
        bencher.iter(|| encode_snapshot(&state, 0, head, None, None).expect("snapshot"));
    });

    let draft = journal_draft(1);
    let committed = commit_record(&draft, 1, None, None).expect("commit");
    criterion.bench_function("journal/record/commit", |bencher| {
        bencher.iter(|| commit_record(&draft, 1, None, None).expect("commit"));
    });
    criterion.bench_function("journal/record/verify", |bencher| {
        bencher.iter(|| verify_envelope(&committed).expect("verify"));
    });

    let batch = (0..APPEND_BATCH_MAX_RECORDS)
        .map(|index| journal_draft(u64::try_from(index).expect("batch index") + 1))
        .collect::<Vec<_>>();
    criterion.bench_function("journal/max-record-batch/commit", |bencher| {
        bencher.iter(|| commit_records(&batch, 1, None, None).expect("commit batch"));
    });
}

criterion_group!(benches, protocol_benches);
criterion_main!(benches);
