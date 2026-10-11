use std::sync::Arc;

use finstack_ai_kernel::{
    ArtifactId, ArtifactRef, BlobRef, ContentBlock, Digest, Message, MessageRole, Metadata,
    TEXT_MAX_BYTES, ToolCallId, ToolResultBlock,
};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::InProcessArtifactStore;
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::middleware::{
    MIDDLEWARE_OUTCOME_NOT_ALLOWED, Middleware as _, MiddlewareError, StageOutcome,
};
use finstack_ai_runtime::ports::model::ModelRequestDraft;
use finstack_ai_tools_document::parser::DocumentLimits;

use crate::{
    DOCUMENT_INGEST_INTEGRITY_MISMATCH, DOCUMENT_INGEST_LOOKUP_FAILED, DocumentIngestMiddleware,
    FALLBACK_NOTE_TEXT, LIMITS_IDENTITY_VERSION, PARSE_CACHE_CAPACITY, ParseCache, ParseCacheKey,
    fallback_text_block, limits_identity_bytes, note_block, rebuild_message, skip_note_message,
};

#[path = "test_support.rs"]
mod test_support;

use test_support::{
    CaptureArtifactStore, SAMPLE_CSV, before_model_input, before_model_input_with_file, blob_of,
    block_on, file_block, message, middleware_context, stage, text,
};

const SCANNED_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/scanned.pdf");

/// RFC 8785 canonical JSON for [`DocumentLimits::default`] under
/// [`LIMITS_IDENTITY_VERSION`].
const PINNED_DEFAULT_LIMITS_IDENTITY: &[u8] = br#"{"max_input_bytes":4194304,"max_output_bytes":1048576,"max_pages":500,"version":"document-ingest-limits-v1"}"#;

fn before_model_input_with_dangling_file() -> finstack_ai_runtime::ports::middleware::StageInput {
    let blob = BlobRef::try_new(
        "never-staged-blob",
        "text/csv",
        4,
        Some(Digest::blob_content(b"nope")),
        Some("ghost.csv"),
    )
    .expect("blob");
    before_model_input(vec![message(
        1,
        MessageRole::User,
        vec![text("please review the attachment"), file_block(blob)],
    )])
}

fn before_model_input_text_only() -> finstack_ai_runtime::ports::middleware::StageInput {
    before_model_input(vec![message(
        1,
        MessageRole::User,
        vec![text("no attachments here")],
    )])
}

fn all_text(draft: &ModelRequestDraft) -> String {
    let mut out = String::new();
    for message in draft.messages.iter() {
        for block in message.content() {
            if let ContentBlock::Text(text) = block {
                out.push_str(text.text());
                out.push('\n');
            }
        }
    }
    out
}

fn has_file_blocks(draft: &ModelRequestDraft) -> bool {
    draft.messages.iter().any(message_has_file_blocks)
}

fn message_has_file_blocks(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::File(_)))
}

fn tool_result_block() -> ContentBlock {
    ContentBlock::ToolResult(
        ToolResultBlock::try_new(
            ToolCallId::from_bytes([9; 16]),
            vec![text("tool body")],
            false,
        )
        .expect("tool result"),
    )
}

fn tool_role_message() -> Message {
    message(2, MessageRole::Tool, vec![tool_result_block()])
}

fn construction_error_has_no_file_blocks(error: &MiddlewareError) {
    assert_eq!(error.code(), MIDDLEWARE_OUTCOME_NOT_ALLOWED);
    let text = error.to_string();
    assert!(
        !text.contains("File") && !text.contains("file") && !text.contains("blob-"),
        "construction error must not leak File blocks or blob identity: {text}"
    );
}

/// Store that is index-resolvable but always fails `get`.
struct FailingGetStore;

