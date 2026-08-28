//! The `ask` command: one question on a new or existing session.

use finstack_ai::AgentRunRequest;
use super::render::EventSink;
use crate::{KnowledgeConfig, KnowledgeError, build_agent_with_journal, model_name, open_journal, security};

/// What `ask` did, for the binary to report.
#[derive(Debug)]
pub struct AskOutcome {
    /// The session the question ran on.
    pub session_id: String,
    /// Whether this call created the session.
    pub created: bool,
}

/// Ask one question, streaming events into `sink`.
///
/// Without a session id a new session is created (the caller prints its
/// id); with one, the question continues that session on lane `main`.
/// Returned boxed so callers hold a small future (the composed agent's
/// state is large).
///
/// # Errors
///
/// [`KnowledgeError::Config`] for an unparseable session id,
/// [`KnowledgeError::Compose`] for composition failures, and
/// [`KnowledgeError::Run`] when the run itself fails (CLI exit code 1).
pub fn run_ask<'a>(
    config: &'a KnowledgeConfig,
    question: &'a str,
    session: Option<&'a str>,
    os_user: &'a str,
    sink: &'a mut dyn EventSink,
) -> std::pin::Pin<Box<dyn Future<Output = Result<AskOutcome, KnowledgeError>> + 'a>> {
    Box::pin(run_ask_inner(config, question, session, os_user, sink))
}

async fn run_ask_inner(
    config: &KnowledgeConfig,
    question: &str,
    session: Option<&str>,
    os_user: &str,
    sink: &mut dyn EventSink,
) -> Result<AskOutcome, KnowledgeError> {
    let journal = open_journal(config)?;
    let agent = build_agent_with_journal(config, journal.clone()).await?;
    let (session, lane, created) = super::session_lane(journal, session).await?;

    let request = AgentRunRequest::try_new(model_name(config)?, question, security(os_user)?)
        .map_err(compose)?;
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
