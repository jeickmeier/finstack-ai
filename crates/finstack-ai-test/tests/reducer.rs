//! Real model-only reducer baseline reducer-backed golden-trace acceptance tests.

use finstack_ai_kernel::{ContentBlock, Kernel, KernelState};
use finstack_ai_test::{
    AdapterOutcome, ConformanceAdapter, ConformanceRunner, DurabilityClass, ExpectedTrace,
    GoldenTrace, ReducerRustAdapter, TraceError, compatibility_fixture, execute_reducer_trace,
    load_golden_trace, normalize_json_value, project_reducer_terminal,
};
use serde_json::{Value, json};

const DIRECT_TRACE: &str = "golden-trace/v1/trace/valid--pr009-model-completed.json";
const SPLIT_TRACE: &str = "golden-trace/v1/trace/valid--pr009-model-completed-split.json";
const DEFERRED_TRACE: &str = "golden-trace/v1/trace/valid--pr009-model-deferred.json";
const CONTINUATION_TRACE: &str = "golden-trace/v1/trace/valid--pr009-before-finalize-continue.json";
const EMPTY_EXTERNAL_TRACE: &str =
    "golden-trace/v1/trace/valid--pr009-external-completed-empty.json";

const ALL_PR009_TRACES: [&str; 5] = [
    DIRECT_TRACE,
    SPLIT_TRACE,
    DEFERRED_TRACE,
    CONTINUATION_TRACE,
    EMPTY_EXTERNAL_TRACE,
];

fn load_trace(relative: &str) -> GoldenTrace {
    load_golden_trace(compatibility_fixture(relative))
        .expect("load model-only reducer baseline trace")
}

fn execute_trace(trace: &GoldenTrace) -> ExpectedTrace {
    let outcome = ReducerRustAdapter
        .execute(trace)
        .expect("execute reducer-backed trace");
    let AdapterOutcome::Observed(observed) = outcome else {
        panic!("real Rust adapter must return observed output");
    };
    serde_json::from_value(observed).expect("observed ExpectedTrace projection")
}

fn durable_final_projection(trace: &ExpectedTrace) -> Vec<u8> {
    normalize_json_value(&json!({
        "durable_records": trace.durable_records,
        "effects": trace.effects,
        "final_state": trace.final_state,
        "final_result": trace.final_result,
        "state_hash": trace.state_hash,
    }))
}

fn event_projection(trace: &ExpectedTrace, durability: DurabilityClass) -> Vec<u8> {
    let events = trace
        .normalized_events
        .iter()
        .filter(|event| event.durability == durability)
        .collect::<Vec<_>>();
    normalize_json_value(&serde_json::to_value(events).expect("serialize normalized events"))
}

fn assistant_text(state: &KernelState) -> String {
    let [message] = state.messages.as_slice() else {
        panic!("expected exactly one durable assistant message");
    };
    message
        .content()
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.text().to_owned(),
            other => panic!("unexpected content block: {other:?}"),
        })
        .collect()
}

fn assert_single_empty_assistant_text(state: &KernelState) {
    assert_eq!(assistant_text(state), "");
}

fn assert_apply_only_replay(relative: &str) {
    let trace = load_trace(relative);
    let first = execute_reducer_trace(&trace).expect("execute and retain committed batches");
    let mut replay = Kernel::default();
    let mut next_transient_sequence = 0_u64;
    for batch in &first.committed_batches {
        let events = replay
            .apply(batch, next_transient_sequence)
            .expect("apply committed replay batch");
        next_transient_sequence = next_transient_sequence
            .checked_add(u64::try_from(events.len()).expect("event count fits u64"))
            .expect("event sequence overflow");
    }
    let replayed = project_reducer_terminal(&replay).expect("project replayed kernel");
    assert_eq!(
        replayed.final_state, first.observed.final_state,
        "{relative}"
    );
    assert_eq!(
        replayed.final_result, first.observed.final_result,
        "{relative}"
    );
    assert_eq!(replayed.state_hash, first.observed.state_hash, "{relative}");
    assert_eq!(replayed.state_hash, trace.expected.state_hash, "{relative}");
    assert_eq!(replay.state(), &first.kernel_state, "{relative}");
}

#[test]
fn model_only_fixtures_execute_through_real_reducer_and_match() {
    let runner = ConformanceRunner::new();
    for relative in ALL_PR009_TRACES {
        let report = runner
            .run_path(compatibility_fixture(relative), &ReducerRustAdapter)
            .expect("run reducer-backed fixture");
        assert!(report.passed, "{relative}: {}", report.message);
        assert!(!report.deferred, "{relative}");
    }
}