impl ArtifactStore for FailingGetStore {
    fn stage_put(
        &self,
        _scope: ArtifactScope,
        _content: Bytes,
        _metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        Box::pin(async {
            Err(ArtifactError::Unavailable {
                message: Arc::from("unavailable"),
            })
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        _artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        Box::pin(async {
            Err(ArtifactError::Unavailable {
                message: Arc::from("unavailable"),
            })
        })
    }
}

fn indexed_artifact(ordinal: usize) -> (BlobRef, ArtifactRef) {
    let digest = Digest::blob_content(&ordinal.to_be_bytes());
    let blob = BlobRef::try_new(
        format!("blob-{ordinal}"),
        "text/csv",
        1,
        Some(digest),
        None::<&str>,
    )
    .expect("blob");
    let artifact = ArtifactRef::try_new(
        ArtifactId::from_bytes([1; 16]),
        "attachment",
        blob.clone(),
        digest,
        Digest::raw_json(b"scope"),
        Metadata::empty(),
    )
    .expect("artifact");
    (blob, artifact)
}

fn parse_key(ordinal: usize) -> ParseCacheKey {
    ParseCacheKey {
        tenant_scope: Arc::from("tenant-a"),
        blob: BlobRef::try_new(
            format!("blob-{ordinal}"),
            "text/csv",
            1,
            Some(Digest::blob_content(&ordinal.to_be_bytes())),
            Some(format!("{ordinal}.csv")),
        )
        .expect("blob"),
    }
}

#[test]
fn descriptor_declares_before_model_context_mutation() {
    let store = Arc::new(CaptureArtifactStore::default());
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let descriptor = middleware.descriptor();
    assert!(
        descriptor
            .stages
            .contains(finstack_ai_kernel::Stage::BeforeModel)
    );
    assert_eq!(
        descriptor.order.tier,
        finstack_ai_runtime::ports::middleware::OrderTier::ContextMutation
    );
}

#[test]
fn replaces_supported_file_block_with_markdown_text() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let input = before_model_input_with_file(&artifact);
    let outcome = block_on(middleware.invoke(middleware_context(), input)).expect("outcome");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace, got {outcome:?}");
    };
    let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).expect("draft");
    let text = all_text(&draft);
    assert!(text.contains("revenue.csv"));
    assert!(text.contains("Q1"));
    assert!(!has_file_blocks(&draft));
}

#[test]
fn scanned_pdf_gets_ocr_note() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SCANNED_PDF, "application/pdf", "scan.pdf");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("outcome");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace");
    };
    let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).expect("draft");
    assert!(all_text(&draft).contains("scanned PDF"));
}

#[test]
fn unresolvable_artifact_fails_closed() {
    let store = Arc::new(CaptureArtifactStore::default());
    // Blob references an artifact that was never staged into the index.
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let error = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_dangling_file(),
    ))
    .expect_err("missing artifact must fail closed");
    assert_eq!(error.code(), DOCUMENT_INGEST_LOOKUP_FAILED);
}

#[test]
fn no_file_blocks_means_continue() {
    let store = Arc::new(CaptureArtifactStore::default());
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let outcome = block_on(middleware.invoke(middleware_context(), before_model_input_text_only()))
        .expect("outcome");
    assert!(matches!(outcome, StageOutcome::Continue));
}

#[test]
fn unsupported_media_type_becomes_coded_note() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), b"\x89PNG\r\n", "image/png", "chart.png");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("outcome");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace");
    };
    let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).expect("draft");
    assert!(all_text(&draft).contains("document_ingest:unsupported"));
    assert!(!has_file_blocks(&draft));
}

#[test]
fn non_user_role_message_is_left_untouched() {
    // Assistant messages may legally carry File blocks (per
    // `validate_role_blocks`), but spec decision 14 scopes rewriting to
    // User-role messages only. An Assistant-role File block must survive
    // unchanged and the middleware must report no change (`Continue`),
    // proving it never even inspects non-User content.
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let input = before_model_input(vec![message(
        1,
        MessageRole::Assistant,
        vec![text("here is the file"), file_block(blob_of(&artifact))],
    )]);
    let outcome = block_on(middleware.invoke(middleware_context(), input)).expect("outcome");
    assert!(
        matches!(outcome, StageOutcome::Continue),
        "assistant-role File blocks must not be rewritten"
    );
}

/// Store that resolves every blob the way an id-indexed production store
/// would, but serves `content` regardless of the digest the wire reference
/// declares, and reports `reference_blob` (the wire blob when `None`) as
/// the store-owned reference. Drives the two integrity branches that a
/// digest-indexed stub can never reach.
struct TamperedStore {
    content: &'static [u8],
    reference_blob: Option<BlobRef>,
}

