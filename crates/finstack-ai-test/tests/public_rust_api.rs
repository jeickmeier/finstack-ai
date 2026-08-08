//! PR-006 public-rust-api compatibility corpus.

use finstack_ai_kernel::Timestamp;
use finstack_ai_runtime::UuidV7Generator;
use finstack_ai_test::{FixedClock, PatternRandomSource, run_all_public_api_fixtures};

#[test]
fn public_rust_api_corpus_passes() {
    let count = run_all_public_api_fixtures().expect("public-rust-api fixtures");
    assert_eq!(
        count, 38,
        "expected the PR-006/PR-007 public-rust-api corpus size, found {count}"
    );
}

#[test]
fn deterministic_uuidv7_fakes_are_repeatable() {
    let clock = FixedClock::new(Timestamp::from_unix_ms(1_700_000_000_000).expect("ts"));
    let random =
        PatternRandomSource::new([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa]);
    let generator = UuidV7Generator::new(clock, random);
    let first = generator
        .generate::<finstack_ai_kernel::RunTag>()
        .expect("id")
        .to_canonical_string();
    let second = generator
        .generate::<finstack_ai_kernel::RunTag>()
        .expect("id")
        .to_canonical_string();
    assert_eq!(first, second);
    assert_eq!(first, "018bcfe5-6800-7122-b344-5566778899aa");
}
