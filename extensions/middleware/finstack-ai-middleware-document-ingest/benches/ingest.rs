//! Document-ingest middleware benchmarks: the `BeforeModel` invoke path.
//!
//! Measures one full invocation with a single supported `File` block (small
//! CSV and a synthetic ~256 KiB CSV), and the no-attachment `Continue` path.
//! The repeated `bencher.iter` invocations model a multi-cycle run hitting
//! `BeforeModel` once per cycle with the same attachment.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_kernel::{
    ArtifactId, ArtifactRef, BlobRef, ContentBlock, Digest, Id, IdTag, LaneId, MediaRef, Message,
    MessageRole, Metadata, OperationLocator, OutputSpec, PrincipalRef, ProviderIds, RawJson, RunId,
    SessionId, TextBlock, Timestamp,
};
use finstack_ai_middleware_document_ingest::{AttachmentIndex, DocumentIngestMiddleware};
use finstack_ai_runtime::{
    ArtifactError, ArtifactMetadata, ArtifactScope, ArtifactStore, AuthorizationContext,
    BeforeModelInput, Bytes, CancellationSignal, Middleware as _, ModelName, ModelRequestDraft,
    ModelRequestLimits, ModelSettings, PortFuture, RunCallContext, StageInput, StageOutcome,
};

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

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
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

fn before_model_input(messages: Vec<Message>) -> StageInput {
    StageInput::BeforeModel(Box::new(BeforeModelInput {
        request: ModelRequestDraft {
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
        },
        source_entries: Arc::from([]),
        model_context_profile_digest: Digest::raw_json(b"profile"),
        hard_input_tokens: 10_000,
        checkpoint: None,
    }))
}

fn input_with_file(artifact: &ArtifactRef) -> StageInput {
    before_model_input(vec![message(
        1,
        MessageRole::User,
        vec![
            text("please review the attachment"),
            ContentBlock::File(MediaRef::new(artifact.blob().clone())),
        ],
    )])
}

fn invoke_replaces(middleware: &DocumentIngestMiddleware, input: StageInput) {
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
        bencher.iter(|| invoke_replaces(&middleware, input_with_file(&csv_artifact)));
    });
    criterion.bench_function("middleware_invoke_large_csv_file_block", |bencher| {
        bencher.iter(|| invoke_replaces(&middleware, input_with_file(&big_artifact)));
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