impl ArtifactStore for TamperedStore {
    fn stage_put(
        &self,
        _scope: ArtifactScope,
        _content: Bytes,
        _metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        Box::pin(async {
            Err(ArtifactError::Unavailable {
                message: Arc::from("unavailable"),
            })
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        _artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        Box::pin(async {
            Err(ArtifactError::Unavailable {
                message: Arc::from("unavailable"),
            })
        })
    }

    fn get_by_blob(
        &self,
        scope: ArtifactScope,
        blob: BlobRef,
    ) -> PortFuture<Result<finstack_ai_runtime::artifact::ArtifactRead, ArtifactError>> {
        let content = Bytes::from_static(self.content);
        let reference_blob = self.reference_blob.clone().unwrap_or(blob);
        Box::pin(async move {
            let reference = ArtifactRef::try_new(
                ArtifactId::from_bytes([7; 16]),
                "attachment",
                reference_blob,
                Digest::blob_content(&content),
                scope.digest()?,
                Metadata::empty(),
            )
            .map_err(|_| ArtifactError::Integrity {
                message: Arc::from("tampered reference invalid"),
            })?;
            Ok(finstack_ai_runtime::artifact::ArtifactRead { reference, content })
        })
    }
}

/// A wire reference whose declared digest is for `SAMPLE_CSV`.
fn sample_csv_blob() -> BlobRef {
    BlobRef::try_new(
        "blob-sample",
        "text/csv",
        u64::try_from(SAMPLE_CSV.len()).expect("length"),
        Some(Digest::blob_content(SAMPLE_CSV)),
        Some("revenue.csv"),
    )
    .expect("blob")
}

fn ingest_error_with(store: TamperedStore, blob: BlobRef) -> MiddlewareError {
    let middleware = DocumentIngestMiddleware::try_new(Arc::new(store)).expect("middleware");
    block_on(middleware.invoke(
        middleware_context(),
        before_model_input(vec![message(
            1,
            MessageRole::User,
            vec![text("please review the attachment"), file_block(blob)],
        )]),
    ))
    .expect_err("integrity failure must fail closed")
}

#[test]
fn digest_mismatch_fails_closed() {
    // The store resolves the blob and returns bytes, but their digest is not
    // the one the wire `BlobRef` declares (a stale or forged reference).
    // `fetch_blob` must reject the mismatch rather than feed
    // wrongly-attributed bytes to the parser, and the failure is an error,
    // never a fail-soft note.
    let error = ingest_error_with(
        TamperedStore {
            content: b"totally different content",
            reference_blob: None,
        },
        sample_csv_blob(),
    );
    assert_eq!(error.code(), DOCUMENT_INGEST_INTEGRITY_MISMATCH);
}

#[test]
fn store_fetch_failure_fails_closed() {
    let store = Arc::new(FailingGetStore);
    let (blob, _artifact) = indexed_artifact(1);
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let error = block_on(middleware.invoke(
        middleware_context(),
        before_model_input(vec![message(
            1,
            MessageRole::User,
            vec![text("please review the attachment"), file_block(blob)],
        )]),
    ))
    .expect_err("store failure must fail closed");
    assert_eq!(error.code(), DOCUMENT_INGEST_LOOKUP_FAILED);
}

#[test]
fn parser_failure_is_fail_soft_and_must_strip() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    let middleware = DocumentIngestMiddleware::try_new_with_limits(
        store,
        DocumentLimits {
            max_input_bytes: 1,
            ..DocumentLimits::default()
        },
    )
    .expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("fail-soft outcome is Ok");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace");
    };
    let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).expect("draft");
    assert!(all_text(&draft).contains("document_ingest:parse_failed"));
    assert!(!has_file_blocks(&draft));
}

#[test]
fn rebuild_message_invalid_blocks_uses_skip_note_without_files() {
    let (blob, _) = indexed_artifact(1);
    let original = message(
        1,
        MessageRole::User,
        vec![text("please review the attachment"), file_block(blob)],
    );
    let rebuilt = rebuild_message(&original, vec![tool_result_block()]).expect("skip note");
    assert!(!message_has_file_blocks(&rebuilt));
    assert_eq!(rebuilt.content().len(), 1);
    let ContentBlock::Text(note) = &rebuilt.content()[0] else {
        panic!("expected skip-note text");
    };
    assert_eq!(note.text(), FALLBACK_NOTE_TEXT);
}

#[test]
fn rebuild_message_construction_failure_has_no_file_blocks() {
    let (blob, _) = indexed_artifact(1);
    let original = tool_role_message();
    let error = rebuild_message(&original, vec![file_block(blob)]).expect_err("tool skip fails");
    construction_error_has_no_file_blocks(&error);
    assert!(
        error
            .to_string()
            .contains("document ingest message could not be constructed")
    );
}

