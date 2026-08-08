//! Criterion microbenchmarks for the Phase 0 conformance hot path.
//!
//! Measures load/normalize/compare for the no-op golden trace. Kernel/binding
//! workloads arrive with later phases; this group proves the harness only.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_test::{
    ConformanceRunner, NoOpRustAdapter, compare_normalized_bytes, load_noop_trace,
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

criterion_group!(benches, conformance_noop);
criterion_main!(benches);
