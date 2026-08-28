//! The `ingest` command: attach a document to a run and let the document
//! pipeline and memory extension do the rest.
//!
//! The file is staged into the shared artifact store and attached to a run
//! whose instruction asks for a summary plus memory capture; the
//! `DocumentIngestMiddleware` converts it to model-visible Markdown, and
//! the `remember` tool / capture observer persist the key facts.

use std::path::Path;

use finstack_ai::runtime::artifact::{ArtifactMetadata, stage_required_artifact};
use finstack_ai::runtime::artifact::ArtifactScope;
use finstack_ai::{AgentRunRequest, AttachmentInput};
use finstack_ai_kernel::{Metadata, Sensitivity, SessionId};

use super::ask::AskOutcome;
use super::render::EventSink;
use crate::{
    KnowledgeConfig, KnowledgeError, build_agent_with_stores, model_name, open_artifact_store,
    open_journal, security,
};

/// Ceiling on ingested file size; the document parser's own input ceiling
/// is 4 MiB, so anything larger can only fail later.
const MAX_INGEST_BYTES: u64 = 4 * 1024 * 1024;

/// Instruction sent with the attachment.
const INGEST_INSTRUCTION: &str = "A document is attached. Summarize it in one paragraph, \
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
    let agent =
        build_agent_with_stores(config, journal.clone(), artifact_store.clone()).await?;

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
        scope,
        bytes.into(),
        ArtifactMetadata {
            kind: std::sync::Arc::from("attachment"),
            media_type: std::sync::Arc::from(media_type),
            name: Some(std::sync::Arc::from(name.as_str())),
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(compose)?;

    let (session, lane, created) = match session {
        None => {
            let session = finstack_ai::Session::create(journal, "local")
                .await
                .map_err(compose)?;
            let lane = session.lane("main").await.map_err(compose)?;
            (session, lane, true)
        }
        Some(id) => {
            let id = SessionId::parse(id).map_err(|_| KnowledgeError::Config {
                reason: "session_id_invalid",
            })?;
            let session = finstack_ai::Session::open(journal, id, "local")
                .await
                .map_err(compose)?;
            let lane = session.lane("main").await.map_err(compose)?;
            (session, lane, false)
        }
    };

    let mut request =
        AgentRunRequest::try_new(model_name(config)?, INGEST_INSTRUCTION, security(os_user)?)
            .map_err(compose)?;
    request.attachments = std::sync::Arc::from([AttachmentInput { artifact }]);

    let run = lane.run(&agent, request).map_err(run_error)?;
    while let Some(batch) = run.next_event_batch().await.map_err(run_error)? {
        sink.on_events(batch.events());
    }
    let output = run.result().await.map_err(run_error)?;
    sink.finish(&output.text());

    Ok(AskOutcome {
        session_id: session.session_id().to_string(),
        created,
    })
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, KnowledgeError> {
    let metadata = std::fs::metadata(path).map_err(|_| KnowledgeError::Config {
        reason: "ingest_file_unreadable",
    })?;
    if !metadata.is_file() || metadata.len() > MAX_INGEST_BYTES {
        return Err(KnowledgeError::Config {
            reason: "ingest_file_invalid_or_oversized",
        });
    }
    std::fs::read(path).map_err(|_| KnowledgeError::Config {
        reason: "ingest_file_unreadable",
    })
}

/// Media type from the file extension; the parser sniffs content anyway,
/// this is only a hint.
fn media_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("docx") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
        Some("csv") => "text/csv",
        Some("md" | "markdown") => "text/markdown",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

fn compose(error: impl std::fmt::Display) -> KnowledgeError {
    KnowledgeError::Compose {
        reason: error.to_string(),
    }
}

fn run_error(error: impl std::fmt::Display) -> KnowledgeError {
    KnowledgeError::Run {
        reason: error.to_string(),
    }
}