#[test]
fn skip_note_message_construction_failure_has_no_file_blocks() {
    let error = skip_note_message(&tool_role_message()).expect_err("tool skip fails");
    construction_error_has_no_file_blocks(&error);
    assert!(
        error
            .to_string()
            .contains("document ingest message could not be constructed")
    );
}

#[test]
fn note_block_oversized_text_uses_fallback_without_files() {
    let oversized = "x".repeat(TEXT_MAX_BYTES + 1);
    let block = note_block(&oversized).expect("fallback note");
    let ContentBlock::Text(note) = block else {
        panic!("expected text fallback, not a File block");
    };
    assert_eq!(note.text(), FALLBACK_NOTE_TEXT);
}

#[test]
fn fallback_text_block_constructs_static_note() {
    let note = fallback_text_block().expect("static note");
    assert_eq!(note.text(), FALLBACK_NOTE_TEXT);
}

#[test]
fn repeated_invocations_are_byte_identical() {
    // Multi-cycle runs hit BeforeModel repeatedly with the same attachment;
    // the parse memo must keep every invocation's Replace JSON
    // byte-identical to the first (cached note == recomputed note).
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let mut outputs = Vec::new();
    for _ in 0..3 {
        let outcome = block_on(middleware.invoke(
            middleware_context(),
            before_model_input_with_file(&artifact),
        ))
        .expect("outcome");
        let StageOutcome::Replace(json) = outcome else {
            panic!("expected Replace");
        };
        outputs.push(json.as_bytes().to_vec());
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[1], outputs[2]);
}

#[test]
fn mismatched_declared_name_fails_reference_validation() {
    // Two File blocks carrying the same bytes under different declared
    // names must each get a note carrying their own name — the memo key
    // includes the name, so a cached "a.csv" note must never surface for
    // "b.csv".
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "a.csv");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");

    let renamed_blob = BlobRef::try_new(
        artifact.blob().id(),
        "text/csv",
        artifact.blob().length(),
        artifact.blob().digest().copied(),
        Some("b.csv"),
    )
    .expect("blob");
    let input = before_model_input(vec![message(
        1,
        MessageRole::User,
        vec![
            text("please review the attachment"),
            file_block(renamed_blob),
        ],
    )]);
    block_on(middleware.invoke(middleware_context(), input))
        .expect_err("renamed reference must fail closed");
}

fn ingest_with_limits(limits: DocumentLimits) -> DocumentIngestMiddleware {
    DocumentIngestMiddleware::try_new_with_limits(Arc::new(CaptureArtifactStore::default()), limits)
        .expect("middleware")
}

fn configuration_digest_of(limits: DocumentLimits) -> Digest {
    ingest_with_limits(limits)
        .descriptor()
        .invocation
        .configuration_digest
}

#[test]
fn configuration_digest_differs_for_different_limits() {
    let base = DocumentLimits::default();
    let larger_input = DocumentLimits {
        max_input_bytes: base.max_input_bytes + 1,
        ..base.clone()
    };
    let larger_output = DocumentLimits {
        max_output_bytes: base.max_output_bytes + 1,
        ..base.clone()
    };
    let more_pages = DocumentLimits {
        max_pages: base.max_pages + 1,
        ..base.clone()
    };
    let base_digest = configuration_digest_of(base);
    let input_digest = configuration_digest_of(larger_input);
    let output_digest = configuration_digest_of(larger_output);
    let pages_digest = configuration_digest_of(more_pages);
    assert_ne!(base_digest, input_digest);
    assert_ne!(base_digest, output_digest);
    assert_ne!(base_digest, pages_digest);
    assert_ne!(input_digest, output_digest);
    assert_ne!(input_digest, pages_digest);
    assert_ne!(output_digest, pages_digest);
}

#[test]
fn try_new_digest_matches_equivalent_explicit_store_limits() {
    let max_artifact_bytes = 64 * 1024 * 1024;
    let store: Arc<dyn ArtifactStore> =
        Arc::new(InProcessArtifactStore::default().with_max_artifact_bytes(max_artifact_bytes));
    let derived = DocumentIngestMiddleware::try_new(Arc::clone(&store)).expect("try_new");
    let explicit = DocumentIngestMiddleware::try_new_with_limits(
        store,
        DocumentLimits {
            max_input_bytes: u64::try_from(max_artifact_bytes).expect("fits u64"),
            ..DocumentLimits::default()
        },
    )
    .expect("try_with_limits");
    assert_eq!(
        derived.descriptor().invocation.configuration_digest,
        explicit.descriptor().invocation.configuration_digest,
    );
}

