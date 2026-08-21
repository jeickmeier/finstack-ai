//! Crate-local fixtures shared by unit tests and the ingest benchmark.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{
    ArtifactId, ArtifactRef, BlobRef, ContentBlock, Digest, Id, IdTag, LaneId, MediaRef, Message,
    MessageRole, Metadata, OperationLocator, OutputSpec, PrincipalRef, ProviderIds, RawJson, RunId,
    SessionId, TextBlock, Timestamp,
};
use finstack_ai_runtime::{
    ArtifactError, ArtifactMetadata, ArtifactRead, ArtifactScope, ArtifactStore,
    AuthorizationContext, BeforeModelInput, Bytes, CancellationSignal, ModelName,
    ModelRequestDraft, ModelRequestLimits, ModelSettings, PortFuture, RunCallContext, StageInput,
};

pub(crate) const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");

pub(crate) fn block_on<T>(future: impl Future<Output = T>) -> T {
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
/// and serves `get` by content-digest match.
#[derive(Clone, Default)]
pub(crate) struct CaptureArtifactStore {
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

    fn get_by_blob(
        &self,
        scope: ArtifactScope,
        blob: BlobRef,
    ) -> PortFuture<Result<ArtifactRead, ArtifactError>> {
        let staged = Arc::clone(&self.staged);
        Box::pin(async move {
            let staged = staged.lock().expect("capture lock");
            let (_, content, metadata) = staged
                .iter()
                .find(|(stored_scope, content, metadata)| {
                    stored_scope == &scope
                        && blob.digest().copied() == Some(Digest::blob_content(content))
                        && blob.media_type() == metadata.media_type.as_ref()
                        && blob.name() == metadata.name.as_deref()
                })
                .ok_or(ArtifactError::NotFound)?;
            let reference = ArtifactRef::try_new(
                ArtifactId::from_bytes([9; 16]),
                metadata.kind.as_ref(),
                blob,
                Digest::blob_content(content),
                scope.digest()?,
                metadata.attributes.clone(),
            )
            .map_err(|_| ArtifactError::Integrity {
                message: Arc::from("capture reference invalid"),
            })?;
            Ok(ArtifactRead {
                reference,
                content: content.clone(),
            })
        })
    }
}

fn test_scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: SessionId::from_bytes([0; 16]),
        run_id: None,
        sensitivity: finstack_ai_kernel::Sensitivity::Internal,
    }
}

/// Stage `bytes` into `store` and return the exact `ArtifactRef` it
/// produced.
pub(crate) fn stage(
    store: &dyn ArtifactStore,
    bytes: &[u8],
    media: &str,
    name: &str,
) -> ArtifactRef {
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
pub(crate) fn blob_of(artifact: &ArtifactRef) -> BlobRef {
    artifact.blob().clone()
}

pub(crate) fn middleware_context() -> finstack_ai_runtime::MiddlewareContext {
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

pub(crate) fn message(ordinal: u64, role: MessageRole, content: Vec<ContentBlock>) -> Message {
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

pub(crate) fn text(value: &str) -> ContentBlock {
    ContentBlock::Text(TextBlock::try_new(value).expect("text"))
}

pub(crate) fn file_block(blob: BlobRef) -> ContentBlock {
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

pub(crate) fn before_model_input(messages: Vec<Message>) -> StageInput {
    StageInput::BeforeModel(Box::new(BeforeModelInput {
        request: draft_with_messages(messages),
        source_entries: Arc::from([]),
        model_context_profile_digest: Digest::raw_json(b"profile"),
        hard_input_tokens: 10_000,
        checkpoint: None,
    }))
}

pub(crate) fn before_model_input_with_file(artifact: &ArtifactRef) -> StageInput {
    before_model_input(vec![message(
        1,
        MessageRole::User,
        vec![
            text("please review the attachment"),
            file_block(blob_of(artifact)),
        ],
    )])
}