#[test]
fn model_chunk_splits_do_not_change_durable_batches_or_state() {
    let direct_trace = load_trace(DIRECT_TRACE);
    let split_trace = load_trace(SPLIT_TRACE);
    let direct = execute_reducer_trace(&direct_trace).expect("direct");
    let split = execute_reducer_trace(&split_trace).expect("split");

    assert_eq!(
        durable_final_projection(&direct.observed),
        durable_final_projection(&split.observed)
    );
    assert_eq!(
        event_projection(&direct.observed, DurabilityClass::Durable),
        event_projection(&split.observed, DurabilityClass::Durable),
        "chunk splits cannot change durable events"
    );
    assert_eq!(
        event_projection(&direct.observed, DurabilityClass::Transient),
        normalize_json_value(&json!([{
            "kind": "model_text_delta",
            "durability": "transient",
            "id": "00000000-0000-7000-8000-000000009001",
            "payload": {"text": "hello world"}
        }])),
        "runtime-harness output includes direct model progress"
    );
    assert_eq!(
        event_projection(&split.observed, DurabilityClass::Transient),
        normalize_json_value(&json!([
            {
                "kind": "model_text_delta",
                "durability": "transient",
                "id": "00000000-0000-7000-8000-000000009002",
                "payload": {"text": "hello "}
            },
            {
                "kind": "model_text_delta",
                "durability": "transient",
                "id": "00000000-0000-7000-8000-000000009003",
                "payload": {"text": "world"}
            }
        ])),
        "runtime-harness output includes split model progress"
    );
    assert_eq!(
        direct.committed_batches, split.committed_batches,
        "chunk count must not change committed batches"
    );
    assert_eq!(direct.kernel_state, split.kernel_state);
    assert_eq!(assistant_text(&direct.kernel_state), "hello world");
    assert_eq!(assistant_text(&split.kernel_state), "hello world");
}

#[test]
fn all_pr009_committed_batches_replay_without_decide() {
    for relative in ALL_PR009_TRACES {
        assert_apply_only_replay(relative);
    }
}

#[test]
fn empty_external_completion_replays_one_empty_text_block() {
    let trace = load_trace(EMPTY_EXTERNAL_TRACE);
    let first = execute_reducer_trace(&trace).expect("execute empty external completion");
    assert_single_empty_assistant_text(&first.kernel_state);

    let mut replay = Kernel::default();
    let mut next_transient_sequence = 0_u64;
    for batch in &first.committed_batches {
        let events = replay
            .apply(batch, next_transient_sequence)
            .expect("apply committed replay batch");
        next_transient_sequence = next_transient_sequence
            .checked_add(u64::try_from(events.len()).expect("event count fits u64"))
            .expect("event sequence overflow");
    }
    assert_single_empty_assistant_text(replay.state());
    assert_eq!(
        first.observed.state_hash, trace.expected.state_hash,
        "empty external fixture state_hash"
    );
}

#[test]
fn reducer_adapter_never_derives_observed_output_from_expected() {
    let trace = load_trace(DIRECT_TRACE);
    let observed = execute_trace(&trace);
    let mut poisoned = trace;
    poisoned.expected.durable_records.clear();
    poisoned.expected.normalized_events.clear();
    poisoned.expected.effects.clear();
    poisoned.expected.final_state = Value::Null;
    poisoned.expected.final_result = Value::Null;
    poisoned.expected.state_hash = "poisoned-expectation".to_owned();
    assert_eq!(execute_trace(&poisoned), observed);
}

#[test]
fn reducer_adapter_rejects_wrong_versions_and_transition_value_drift() {
    let trace = load_trace(DIRECT_TRACE);
    let cases = [
        {
            let mut value = trace.clone();
            value.format_version = 2;
            value
        },
        {
            let mut value = trace.clone();
            value.scripted_outcomes.format_version = 2;
            value
        },
        {
            let mut value = trace;
            value.transition_env.ids[0] = "not-a-uuid".to_owned();
            value
        },
    ];
    for case in cases {
        let error = ReducerRustAdapter
            .execute(&case)
            .expect_err("invalid fixture input must fail");
        assert!(matches!(error, TraceError::Adapter(_)));
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn reducer_adapter_rejects_a_missing_transition_id() {
    let mut trace = load_trace(DIRECT_TRACE);
    trace.transition_env.ids.pop();
    let error = ReducerRustAdapter
        .execute(&trace)
        .expect_err("missing transition ID must fail");
    assert!(
        error.to_string().contains("ids exhausted"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn reducer_adapter_rejects_a_missing_transition_timestamp() {
    let mut trace = load_trace(DIRECT_TRACE);
    trace.transition_env.timestamps_ms.pop();
    let error = ReducerRustAdapter
        .execute(&trace)
        .expect_err("missing transition timestamp must fail");
    assert!(
        error.to_string().contains("timestamps_ms exhausted"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn reducer_adapter_rejects_an_extra_transition_id() {
    let mut trace = load_trace(DIRECT_TRACE);
    trace
        .transition_env
        .ids
        .push("00000000-0000-7000-8000-00000000ffff".to_owned());
    let error = ReducerRustAdapter
        .execute(&trace)
        .expect_err("extra transition ID must fail");
    assert!(
        error.to_string().contains("unused value"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn reducer_adapter_rejects_an_extra_transition_timestamp() {
    let mut trace = load_trace(DIRECT_TRACE);
    trace.transition_env.timestamps_ms.push(1700);
    let error = ReducerRustAdapter
        .execute(&trace)
        .expect_err("extra transition timestamp must fail");
    assert!(
        error.to_string().contains("unused value"),
        "unexpected diagnostic: {error}"
    );
}