#[test]
fn configuration_identity_pins_canonical_bytes_and_version_tag() {
    assert_eq!(LIMITS_IDENTITY_VERSION, "document-ingest-limits-v1");
    let defaults = DocumentLimits::default();
    let bytes = limits_identity_bytes(&defaults).expect("encode defaults");
    assert_eq!(bytes, PINNED_DEFAULT_LIMITS_IDENTITY);
    assert!(
        bytes
            .windows(LIMITS_IDENTITY_VERSION.len())
            .any(|window| window == LIMITS_IDENTITY_VERSION.as_bytes()),
        "canonical identity must contain the version tag"
    );

    let middleware = DocumentIngestMiddleware::try_new_with_limits(
        Arc::new(CaptureArtifactStore::default()),
        defaults,
    )
    .expect("middleware");
    let digest = middleware.descriptor().invocation.configuration_digest;
    assert_eq!(digest, Digest::raw_json(PINNED_DEFAULT_LIMITS_IDENTITY));
    assert_eq!(
        digest.to_hex(),
        "cc84a3bd94ea6400617b7b2309c0e3c876fd9cf642859137206e0df716b2c2b0"
    );
    assert_ne!(digest, Digest::raw_json(b"document-ingest-v1"));
    assert_ne!(PINNED_DEFAULT_LIMITS_IDENTITY, b"document-ingest-v1");
}

#[test]
fn ingest_limit_follows_the_store_ceiling() {
    let store: Arc<dyn ArtifactStore> =
        Arc::new(InProcessArtifactStore::default().with_max_artifact_bytes(64 * 1024 * 1024));
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    assert_eq!(middleware.limits.max_input_bytes, 64 * 1024 * 1024);
}

#[test]
fn parse_cache_fifo_evicts_oldest_entry_at_capacity() {
    let cache = ParseCache::default();
    for ordinal in 0..=PARSE_CACHE_CAPACITY {
        cache.insert(parse_key(ordinal), format!("note-{ordinal}"));
    }
    assert!(
        cache.lookup(&parse_key(0)).is_none(),
        "oldest parse-cache entry must be evicted"
    );
    assert_eq!(
        cache.lookup(&parse_key(PARSE_CACHE_CAPACITY)).as_deref(),
        Some(format!("note-{PARSE_CACHE_CAPACITY}").as_str())
    );
}

#[test]
fn parse_cache_reinsert_refreshes_value_without_changing_fifo_order() {
    let cache = ParseCache::default();
    for ordinal in 0..PARSE_CACHE_CAPACITY {
        cache.insert(parse_key(ordinal), format!("note-{ordinal}"));
    }
    cache.insert(parse_key(0), "refreshed".to_owned());
    assert_eq!(cache.lookup(&parse_key(0)).as_deref(), Some("refreshed"));

    cache.insert(parse_key(PARSE_CACHE_CAPACITY), "overflow".to_owned());
    assert!(
        cache.lookup(&parse_key(0)).is_none(),
        "reinsert must not move the oldest parse-cache key"
    );
    assert_eq!(
        cache.lookup(&parse_key(1)).as_deref(),
        Some("note-1"),
        "the next-oldest parse-cache key must survive overflow"
    );
    assert_eq!(
        cache.lookup(&parse_key(PARSE_CACHE_CAPACITY)).as_deref(),
        Some("overflow")
    );
}

#[test]
fn parse_cache_recovers_from_poison() {
    let cache = ParseCache::default();
    cache.insert(parse_key(0), "kept".to_owned());
    cache.poison();
    assert_eq!(cache.lookup(&parse_key(0)).as_deref(), Some("kept"));
    cache.insert(parse_key(1), "added".to_owned());
    assert_eq!(cache.lookup(&parse_key(1)).as_deref(), Some("added"));
}

