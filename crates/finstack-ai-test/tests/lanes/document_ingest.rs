// Attach → auto-ingest → model-visible Markdown lane.
//
// Exercises the full public path: stage an attachment into a real
// `ArtifactStore`, record it in the shared `AttachmentIndex`, run an agent
// with `DocumentIngestMiddleware` and `DocumentToolset` registered, and
// assert that (a) the run completes, (b) the model only ever sees the
// extracted Markdown (no `File` block), and (c) the canonical journaled
// conversation still carries the original `File` block.

use finstack_ai::{AgentRunRequest, AttachmentInput};
use finstack_ai_context_memory::InProcessArtifactStore;
use finstack_ai_kernel::{EntryBody, Sensitivity};
use finstack_ai_middleware_document_ingest::{AttachmentIndex, DocumentIngestMiddleware};
use finstack_ai_runtime::{ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes};
use finstack_ai_tools_document::DocumentToolset;

const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");

const COMPONENT_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Any consistent scope works for staging: [`InProcessArtifactStore::get`]
/// resolves purely by content-derived `ArtifactId`, ignoring the scope
/// passed to `get`. The middleware resolves the `BlobRef` back to this
/// exact `ArtifactRef` via the shared `AttachmentIndex` (spec decision 19),
/// not via scope matching.
fn staging_scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("tenant-lanes"),
        session_id: id(9_000),
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

async fn document_ingest_agent() -> (
    Agent,
    Arc<dyn finstack_ai_runtime::JournalStore>,
    Arc<ScriptedModel>,
    Arc<InProcessArtifactStore>,
    Arc<AttachmentIndex>,
) {
    use finstack_ai_kernel::{AgentId, BundleId};

    let store: Arc<dyn finstack_ai_runtime::JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("journal store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        scripted_profile(),
        vec![completed_plan("acknowledged")],
    ));
    let artifact_store = Arc::new(InProcessArtifactStore::default());
    let attachment_index = Arc::new(AttachmentIndex::default());

    let middleware = Arc::new(
        DocumentIngestMiddleware::try_new(
            Arc::clone(&artifact_store) as Arc<dyn ArtifactStore>,
            Arc::clone(&attachment_index),
        )
        .expect("document ingest middleware"),
    );
    let toolset = Arc::new(
        DocumentToolset::try_new()
            .expect("document toolset")
            .with_artifact_store(Arc::clone(&artifact_store) as Arc<dyn ArtifactStore>),
    );

    let agent = Agent::builder(
        AgentId::parse("test.agent.document-ingest").expect("agent id"),
        BundleId::parse("test.bundle.document-ingest").expect("bundle id"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.document-ingest").expect("model id"),
                Some(COMPONENT_VERSION),
            ),
            Arc::clone(&model) as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.document-ingest").expect("store id"),
                Some(COMPONENT_VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .toolset(
        ComponentRef::new(
            ComponentId::parse("finstack.tools.document").expect("toolset id"),
            Some(COMPONENT_VERSION),
        ),
        toolset,
    )
    .middleware(
        ComponentRef::new(
            ComponentId::parse("finstack.middleware.document-ingest").expect("middleware id"),
            Some(COMPONENT_VERSION),
        ),
        middleware,
    )
    .policy(RunPolicy::default())
    .build()
    .await
    .expect("agent");

    (agent, store, model, artifact_store, attachment_index)
}

#[tokio::test]
async fn document_ingest_lane_delivers_markdown_to_model_and_keeps_journaled_file_block() {
    let (agent, store, model, artifact_store, attachment_index) = document_ingest_agent().await;

    // Stage the fixture into the real artifact store and record it in the
    // shared index, exactly as the run/session attachment path would.
    let artifact = artifact_store
        .stage_put(
            staging_scope(),
            Bytes::copy_from_slice(SAMPLE_CSV),
            ArtifactMetadata {
                kind: Arc::from("attachment"),
                media_type: Arc::from("text/csv"),
                name: Some(Arc::from("sample.csv")),
                attributes: Metadata::empty(),
            },
        )
        .await
        .expect("staged artifact");
    attachment_index.insert(artifact.clone());

    let mut request = AgentRunRequest::try_new(
        finstack_ai_runtime::ModelName::try_new("lanes-1").expect("model name"),
        "Please summarize the attached document",
        security("decision-v1"),
    )
    .expect("request");
    request.attachments = Arc::from([AttachmentInput {
        artifact: artifact.clone(),
    }]);

    // (a) the run completes.
    let output = agent.run(request).await.expect("run completes");
    assert_eq!(output.text(), "acknowledged");

    // (b) the model request captured by the scripted model contains the
    // extracted Markdown and carries no File block at all.
    let sent_request = model.last_request().expect("model was called");
    let user_blocks: Vec<&ContentBlock> = sent_request
        .draft
        .messages
        .iter()
        .filter(|message| message.role() == finstack_ai_kernel::MessageRole::User)
        .flat_map(finstack_ai_kernel::Message::content)
        .collect();
    let model_visible_text: String = user_blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect();
    assert!(
        model_visible_text.contains("Q1"),
        "model-visible text must contain extracted Markdown: {model_visible_text:?}"
    );
    assert!(
        !sent_request
            .draft
            .messages
            .iter()
            .flat_map(finstack_ai_kernel::Message::content)
            .any(|block| matches!(block, ContentBlock::File(_))),
        "model must never see a File block"
    );

    // (c) the canonical journaled conversation still carries the original
    // user message with its File block, unmodified by the middleware.
    let session = finstack_ai_runtime::SessionRuntime::open(
        Arc::clone(&store),
        output.locator.session_id,
        Arc::clone(&output.locator.tenant_scope),
    )
    .await
    .expect("open session");
    let inspected = session
        .inspect(output.locator.lane_id)
        .await
        .expect("inspect lane");
    let journaled_file_blocks: usize = inspected
        .history
        .iter()
        .filter_map(|entry| {
            let EntryBody::Message(message) = entry.body();
            (message.role() == finstack_ai_kernel::MessageRole::User).then_some(message)
        })
        .flat_map(finstack_ai_kernel::Message::content)
        .filter(|block| matches!(block, ContentBlock::File(_)))
        .count();
    assert_eq!(
        journaled_file_blocks, 1,
        "journaled user message must still carry the original File block"
    );
}
