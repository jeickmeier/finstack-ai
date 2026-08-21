//! Conformance runner and adapter tests (golden-trace baseline).

use finstack_ai_test::{
    AdapterCapability, ConformanceAdapter, ConformanceRunner, DeferredBindingAdapter,
    NoOpRustAdapter, TargetKind, compatibility_fixture,
};

#[test]
fn rust_noop_adapter_passes_noop_trace() {
    let runner = ConformanceRunner::new();
    let report = runner
        .run_path(
            compatibility_fixture("golden-trace/v1/trace/valid--noop.json"),
            &NoOpRustAdapter,
        )
        .expect("runner");
    assert!(report.passed);
    assert!(!report.deferred);
    assert_eq!(report.target, TargetKind::Rust);
    assert_eq!(report.trace_id, "noop");
}

#[test]
fn rust_noop_adapter_passes_durable_vs_transient_trace() {
    let runner = ConformanceRunner::new();
    let report = runner
        .run_path(
            compatibility_fixture("golden-trace/v1/trace/valid--durable-vs-transient.json"),
            &NoOpRustAdapter,
        )
        .expect("runner");
    assert!(report.passed);
    assert_eq!(report.trace_id, "durable-vs-transient");
}

#[test]
fn python_and_wasm_adapters_are_deferred_and_never_pass() {
    let runner = ConformanceRunner::new();
    let path = compatibility_fixture("golden-trace/v1/trace/valid--noop.json");

    let python = DeferredBindingAdapter::python();
    assert_eq!(python.capability(), AdapterCapability::Unavailable);
    let python_report = runner.run_path(&path, &python).expect("python runner");
    assert!(!python_report.passed);
    assert!(python_report.deferred);
    assert_eq!(python_report.target, TargetKind::Python);
    assert!(python_report.message.contains("pytest"));

    let wasm = DeferredBindingAdapter::wasm();
    assert_eq!(wasm.capability(), AdapterCapability::Unavailable);
    let wasm_report = runner.run_path(&path, &wasm).expect("wasm runner");
    assert!(!wasm_report.passed);
    assert!(wasm_report.deferred);
    assert_eq!(wasm_report.target, TargetKind::Wasm);
    assert!(wasm_report.message.contains("Playwright"));
}