#[test]
fn cached_parse_keeps_each_exact_uploaded_blob_reference() {
    let store = Arc::new(CaptureArtifactStore::default());
    let first = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "first.csv");
    let second = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "second.csv");
    let middleware = DocumentIngestMiddleware::try_new(store).unwrap();
    for artifact in [&first, &second, &first] {
        let outcome = block_on(
            middleware.invoke(middleware_context(), before_model_input_with_file(artifact)),
        )
        .unwrap();
        let StageOutcome::Replace(json) = outcome else {
            panic!("expected replacement");
        };
        let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).unwrap();
        let text = all_text(&draft);
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix("Document source reference: "))
            .unwrap();
        let reference: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            reference["attachment"],
            serde_json::to_value(blob_of(artifact)).unwrap()
        );
        assert!(text.contains("Q1"));
    }
}

#[test]
fn cache_hit_skips_artifact_fetch() {
    // Turn one resolves, verifies, and parses the attachment (one store
    // read). Turn two carries the same wire reference under the same tenant,
    // so the memo answers without touching the store at all.
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    let middleware =
        DocumentIngestMiddleware::try_new(Arc::clone(&store) as Arc<dyn ArtifactStore>)
            .expect("middleware");

    let first = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("first turn");
    assert_eq!(store.blob_reads(), 1, "first turn must fetch the blob");

    let second = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("second turn");
    assert_eq!(
        store.blob_reads(),
        1,
        "second turn with the same attachment must perform zero store reads"
    );

    let (StageOutcome::Replace(first), StageOutcome::Replace(second)) = (first, second) else {
        panic!("both turns must replace the draft");
    };
    assert_eq!(first.as_bytes(), second.as_bytes());
}

#[test]
fn cache_is_tenant_scoped() {
    // A hit must never cross tenants: the same wire reference presented by
    // another tenant goes back through the store (and fails there, since the
    // artifact lives in tenant-a's scope).
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    let middleware =
        DocumentIngestMiddleware::try_new(Arc::clone(&store) as Arc<dyn ArtifactStore>)
            .expect("middleware");
    block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("tenant-a turn");
    assert_eq!(store.blob_reads(), 1);

    let mut other_tenant = middleware_context();
    other_tenant.run.locator = finstack_ai_kernel::OperationLocator::try_new(
        "tenant-b",
        other_tenant.run.locator.session_id,
        other_tenant.run.locator.lane_id,
        other_tenant.run.locator.run_id,
    )
    .expect("locator");
    let error = block_on(middleware.invoke(other_tenant, before_model_input_with_file(&artifact)))
        .expect_err("another tenant must not be served from the memo");
    assert_eq!(error.code(), DOCUMENT_INGEST_LOOKUP_FAILED);
    assert_eq!(store.blob_reads(), 2, "the other tenant must hit the store");
}

#[test]
fn lookup_failure_uses_lookup_code() {
    let store = Arc::new(FailingGetStore);
    let (blob, _artifact) = indexed_artifact(1);
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let error = block_on(middleware.invoke(
        middleware_context(),
        before_model_input(vec![message(
            1,
            MessageRole::User,
            vec![text("please review the attachment"), file_block(blob)],
        )]),
    ))
    .expect_err("store failure must fail closed");
    assert_eq!(error.code(), DOCUMENT_INGEST_LOOKUP_FAILED);
    assert_ne!(error.code(), MIDDLEWARE_OUTCOME_NOT_ALLOWED);
    assert_eq!(
        error.descriptor().category,
        finstack_ai_kernel::ErrorCategory::Middleware
    );
}

#[test]
fn digest_mismatch_uses_integrity_code() {
    let error = ingest_error_with(
        TamperedStore {
            content: b"totally different content",
            reference_blob: None,
        },
        sample_csv_blob(),
    );
    assert_eq!(error.code(), DOCUMENT_INGEST_INTEGRITY_MISMATCH);
    assert_ne!(error.code(), DOCUMENT_INGEST_LOOKUP_FAILED);
    assert_ne!(error.code(), MIDDLEWARE_OUTCOME_NOT_ALLOWED);
}

#[test]
fn reference_mismatch_uses_integrity_code() {
    // Content digest is fine, but the store-owned reference names a
    // different blob than the wire reference: still an integrity failure,
    // not a lookup failure.
    let wire = sample_csv_blob();
    let other = BlobRef::try_new(
        "blob-other",
        "text/csv",
        wire.length(),
        wire.digest().copied(),
        Some("revenue.csv"),
    )
    .expect("blob");
    let error = ingest_error_with(
        TamperedStore {
            content: SAMPLE_CSV,
            reference_blob: Some(other),
        },
        wire,
    );
    assert_eq!(error.code(), DOCUMENT_INGEST_INTEGRITY_MISMATCH);
}
