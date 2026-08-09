//! Criterion microbenchmarks for conformance and PR-009 reducer hot paths.
//!
//! The reducer group executes a deterministic scripted model trace with no
//! provider, network, storage, or runtime latency.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_test::{
    ConformanceRunner, NoOpRustAdapter, ReducerRustAdapter, compare_normalized_bytes,
    compatibility_fixture, execute_reducer_trace, load_golden_trace, load_noop_trace,
    normalize_json_value,
};

fn conformance_noop(c: &mut Criterion) {
    let mut group = c.benchmark_group("conformance_noop");
    group.sample_size(20);
    group.warm_up_time(std::time::Duration::from_millis(300));
    group.measurement_time(std::time::Duration::from_secs(1));

    group.bench_function("load_normalize_compare", |bencher| {
        bencher.iter(|| {
            let trace = load_noop_trace().expect("load noop");
            let value = serde_json::to_value(&trace).expect("serialize");
            let bytes = normalize_json_value(&value);
            compare_normalized_bytes(&value, &value).expect("compare");
            black_box(bytes);
        });
    });

    group.bench_function("runner_noop_adapter", |bencher| {
        let runner = ConformanceRunner::new();
        let adapter = NoOpRustAdapter;
        bencher.iter(|| {
            let report = runner
                .run_trace(&load_noop_trace().expect("load"), &adapter)
                .expect("run");
            assert!(report.passed);
            black_box(report);
        });
    });

    group.finish();
}

fn scripted_model_reducer(c: &mut Criterion) {
    let mut group = c.benchmark_group("scripted_model_reducer");
    group.sample_size(20);
    group.warm_up_time(std::time::Duration::from_millis(300));
    group.measurement_time(std::time::Duration::from_secs(1));

    let trace = load_golden_trace(compatibility_fixture(
        "golden-trace/v1/trace/valid--pr009-model-completed.json",
    ))
    .expect("load PR-009 reducer trace");
    let runner = ConformanceRunner::new();
    let adapter = ReducerRustAdapter;
    group.bench_function("execute_model_only_completion", |bencher| {
        bencher.iter(|| {
            let execution =
                execute_reducer_trace(black_box(&trace)).expect("execute reducer trace");
            black_box(execution);
        });
    });
    group.bench_function("runner_model_only_completion", |bencher| {
        bencher.iter(|| {
            let report = runner
                .run_trace(black_box(&trace), &adapter)
                .expect("run reducer trace");
            assert!(report.passed);
            black_box(report);
        });
    });

    group.finish();
}

criterion_group!(benches, conformance_noop, scripted_model_reducer);
criterion_main!(benches);
