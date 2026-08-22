use crate::parser::{
    self, DocumentClassification, DocumentFormat, DocumentLimits, DocumentParseError,
};

const TEXT_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/text.pdf");
const SCANNED_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/scanned.pdf");
const SAMPLE_DOCX: &[u8] = include_bytes!("../../../../fixtures/documents/sample.docx");
const SAMPLE_XLSX: &[u8] = include_bytes!("../../../../fixtures/documents/sample.xlsx");
const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");
const CORRUPT: &[u8] = include_bytes!("../../../../fixtures/documents/corrupt.bin");

#[test]
fn parses_text_pdf_to_markdown() {
    let parsed = parser::parse(
        TEXT_PDF,
        Some("application/pdf"),
        &DocumentLimits::default(),
    )
    .expect("text pdf parses");
    assert_eq!(parsed.format, DocumentFormat::Pdf);
    assert!(parsed.markdown.contains("Quarterly Revenue Report"));
    assert!(!parsed.requires_ocr);
    assert!(!parsed.truncated);
}

#[test]
fn scanned_pdf_is_success_with_requires_ocr() {
    let parsed = parser::parse(
        SCANNED_PDF,
        Some("application/pdf"),
        &DocumentLimits::default(),
    )
    .expect("scanned pdf is a successful parse");
    assert!(parsed.requires_ocr);
    assert_eq!(parsed.classification, Some(DocumentClassification::Scanned));
}

#[test]
fn parses_docx_xlsx_csv() {
    for (bytes, media, format) in [
        (
            SAMPLE_DOCX,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            DocumentFormat::Docx,
        ),
        (
            SAMPLE_XLSX,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            DocumentFormat::Xlsx,
        ),
        (SAMPLE_CSV, "text/csv", DocumentFormat::Csv),
    ] {
        let parsed =
            parser::parse(bytes, Some(media), &DocumentLimits::default()).expect("fixture parses");
        assert_eq!(parsed.format, format);
        assert!(!parsed.markdown.is_empty());
    }
}

#[test]
fn detected_format_wins_over_wrong_media_type() {
    let parsed = parser::parse(
        SAMPLE_DOCX,
        Some("application/pdf"),
        &DocumentLimits::default(),
    )
    .expect("content sniffing wins");
    assert_eq!(parsed.format, DocumentFormat::Docx);
}

#[test]
fn corrupt_bytes_fail_with_parse_or_unsupported() {
    let error = parser::parse(CORRUPT, None, &DocumentLimits::default())
        .expect_err("corrupt bytes must not parse");
    assert!(matches!(
        error,
        DocumentParseError::ParseFailed { .. } | DocumentParseError::UnsupportedFormat
    ));
}

#[test]
fn oversized_input_is_rejected_not_truncated() {
    let limits = DocumentLimits {
        max_input_bytes: 16,
        ..DocumentLimits::default()
    };
    assert!(matches!(
        parser::parse(SAMPLE_CSV, Some("text/csv"), &limits),
        Err(DocumentParseError::TooLarge { .. })
    ));
}

#[test]
fn oversized_output_is_truncated_with_flag() {
    let limits = DocumentLimits {
        max_output_bytes: 8,
        ..DocumentLimits::default()
    };
    let parsed = parser::parse(SAMPLE_CSV, Some("text/csv"), &limits).expect("parses");
    assert!(parsed.truncated);
    assert!(parsed.markdown.len() <= 8);
}

#[test]
fn classify_pdf_distinguishes_text_and_scanned() {
    let (text_class, pages) = parser::classify_pdf(TEXT_PDF).expect("classifies");
    assert_eq!(text_class, DocumentClassification::Text);
    assert_eq!(pages, 1);
    let (scanned_class, _) = parser::classify_pdf(SCANNED_PDF).expect("classifies");
    assert_eq!(scanned_class, DocumentClassification::Scanned);
}

#[test]
fn classify_rejects_non_pdf() {
    assert!(matches!(
        parser::classify_pdf(SAMPLE_DOCX),
        Err(DocumentParseError::UnsupportedFormat)
    ));
}

use crate::DocumentToolset;
use finstack_ai_runtime::ports::tool::Toolset as _;

#[test]
fn toolset_exposes_two_validated_tools() {
    let toolset =
        DocumentToolset::try_new(Arc::new(CaptureArtifactStore::default())).expect("toolset");
    let tools = toolset.tools();
    assert_eq!(tools.len(), 2);
    let names: Vec<&str> = tools.iter().map(|spec| spec.model_name.as_ref()).collect();
    assert_eq!(names, ["document_parse", "pdf_classify"]);
    for spec in tools.iter() {
        spec.validate().expect("spec validates");
    }
    // Every registered `ToolSpec::id` must be globally unique per
    // `ToolCatalog::try_new` (shared model-visible namespace), so the two
    // document tools carry distinct ids even though they live in one
    // toolset.
    assert_eq!(tools[0].id.as_str(), "finstack.tools.document.parse");
    assert_eq!(tools[1].id.as_str(), "finstack.tools.document.classify");
    assert_ne!(tools[0].id, tools[1].id);
    assert_eq!(toolset.descriptor().name.as_ref(), "finstack-document");
}

