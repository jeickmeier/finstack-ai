//! Document ingestion benchmarks: parser engine and toolset call path.
//!
//! Covers `parser::parse` on every checked-in fixture, `classify_pdf`, a
//! synthetic large CSV (measurable parse cost), and the full
//! `document_parse` tool-call path over an artifact source.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    ArtifactId, ArtifactRef, BlobRef, Digest, EffectId, EffectOutputContract, EffectOutputKind,
    LaneId, Metadata, OperationLocator, PrincipalRef, RawJson, RunId, SessionId, ToolBatchId,
    ToolCallBlock, ToolCallId, ToolFailurePolicy, ValidatedToolCall,
};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{AuthorizationContext, CancellationSignal, RunCallContext};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolStreamItem, Toolset as _};
use finstack_ai_tools_document::DocumentToolset;
use finstack_ai_tools_document::parser::{self, DocumentLimits};
use futures_util::StreamExt;

const TEXT_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/text.pdf");
const SAMPLE_DOCX: &[u8] = include_bytes!("../../../../fixtures/documents/sample.docx");
const SAMPLE_XLSX: &[u8] = include_bytes!("../../../../fixtures/documents/sample.xlsx");
const SAMPLE_PPTX: &[u8] = include_bytes!("../../../../fixtures/documents/sample.pptx");
const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

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

fn parser_benches(criterion: &mut Criterion) {
    let limits = DocumentLimits::default();
    let cases: [(&str, &[u8], Option<&str>); 5] = [
        ("parse_text_pdf", TEXT_PDF, Some("application/pdf")),
        (
            "parse_sample_docx",
            SAMPLE_DOCX,
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        ),
        (
            "parse_sample_xlsx",
            SAMPLE_XLSX,
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        ),
        (
            "parse_sample_pptx",
            SAMPLE_PPTX,
            Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        ),
        ("parse_sample_csv", SAMPLE_CSV, Some("text/csv")),
    ];
    for (name, bytes, hint) in cases {
        criterion.bench_function(name, |bencher| {
            bencher.iter(|| parser::parse(std::hint::black_box(bytes), hint, &limits));
        });
    }
    let big = large_csv();
    criterion.bench_function("parse_large_csv_256k", |bencher| {
        bencher.iter(|| parser::parse(std::hint::black_box(&big), Some("text/csv"), &limits));
    });
    criterion.bench_function("classify_text_pdf", |bencher| {
        bencher.iter(|| parser::classify_pdf(std::hint::black_box(TEXT_PDF)));
    });
}

// --- toolset call-path scaffolding (mirrors src/tests.rs) ---

#[derive(Clone, Default)]
struct CaptureArtifactStore {
    staged: Arc<Mutex<Vec<(ArtifactScope, Bytes, ArtifactMetadata)>>>,
}

impl ArtifactStore for CaptureArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        let staged = Arc::clone(&self.staged);
        Box::pin(async move {
            staged.lock().expect("capture lock").push((
                scope.clone(),
                content.clone(),
                metadata.clone(),
            ));
            let content_digest = Digest::blob_content(&content);
            let blob = BlobRef::try_new(
                format!(
                    "capture-blob-{}",
                    metadata.name.as_deref().unwrap_or("anon")
                ),
                metadata.media_type.as_ref(),
                u64::try_from(content.len()).expect("length"),
                Some(content_digest),
                metadata.name.as_deref(),
            )
            .expect("blob");
            Ok(ArtifactRef::try_new(
                ArtifactId::from_bytes([9; 16]),
                metadata.kind.as_ref(),
                blob,
                content_digest,
                scope.digest().expect("scope digest"),
                metadata.attributes,
            )
            .expect("artifact"))
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        let staged = Arc::clone(&self.staged);
        Box::pin(async move {
            staged
                .lock()
                .expect("capture lock")
                .iter()
                .find(|(_, content, _)| Digest::blob_content(content) == artifact.content_digest())
                .map(|(_, content, _)| content.clone())
                .ok_or(ArtifactError::NotFound)
        })
    }
}

fn test_scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: Some(RunId::from_bytes([3; 16])),
        sensitivity: finstack_ai_kernel::Sensitivity::Internal,
    }
}

fn call_context() -> ToolCallContext {
    ToolCallContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: EffectId::from_bytes([4; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
            relation_depth: 0,
        },
        tool_batch_id: ToolBatchId::from_bytes([5; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

fn stage(store: &dyn ArtifactStore, bytes: &[u8], media: &str, name: &str) -> ArtifactRef {
    block_on(finstack_ai_runtime::artifact::stage_required_artifact(
        store,
        test_scope(),
        Bytes::copy_from_slice(bytes),
        ArtifactMetadata {
            kind: Arc::from("attachment"),
            media_type: Arc::from(media),
            name: Some(Arc::from(name)),
            attributes: Metadata::empty(),
        },
    ))
    .expect("staged")
}

fn validated_call(
    toolset: &DocumentToolset,
    name: &str,
    arguments: &serde_json::Value,
) -> ValidatedToolCall {
    let tools = toolset.tools();
    let spec = tools
        .iter()
        .find(|spec| spec.model_name.as_ref() == name)
        .expect("tool spec exists");
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::from_bytes([6; 16]),
            name,
            RawJson::parse(serde_json::to_vec(arguments).expect("arguments json"))
                .expect("arguments"),
        )
        .expect("call"),
        tool_id: spec.id.clone(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: spec.retry_safety,
        deadline: None,
        execution: spec.execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

fn call_tool_once(toolset: &DocumentToolset, name: &str, arguments: &serde_json::Value) {
    let mut stream =
        block_on(toolset.call(call_context(), validated_call(toolset, name, arguments)))
            .expect("call succeeds");
    let item = block_on(stream.next())
        .expect("stream item")
        .expect("stream ok");
    let ToolStreamItem::Completed(result) = item else {
        panic!("expected a completed tool result");
    };
    assert!(!result.is_error);
}

fn toolset_benches(criterion: &mut Criterion) {
    let store = Arc::new(CaptureArtifactStore::default());
    let csv_artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "sample.csv");
    let big = large_csv();
    let big_artifact = stage(store.as_ref(), &big, "text/csv", "big.csv");
    let pdf_artifact = stage(store.as_ref(), TEXT_PDF, "application/pdf", "text.pdf");
    let toolset = DocumentToolset::try_new(store).expect("toolset");

    let csv_args = serde_json::json!({
        "artifact": serde_json::to_value(&csv_artifact).expect("json"),
    });
    criterion.bench_function("toolset_document_parse_csv_artifact", |bencher| {
        bencher.iter(|| call_tool_once(&toolset, "document_parse", &csv_args));
    });
    let big_args = serde_json::json!({
        "artifact": serde_json::to_value(&big_artifact).expect("json"),
    });
    criterion.bench_function("toolset_document_parse_large_csv_artifact", |bencher| {
        bencher.iter(|| call_tool_once(&toolset, "document_parse", &big_args));
    });
    let classify_args = serde_json::json!({
        "artifact": serde_json::to_value(&pdf_artifact).expect("json"),
    });
    criterion.bench_function("toolset_pdf_classify_artifact", |bencher| {
        bencher.iter(|| call_tool_once(&toolset, "pdf_classify", &classify_args));
    });
}

criterion_group!(benches, parser_benches, toolset_benches);
criterion_main!(benches);
