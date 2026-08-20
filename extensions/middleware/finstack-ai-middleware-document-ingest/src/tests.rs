use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    ArtifactId, ArtifactRef, BlobRef, ContentBlock, Digest, Id, IdTag, LaneId, MediaRef, Message,
    MessageRole, Metadata, OperationLocator, OutputSpec, PrincipalRef, ProviderIds, RawJson, RunId,
    SessionId, TextBlock, Timestamp,
};
use finstack_ai_runtime::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, AuthorizationContext,
    BeforeModelInput, Bytes, CancellationSignal, Middleware as _, ModelName, ModelRequestDraft,
    ModelRequestLimits, ModelSettings, PortFuture, RunCallContext, StageInput, StageOutcome,
};

use finstack_ai_memory::InProcessArtifactStore;

use crate::{AttachmentIndex, DocumentIngestMiddleware};

const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");
const SCANNED_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/scanned.pdf");

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

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

/// Test-only in-memory `ArtifactStore` that captures every `stage_put` call
/// and serves `get` by content-digest match. Copied from
/// `extensions/toolsets/finstack-ai-tools-document/src/tests.rs`.
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

fn test_scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: Some(RunId::from_bytes([3; 16])),
        sensitivity: finstack_ai_kernel::Sensitivity::Internal,
    }
}

/// Stage `bytes` into `store` and return the exact `ArtifactRef` it
/// produced. Callers must separately `AttachmentIndex::insert` this ref for
/// the middleware to be able to resolve the corresponding `BlobRef` (spec
/// decision 19) — the dangling-file test deliberately skips that step.
fn stage(store: &dyn ArtifactStore, bytes: &[u8], media: &str, name: &str) -> ArtifactRef {
    block_on(finstack_ai_runtime::stage_required_artifact(
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

/// The exact `BlobRef` carried on the wire for a staged artifact.
fn blob_of(artifact: &ArtifactRef) -> BlobRef {
    artifact.blob().clone()
}

fn middleware_context() -> finstack_ai_runtime::MiddlewareContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    finstack_ai_runtime::MiddlewareContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal,
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: id::<finstack_ai_kernel::EffectTag>(4),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
            relation_depth: 0,
        },
        chain_digest: Digest::raw_json(b"chain"),
        chain_index: 0,
        compaction_resume: None,
    }
}

fn message(ordinal: u64, role: MessageRole, content: Vec<ContentBlock>) -> Message {
    Message::try_new(
        id(ordinal),
        role,
        content,
        Timestamp::from_unix_ms(i64::try_from(ordinal).expect("ts")).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn text(value: &str) -> ContentBlock {
    ContentBlock::Text(TextBlock::try_new(value).expect("text"))
}

fn file_block(blob: BlobRef) -> ContentBlock {
    ContentBlock::File(MediaRef::new(blob))
}

fn draft_with_messages(messages: Vec<Message>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: ModelName::try_new("preview-model").expect("model"),
        messages: messages.into(),
        tools: Arc::from([]),
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 1_000_000,
            max_input_tokens: 10_000,
            max_output_tokens: 1_000,
        },
    }
}

fn before_model_input(messages: Vec<Message>) -> StageInput {
    StageInput::BeforeModel(Box::new(BeforeModelInput {
        request: draft_with_messages(messages),
        source_entries: Arc::from([]),
        model_context_profile_digest: Digest::raw_json(b"profile"),
        hard_input_tokens: 10_000,
        checkpoint: None,
    }))
}

fn before_model_input_with_file(artifact: &ArtifactRef) -> StageInput {
    before_model_input(vec![message(
        1,
        MessageRole::User,
        vec![
            text("please review the attachment"),
            file_block(blob_of(artifact)),
        ],
    )])
}

fn before_model_input_with_dangling_file() -> StageInput {
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

fn before_model_input_text_only() -> StageInput {
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
    draft.messages.iter().any(|message| {
        message
            .content()
            .iter()
            .any(|block| matches!(block, ContentBlock::File(_)))
    })
}

#[test]
fn descriptor_declares_before_model_context_mutation() {
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
    let descriptor = middleware.descriptor();
    assert!(
        descriptor
            .stages
            .contains(finstack_ai_kernel::Stage::BeforeModel)
    );
    assert_eq!(
        descriptor.order.tier,
        finstack_ai_runtime::OrderTier::ContextMutation
    );
}

#[test]
fn replaces_supported_file_block_with_markdown_text() {
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    index.insert(artifact.clone());
    let middleware =
        DocumentIngestMiddleware::try_new(store, Arc::clone(&index)).expect("middleware");
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
    let index = Arc::new(AttachmentIndex::default());
    let artifact = stage(store.as_ref(), SCANNED_PDF, "application/pdf", "scan.pdf");
    index.insert(artifact.clone());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
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
fn unresolvable_artifact_is_fail_soft_note() {
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    // Blob references an artifact that was never staged into the index.
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_dangling_file(),
    ))
    .expect("fail-soft outcome is Ok");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace");
    };
    let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).expect("draft");
    assert!(all_text(&draft).contains("could not be read"));
}

#[test]
fn no_file_blocks_means_continue() {
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
    let outcome = block_on(middleware.invoke(middleware_context(), before_model_input_text_only()))
        .expect("outcome");
    assert!(matches!(outcome, StageOutcome::Continue));
}

