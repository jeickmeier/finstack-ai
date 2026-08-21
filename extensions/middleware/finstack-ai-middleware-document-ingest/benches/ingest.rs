//! Document-ingest middleware benchmarks: the `BeforeModel` invoke path.
//!
//! Measures one full invocation with a single supported `File` block (small
//! CSV and a synthetic ~256 KiB CSV), and the no-attachment `Continue` path.
//! The repeated `bencher.iter` invocations model a multi-cycle run hitting
//! `BeforeModel` once per cycle with the same attachment.

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::MessageRole;
use finstack_ai_middleware_document_ingest::{AttachmentIndex, DocumentIngestMiddleware};
use finstack_ai_runtime::{Middleware as _, StageOutcome};

#[path = "../src/test_support.rs"]
mod test_support;

use test_support::{
    CaptureArtifactStore, SAMPLE_CSV, before_model_input, before_model_input_with_file, block_on,
    message, middleware_context, stage, text,
};

/// Synthetic ~256 KiB CSV so parse cost is measurable above call overhead.
fn large_csv() -> Vec<u8> {
    use std::fmt::Write as _;
    let mut csv = String::from("quarter,region,revenue,cost,margin\n");
    let mut row = 0_u32;
    while csv.len() < 256 * 1024 {
        let _ = writeln!(
            csv,
            "Q{q},region-{r},{rev},{cost},{margin}",
            q = row % 4 + 1,
            r = row % 17,
            rev = u64::from(row) * 1_037,
            cost = u64::from(row) * 613,
            margin = f64::from(row % 100) / 100.0,
        );
        row += 1;
    }
    csv.into_bytes()
}

fn invoke_replaces(middleware: &DocumentIngestMiddleware, input: finstack_ai_runtime::StageInput) {
    let outcome = block_on(middleware.invoke(middleware_context(), input)).expect("outcome");
    assert!(matches!(outcome, StageOutcome::Replace(_)));
}

fn middleware_benches(criterion: &mut Criterion) {
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let csv_artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    index.insert(csv_artifact.clone());
    let big = large_csv();
    let big_artifact = stage(store.as_ref(), &big, "text/csv", "big.csv");
    index.insert(big_artifact.clone());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");

    criterion.bench_function("middleware_invoke_csv_file_block", |bencher| {
        bencher.iter(|| invoke_replaces(&middleware, before_model_input_with_file(&csv_artifact)));
    });
    criterion.bench_function("middleware_invoke_large_csv_file_block", |bencher| {
        bencher.iter(|| invoke_replaces(&middleware, before_model_input_with_file(&big_artifact)));
    });
    criterion.bench_function("middleware_invoke_text_only_continue", |bencher| {
        bencher.iter(|| {
            let input = before_model_input(vec![message(
                1,
                MessageRole::User,
                vec![text("no attachments here")],
            )]);
            let outcome =
                block_on(middleware.invoke(middleware_context(), input)).expect("outcome");
            assert!(matches!(outcome, StageOutcome::Continue));
        });
    });
}

criterion_group!(benches, middleware_benches);
criterion_main!(benches);