use std::future::Future;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ArtifactId, ArtifactRef, BlobRef, Digest, EffectId, EffectOutputContract, EffectOutputKind,
    LaneId, Metadata, OperationLocator, PrincipalRef, RawJson, RunId, SessionId, ToolBatchId,
    ToolCallBlock, ToolCallId, ToolFailurePolicy,
};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    AuthorizationContext, CancellationSignal, RunCallContext, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError, ToolStreamItem};
use futures_util::StreamExt;

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("test runtime")
        .block_on(future)
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

fn validated_call(
    toolset: &DocumentToolset,
    name: &str,
    arguments: &serde_json::Value,
) -> finstack_ai_kernel::ValidatedToolCall {
    let spec = toolset
        .tools
        .iter()
        .find(|spec| spec.model_name.as_ref() == name)
        .expect("tool spec exists");
    finstack_ai_kernel::ValidatedToolCall {
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

/// Test-only: override one tool spec's `max_result_bytes` so the spill
/// branch can be exercised without an oversized fixture. `tools` is
/// `pub(crate)`, so this mutation is only reachable from within the crate.
fn with_max_result_bytes(
    mut toolset: DocumentToolset,
    name: &str,
    max_result_bytes: u64,
) -> DocumentToolset {
    let tools: Vec<ToolSpec> = toolset
        .tools
        .iter()
        .cloned()
        .map(|mut spec| {
            if spec.model_name.as_ref() == name {
                spec.max_result_bytes = max_result_bytes;
            }
            spec
        })
        .collect();
    toolset.tools = Arc::from(tools);
    toolset
}

fn call_tool(
    toolset: &DocumentToolset,
    name: &str,
    arguments: &serde_json::Value,
) -> serde_json::Value {
    let mut stream =
        block_on(toolset.call(call_context(), validated_call(toolset, name, arguments)))
            .expect("call succeeds");
    let result = loop {
        match block_on(stream.next())
            .expect("stream item")
            .expect("stream ok")
        {
            ToolStreamItem::Artifact(_) => {}
            ToolStreamItem::Completed(result) => break result,
            _ => panic!("unexpected document tool stream item"),
        }
    };
    assert!(block_on(stream.next()).is_none());
    assert!(!result.is_error);
    serde_json::from_slice(result.output.as_bytes()).expect("json output")
}

fn call_tool_err(
    toolset: &DocumentToolset,
    name: &str,
    arguments: &serde_json::Value,
) -> ToolError {
    match block_on(toolset.call(call_context(), validated_call(toolset, name, arguments))) {
        Err(error) => error,
        Ok(_) => panic!("call unexpectedly succeeded"),
    }
}

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
                "capture-blob",
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

#[test]
fn document_parse_via_artifact_source_returns_markdown() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "sample.csv");
    let toolset = DocumentToolset::try_new(store).expect("toolset");
    let arguments = serde_json::json!({
        "artifact": serde_json::to_value(&artifact).expect("artifact json"),
    });
    let output = call_tool(&toolset, "document_parse", &arguments);
    assert!(
        output["markdown"]
            .as_str()
            .expect("markdown")
            .contains("Q1")
    );
    assert_eq!(output["format"], "csv");
    assert_eq!(output["requires_ocr"], false);
}

#[test]
fn document_parse_rejects_invalid_artifact_reference() {
    let toolset =
        DocumentToolset::try_new(Arc::new(CaptureArtifactStore::default())).expect("toolset");
    let arguments = serde_json::json!({"artifact": {}});
    let error = call_tool_err(&toolset, "document_parse", &arguments);
    assert_eq!(error.code().as_str(), crate::DOCUMENT_INVALID_ARGUMENTS);
}

#[test]
fn pdf_classify_via_artifact_source() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SCANNED_PDF, "application/pdf", "scan.pdf");
    let toolset = DocumentToolset::try_new(store).expect("toolset");
    let arguments = serde_json::json!({"artifact": serde_json::to_value(&artifact).expect("json")});
    let output = call_tool(&toolset, "pdf_classify", &arguments);
    assert_eq!(output["classification"], "scanned");
    assert_eq!(output["page_count"], 1);
}

#[test]
fn missing_artifact_or_path_property_is_invalid() {
    let toolset =
        DocumentToolset::try_new(Arc::new(CaptureArtifactStore::default())).expect("toolset");
    for arguments in [
        serde_json::json!({}),
        serde_json::json!({"path": "/tmp/x.pdf"}),
    ] {
        let error = call_tool_err(&toolset, "document_parse", &arguments);
        assert_eq!(error.code().as_str(), crate::DOCUMENT_INVALID_ARGUMENTS);
    }
}

#[test]
fn unsupported_format_maps_to_stable_code() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), CORRUPT, "application/octet-stream", "x.bin");
    let toolset = DocumentToolset::try_new(store).expect("toolset");
    let arguments = serde_json::json!({"artifact": serde_json::to_value(&artifact).expect("json")});
    let error = call_tool_err(&toolset, "document_parse", &arguments);
    assert!(matches!(
        error.code().as_str(),
        crate::DOCUMENT_UNSUPPORTED_FORMAT | crate::DOCUMENT_PARSE_FAILED
    ));
}

