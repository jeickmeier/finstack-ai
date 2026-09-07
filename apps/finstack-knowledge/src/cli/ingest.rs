//! The `ingest` command: attach a document to a run and let the document
//! pipeline and memory extension do the rest.
//!
//! The file is staged into the shared artifact store and attached to a run
//! whose instruction asks for a summary plus memory capture; the
//! `DocumentIngestMiddleware` converts it to model-visible Markdown, and
//! the `remember` tool / capture observer persist the key facts.

use std::path::Path;

use finstack_ai::runtime::artifact::ArtifactScope;
use finstack_ai::runtime::artifact::{ArtifactMetadata, stage_required_artifact};
use finstack_ai::{AgentRunRequest, AttachmentInput};
use finstack_ai_kernel::{Metadata, Sensitivity, SessionId};
use finstack_ai_tools_document::parser::{DocumentFormat, DocumentLimits};

use super::ask::AskOutcome;
use super::render::EventSink;
use crate::config::compose_error;
use crate::{
    KnowledgeConfig, KnowledgeError, build_agent_with_stores, model_name, open_artifact_store,
    open_journal, security,
};

/// Instruction sent with the attachment.
const INGEST_INSTRUCTION: &str = "A document is attached. First call index_document with the attachment reference shown with the converted document, then report its extraction status. Summarize it in one paragraph, \
then use the remember tool to store its key facts (names, figures, dates) with useful \
keywords, citing the document name.";

/// Ingest one document into a new or existing session.
///
/// Returned boxed for the same reason as `run_ask` (large composed state).
///
/// # Errors
///
/// [`KnowledgeError::Config`] for a missing/oversized file or bad session
/// id, [`KnowledgeError::Compose`] for composition failures, and
/// [`KnowledgeError::Run`] when the run fails.
pub fn run_ingest<'a>(
    config: &'a KnowledgeConfig,
    path: &'a Path,
    session: Option<&'a str>,
    os_user: &'a str,
    sink: &'a mut dyn EventSink,
) -> std::pin::Pin<Box<dyn Future<Output = Result<AskOutcome, KnowledgeError>> + 'a>> {
    Box::pin(run_ingest_inner(config, path, session, os_user, sink))
}

async fn run_ingest_inner(
    config: &KnowledgeConfig,
    path: &Path,
    session: Option<&str>,
    os_user: &str,
    sink: &mut dyn EventSink,
) -> Result<AskOutcome, KnowledgeError> {
    let bytes = read_bounded(path)?;
    let name = path.file_name().map_or_else(
        || "document".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let media_type = media_type_for(path);

    let journal = open_journal(config)?;
    let artifact_store = open_artifact_store(config)?;
    let (session, lane, created) = super::session_lane(journal.clone(), session).await?;
    let config = crate::local_search_config(config).await?;
    let mut composition = build_agent_with_stores(&config, journal, artifact_store.clone()).await?;

    // Stage into the same tenant-bound pre-run scope the ingest middleware
    // resolves (pattern of the python binding's attachment staging).
    let scope = ArtifactScope {
        tenant_scope: std::sync::Arc::from("local"),
        session_id: SessionId::from_bytes([0_u8; 16]),
        run_id: None,
        sensitivity: Sensitivity::Internal,
    };
    let artifact = stage_required_artifact(
        artifact_store.as_ref(),
        scope.clone(),
        bytes.into(),
        ArtifactMetadata {
            kind: std::sync::Arc::from("attachment"),
            media_type: std::sync::Arc::from(media_type),
            name: Some(std::sync::Arc::from(name.as_str())),
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(compose_error)?;

    let mut request =
        AgentRunRequest::try_new(model_name(&config)?, INGEST_INSTRUCTION, security(os_user)?)
            .map_err(compose_error)?;
    request.attachments = std::sync::Arc::from([AttachmentInput {
        artifact: artifact.clone(),
    }]);
    let result = super::run_to_sink(&lane, &composition.agent, request, sink).await;
    let maintenance = composition.search.maintain(256).await?;
    result?;
    if composition
        .search
        .documents
        .indexed_document(finstack_ai_index_documents::DocumentInput {
            artifact_scope: scope,
            artifact,
        })
        .await
        .map_err(compose_error)?
        .is_none()
    {
        return Err(KnowledgeError::Run {
            reason: "document_index_effect_not_completed".into(),
        });
    }

    Ok(AskOutcome {
        session_id: session.session_id().to_string(),
        created,
        maintenance: vec![composition.initial_maintenance, maintenance],
    })
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, KnowledgeError> {
    // The size ceiling is the document parser's own input limit; the app
    // adds no policy of its own here.
    let max_bytes = DocumentLimits::default().max_input_bytes;
    let metadata = std::fs::metadata(path).map_err(|_| KnowledgeError::Config {
        reason: "ingest_file_unreadable",
    })?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(KnowledgeError::Config {
            reason: "ingest_file_invalid_or_oversized",
        });
    }
    std::fs::read(path).map_err(|_| KnowledgeError::Config {
        reason: "ingest_file_unreadable",
    })
}

/// Media type from the file extension via the document toolset's own
/// mapping; content sniffing still decides the real format at parse time.
fn media_type_for(path: &Path) -> &'static str {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(DocumentFormat::media_type_for_extension)
        .unwrap_or("application/octet-stream")
}