#[test]
fn unsupported_media_type_file_block_is_left_alone() {
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let artifact = stage(store.as_ref(), b"\x89PNG\r\n", "image/png", "chart.png");
    index.insert(artifact.clone());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("outcome");
    assert!(matches!(outcome, StageOutcome::Continue));
}

#[test]
fn non_user_role_message_is_left_untouched() {
    // Assistant messages may legally carry File blocks (per
    // `validate_role_blocks`), but spec decision 14 scopes rewriting to
    // User-role messages only. An Assistant-role File block must survive
    // unchanged and the middleware must report no change (`Continue`),
    // proving it never even inspects non-User content.
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    index.insert(artifact.clone());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
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

#[test]
fn digest_mismatch_is_fail_soft_note() {
    // The staged artifact resolves fine (index hit, store returns bytes),
    // but the wire `BlobRef` on the message's `ContentBlock::File` declares
    // a digest that does not match the actual stored content — e.g. a
    // stale/forged reference. `fetch_blob` must reject the mismatch and
    // the middleware must fall back to the same "could not be read"
    // fail-soft note used for an unresolvable artifact, rather than
    // feeding wrongly-attributed bytes to the parser.
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    index.insert(artifact.clone());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");

    // Same blob id as the staged artifact (so the index lookup and store
    // fetch both succeed), but a digest that belongs to different content.
    let wrong_digest = Digest::blob_content(b"totally different content");
    let mismatched_blob = BlobRef::try_new(
        artifact.blob().id(),
        "text/csv",
        artifact.blob().length(),
        Some(wrong_digest),
        Some("revenue.csv"),
    )
    .expect("blob");
    let input = before_model_input(vec![message(
        1,
        MessageRole::User,
        vec![
            text("please review the attachment"),
            file_block(mismatched_blob),
        ],
    )]);

    let outcome =
        block_on(middleware.invoke(middleware_context(), input)).expect("fail-soft outcome is Ok");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace");
    };
    let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).expect("draft");
    assert!(all_text(&draft).contains("could not be read"));
    assert!(!has_file_blocks(&draft));
}

#[test]
fn repeated_invocations_are_byte_identical() {
    // Multi-cycle runs hit BeforeModel repeatedly with the same attachment;
    // the parse memo must keep every invocation's Replace JSON
    // byte-identical to the first (cached note == recomputed note).
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    index.insert(artifact.clone());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
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
fn parse_memo_keys_on_declared_name() {
    // Two File blocks carrying the same bytes under different declared
    // names must each get a note carrying their own name — the memo key
    // includes the name, so a cached "a.csv" note must never surface for
    // "b.csv".
    let store = Arc::new(CaptureArtifactStore::default());
    let index = Arc::new(AttachmentIndex::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "a.csv");
    index.insert(artifact.clone());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");

    let renamed_blob = BlobRef::try_new(
        artifact.blob().id(),
        "text/csv",
        artifact.blob().length(),
        artifact.blob().digest().copied(),
        Some("b.csv"),
    )
    .expect("blob");
    for (blob, expected_name, absent_name) in [
        (blob_of(&artifact), "a.csv", "b.csv"),
        (renamed_blob, "b.csv", "a.csv"),
    ] {
        let input = before_model_input(vec![message(
            1,
            MessageRole::User,
            vec![text("please review the attachment"), file_block(blob)],
        )]);
        let outcome = block_on(middleware.invoke(middleware_context(), input)).expect("outcome");
        let StageOutcome::Replace(json) = outcome else {
            panic!("expected Replace");
        };
        let draft: ModelRequestDraft = serde_json::from_slice(json.as_bytes()).expect("draft");
        let all = all_text(&draft);
        assert!(all.contains(expected_name), "note must carry its own name");
        assert!(!all.contains(absent_name), "cached note must not leak");
    }
}

#[test]
fn ingest_limit_follows_the_store_ceiling() {
    let store: Arc<dyn ArtifactStore> =
        Arc::new(InProcessArtifactStore::default().with_max_artifact_bytes(64 * 1024 * 1024));
    let index = Arc::new(AttachmentIndex::default());
    let middleware = DocumentIngestMiddleware::try_new(store, index).expect("middleware");
    assert_eq!(middleware.limits().max_input_bytes, 64 * 1024 * 1024);
}

#[test]
fn attachment_index_fifo_evicts_oldest_entry_at_capacity() {
    let index = AttachmentIndex::default();
    let mut last = None;
    for ordinal in 0..1025_u16 {
        let blob = BlobRef::try_new(
            format!("blob-{ordinal}"),
            "text/csv",
            1,
            Some(Digest::blob_content(&ordinal.to_be_bytes())),
            None::<&str>,
        )
        .expect("blob");
        let artifact = ArtifactRef::try_new(
            ArtifactId::from_bytes([1; 16]),
            "attachment",
            blob.clone(),
            Digest::blob_content(&ordinal.to_be_bytes()),
            Digest::raw_json(b"scope"),
            Metadata::empty(),
        )
        .expect("artifact");
        index.insert(artifact);
        last = Some(blob);
    }
    let first_blob = BlobRef::try_new("blob-0", "text/csv", 1, None, None::<&str>).expect("blob");
    assert!(
        index.lookup(&first_blob).is_none(),
        "oldest entry must be evicted"
    );
    let last_blob = last.expect("at least one insert");
    assert!(
        index.lookup(&last_blob).is_some(),
        "most recent entry must remain"
    );
}