#[test]
fn page_range_is_rejected_for_non_pdf_sources() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "sample.csv");
    let toolset = DocumentToolset::try_new(store).expect("toolset");
    let arguments = serde_json::json!({
        "artifact": serde_json::to_value(&artifact).expect("json"),
        "page_range": [1, 1],
    });
    let error = call_tool_err(&toolset, "document_parse", &arguments);
    assert_eq!(error.code().as_str(), crate::DOCUMENT_INVALID_ARGUMENTS);
}

#[test]
fn page_range_rejects_zero_based_and_reversed_ranges() {
    for range in [(0u32, 1u32), (2, 1)] {
        let store = Arc::new(CaptureArtifactStore::default());
        let artifact = stage(store.as_ref(), TEXT_PDF, "application/pdf", "text.pdf");
        let toolset = DocumentToolset::try_new(store).expect("toolset");
        let arguments = serde_json::json!({
            "artifact": serde_json::to_value(&artifact).expect("json"),
            "page_range": [range.0, range.1],
        });
        let error = call_tool_err(&toolset, "document_parse", &arguments);
        assert_eq!(error.code().as_str(), crate::DOCUMENT_INVALID_ARGUMENTS);
    }
}

#[test]
fn page_range_extracts_a_pdf_page() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), TEXT_PDF, "application/pdf", "text.pdf");
    let toolset = DocumentToolset::try_new(store).expect("toolset");
    let arguments = serde_json::json!({
        "artifact": serde_json::to_value(&artifact).expect("json"),
        "page_range": [1, 1],
    });
    let output = call_tool(&toolset, "document_parse", &arguments);
    assert_eq!(output["format"], "pdf");
    assert_eq!(output["page_count"], 1);
}

// Force the spill branch without an oversized fixture: below the real
// (multi-KB) full output for `document_parse_spills_oversized_output_to_artifact`'s
// synthetic CSV, but well above the staged-artifact-reference JSON overhead
// alone, so a truncated-but-nonempty inline markdown is achievable.
const SPILL_TEST_MAX_RESULT_BYTES: u64 = 800;

#[test]
fn document_parse_spills_oversized_output_to_artifact() {
    // A small fixture's full output is smaller than the ~540-byte JSON
    // overhead of a real staged `spilled_artifact` reference, so no
    // `max_result_bytes` value could exercise the spill branch and still
    // succeed. Synthesize a CSV large enough that its full markdown table
    // output exceeds that overhead by a comfortable margin, in memory (no
    // new fixture file).
    use std::fmt::Write as _;
    let mut big_csv = String::from("quarter,revenue\n");
    for i in 0..200 {
        let _ = writeln!(big_csv, "Q{i},{}", i * 1000);
    }
    let big_csv = big_csv.into_bytes();

    let store: Arc<dyn ArtifactStore> = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), &big_csv, "text/csv", "big.csv");
    let toolset = DocumentToolset::try_new(Arc::clone(&store)).expect("toolset");
    let toolset = with_max_result_bytes(toolset, "document_parse", SPILL_TEST_MAX_RESULT_BYTES);
    let arguments = serde_json::json!({"artifact": serde_json::to_value(&artifact).expect("json")});
    let output = call_tool(&toolset, "document_parse", &arguments);

    // (c) truncated flag semantics are coherent: spill always truncates the
    // inline copy, even when the underlying parse itself was not truncated.
    assert_eq!(output["truncated"], true);

    // (b) the inline JSON total size fits the configured ceiling — this is
    // the exact bug being regression-tested: a budget computed from the
    // pre-spill (`spilled_artifact: null`) skeleton undercounts the real,
    // hundreds-of-bytes `ArtifactRef` JSON and can overshoot the ceiling.
    let total_size = serde_json::to_vec(&output).expect("serialize output").len();
    assert!(
        u64::try_from(total_size).expect("size fits u64") <= SPILL_TEST_MAX_RESULT_BYTES,
        "inline result must respect max_result_bytes={SPILL_TEST_MAX_RESULT_BYTES}, got {total_size} bytes: {output}"
    );

    // (a) spilled_artifact is a valid ArtifactRef whose stored bytes carry
    // the FULL (untruncated) markdown.
    let spilled = output["spilled_artifact"].clone();
    assert!(!spilled.is_null(), "expected a spilled_artifact reference");
    let artifact_ref: ArtifactRef = serde_json::from_value(spilled).expect("artifact ref");
    let stored = block_on(store.get(test_scope(), artifact_ref)).expect("stored bytes");
    let full_markdown = String::from_utf8_lossy(&stored);
    assert!(full_markdown.contains("Q1"));
    let inline_markdown = output["markdown"].as_str().expect("inline markdown");
    assert!(
        full_markdown.len() > inline_markdown.len(),
        "the spilled artifact must carry more content than the truncated inline copy"
    );
}
