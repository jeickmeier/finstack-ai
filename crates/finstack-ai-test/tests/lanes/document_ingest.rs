// Attach → auto-ingest → model-visible Markdown lane.
//
// Exercises the full public path: stage an attachment into a real
// `ArtifactStore`, run an agent
// with `DocumentIngestMiddleware` and `DocumentToolset` registered, and
// assert that (a) the run completes, (b) the model only ever sees the
// extracted Markdown (no `File` block), and (c) the canonical journaled
// conversation still carries the original `File` block.

use finstack_ai::{AgentRunRequest, AttachmentInput};
use finstack_ai_kernel::{EntryBody, Sensitivity};
use finstack_ai_memory::store::InProcessArtifactStore;
use finstack_ai_middleware_document_ingest::DocumentIngestMiddleware;
use finstack_ai_runtime::artifact::{ArtifactMetadata, ArtifactScope, ArtifactStore};
use finstack_ai_runtime::{Bytes};
use finstack_ai_tools_document::DocumentToolset;

const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");

const COMPONENT_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Canonical tenant-bound pre-run upload scope used by the bindings and
/// document-ingest middleware before session/run ids exist.
fn staging_scope() -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: finstack_ai_kernel::SessionId::from_bytes([0_u8; 16]),
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

/// Shared scaffolding for every document-ingest lane test: builds a fresh
/// journal store, scripted model, attachment index, and an `Agent` wired
/// with `DocumentIngestMiddleware` + `DocumentToolset` over the caller's
/// `artifact_store`. `label` disambiguates component/agent ids across tests
/// that call this more than once in the same binary.
async fn document_ingest_agent_with_store(
    label: &str,
    artifact_store: Arc<dyn ArtifactStore>,
) -> (
    Agent,
    Arc<dyn finstack_ai_runtime::ports::journal::JournalStore>,
    Arc<ScriptedModel>,
) {
    use finstack_ai_kernel::{AgentId, BundleId};

    let store: Arc<dyn finstack_ai_runtime::ports::journal::JournalStore> = Arc::new(
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

    let middleware = Arc::new(
        DocumentIngestMiddleware::try_new(Arc::clone(&artifact_store))
            .expect("document ingest middleware"),
    );
    let toolset =
        Arc::new(DocumentToolset::try_new(Arc::clone(&artifact_store)).expect("document toolset"));

    let agent = Agent::builder(
        AgentId::parse(format!("test.agent.document-ingest-{label}")).expect("agent id"),
        BundleId::parse(format!("test.bundle.document-ingest-{label}")).expect("bundle id"),
        (
            ComponentRef::new(
                ComponentId::parse(format!("test.model.document-ingest-{label}"))
                    .expect("model id"),
                Some(COMPONENT_VERSION),
            ),
            Arc::clone(&model) as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse(format!("test.store.document-ingest-{label}"))
                    .expect("store id"),
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

    (agent, store, model)
}

async fn document_ingest_agent() -> (
    Agent,
    Arc<dyn finstack_ai_runtime::ports::journal::JournalStore>,
    Arc<ScriptedModel>,
    Arc<dyn ArtifactStore>,
) {
    let artifact_store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    let (agent, store, model) =
        document_ingest_agent_with_store("csv", Arc::clone(&artifact_store)).await;
    (agent, store, model, artifact_store)
}

#[tokio::test]
async fn document_ingest_lane_delivers_markdown_to_model_and_keeps_journaled_file_block() {
    let (agent, store, model, artifact_store) = document_ingest_agent().await;

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

    let mut request = AgentRunRequest::try_new(
        finstack_ai_runtime::ports::model::ModelName::try_new("lanes-1").expect("model name"),
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
    let session = finstack_ai_runtime::session::SessionRuntime::open(
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

/// A >4 MiB attachment must stage and flow through the full lane when the
/// artifact store is backed by [`ObjectArtifactStore`] (default 64 MiB
/// ceiling), not the small in-process default. This is the regression the
/// object-store integration lane exists to catch: any consumer path still
/// pinning the old 4 MiB assumption would reject this attachment outright.
#[tokio::test]
async fn large_attachment_stages_through_the_object_backed_artifact_store() {

    const SIX_MIB: usize = 6 * 1024 * 1024;

    let root = tempfile::tempdir().expect("artifact root");
    let artifact_store: Arc<dyn ArtifactStore> = Arc::new(
        finstack_ai_store_artifact::LocalArtifactStore::try_new(root.path().to_path_buf())
            .expect("local artifact store"),
    );

    let (agent, _store, _model) =
        document_ingest_agent_with_store("large", Arc::clone(&artifact_store)).await;

    let artifact = artifact_store
        .stage_put(
            staging_scope(),
            Bytes::from(vec![0x25_u8; SIX_MIB]),
            ArtifactMetadata {
                kind: Arc::from("attachment"),
                media_type: Arc::from("application/pdf"),
                name: Some(Arc::from("large.pdf")),
                attributes: Metadata::empty(),
            },
        )
        .await
        .expect("staged artifact");
    assert_eq!(
        artifact.blob().length(),
        6_291_456,
        "staged ArtifactRef length must be exactly 6 MiB"
    );

    let mut request = AgentRunRequest::try_new(
        finstack_ai_runtime::ports::model::ModelName::try_new("lanes-1").expect("model name"),
        "Please summarize the attached document",
        security("decision-v1"),
    )
    .expect("request");
    request.attachments = Arc::from([AttachmentInput {
        artifact: artifact.clone(),
    }]);

    let output = agent.run(request).await.expect("run completes");
    assert_eq!(output.text(), "acknowledged");
}
