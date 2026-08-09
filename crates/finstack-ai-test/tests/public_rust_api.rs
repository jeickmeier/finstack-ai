//! Public-rust-api compatibility corpus (PR-006–PR-012).

use std::collections::BTreeSet;

use finstack_ai_kernel::Timestamp;
use finstack_ai_runtime::UuidV7Generator;
use finstack_ai_test::{
    FixedClock, PatternRandomSource, discover_public_api_fixtures, load_public_api_fixture,
    run_all_public_api_fixtures, run_public_api_fixture,
};

#[test]
fn public_rust_api_corpus_passes() {
    let count = run_all_public_api_fixtures().expect("public-rust-api fixtures");
    assert_eq!(
        count, 83,
        "expected the PR-006–PR-012 public-rust-api corpus size, found {count}"
    );
}

#[test]
fn public_rust_api_corpus_includes_reducer_subjects_through_pr012() {
    let paths = discover_public_api_fixtures().expect("discover public-rust-api fixtures");
    let subjects = paths
        .iter()
        .map(|path| load_public_api_fixture(path).expect("load fixture").subject)
        .collect::<BTreeSet<_>>();
    for subject in [
        "run-phase",
        "kernel-input",
        "committed-batch",
        "kernel-state",
        "pr009-record",
        "pr010-record",
        "pr011-record",
        "pr012-record",
    ] {
        assert!(
            subjects.contains(subject),
            "missing PR-009 subject {subject}"
        );
    }
}

#[test]
fn run_phase_fixture_requires_the_complete_ordered_vocabulary() {
    let path = finstack_ai_test::compatibility_fixture(
        "public-rust-api/v1/run-phase/roundtrip--exact-vocabulary.json",
    );
    let mut fixture = load_public_api_fixture(path).expect("load RunPhase vocabulary fixture");
    fixture
        .input
        .as_mut()
        .and_then(serde_json::Value::as_array_mut)
        .expect("RunPhase vocabulary input")
        .pop();
    run_public_api_fixture(&fixture).expect_err("incomplete RunPhase vocabulary must fail");
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
