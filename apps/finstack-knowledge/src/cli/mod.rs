//! The `finstack-know` command-line interface.
//!
//! Argument parsing, command dispatch, and rendering live here; the agent
//! definition stays in the crate root. Renderers are pure functions over
//! events and results — the binary owns stdout/stderr, and `rich_rust`
//! supplies terminal formatting (plain-text export in tests, ANSI when
//! attached to a terminal).

pub mod args;
pub mod ask;
pub mod ingest;
pub mod render;
pub mod repl;
pub mod sessions;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai::{Agent, AgentRunRequest, Lane};
use finstack_ai_kernel::SessionId;
use rich_rust::console::Console;
use rich_rust::markup::escape;

use self::render::EventSink;
use crate::config::{compose_error, run_error};
use crate::{KnowledgeError, SELF_DOCS};

/// Open an existing session's `main` lane, or create a fresh session.
///
/// Every CLI command that runs turns shares this: `None` creates a session
/// (whose seeded `main` lane is opened), `Some(id)` opens it under the
/// `local` tenant scope. The `bool` reports whether this call created the
/// session, so the binary knows to print the new id.
///
/// # Errors
///
/// [`KnowledgeError::Config`] for an unparseable id and
/// [`KnowledgeError::Compose`] when the session cannot be created/opened.
pub async fn session_lane(
    journal: Arc<dyn JournalStore>,
    session: Option<&str>,
) -> Result<(finstack_ai::Session, finstack_ai::Lane, bool), KnowledgeError> {
    let (session, created) = match session {
        None => (
            finstack_ai::Session::create(journal, "local")
                .await
                .map_err(compose_error)?,
            true,
        ),
        Some(id) => {
            let id = SessionId::parse(id).map_err(|_| KnowledgeError::Config {
                reason: "session_id_invalid",
            })?;
            (
                finstack_ai::Session::open(journal, id, "local")
                    .await
                    .map_err(compose_error)?,
                false,
            )
        }
    };
    let lane = session.lane("main").await.map_err(compose_error)?;
    Ok((session, lane, created))
}

/// Run one request on `lane`, streaming every event batch and then the
/// final result text into `sink`.
///
/// # Errors
///
/// [`KnowledgeError::Run`] when the run cannot start, its stream breaks, or
/// it ends in failure.
pub(crate) async fn run_to_sink(
    lane: &Lane,
    agent: &Agent,
    request: AgentRunRequest,
    sink: &mut dyn EventSink,
) -> Result<(), KnowledgeError> {
    let run = lane.run(agent, request).map_err(run_error)?;
    while let Some(batch) = run.next_event_batch().await.map_err(run_error)? {
        sink.on_events(batch.events());
    }
    let output = run.result().await.map_err(run_error)?;
    sink.finish(&output.text());
    Ok(())
}

/// Render the `docs` command output: a topic listing, or one topic body.
///
/// Pure: returns the plain-text rendering (the binary decides whether to
/// re-render with ANSI for a terminal).
///
/// # Errors
///
/// Returns [`KnowledgeError::Config`] for an unknown topic.
pub fn docs_command(topic: Option<&str>) -> Result<String, KnowledgeError> {
    let console = Console::builder().markup(true).build();
    match topic {
        None => {
            use std::fmt::Write as _;
            let mut markup = String::from("[bold]Bundled self-doc topics[/bold]\n");
            for (name, body) in SELF_DOCS {
                let first_line = body
                    .lines()
                    .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
                    .unwrap_or_default();
                let _ = writeln!(
                    markup,
                    "  [cyan]{}[/cyan]  {}",
                    escape(name),
                    escape(first_line.trim())
                );
            }
            markup.push_str("\nUse [bold]finstack-know docs <topic>[/bold] to read one.");
            Ok(console.export_text(&markup))
        }
        Some(topic) => {
            let body = SELF_DOCS
                .iter()
                .find(|(name, _)| *name == topic)
                .map(|(_, body)| *body)
                .ok_or(KnowledgeError::Config {
                    reason: "docs_topic_unknown",
                })?;
            let markup = format!(
                "[bold underline]{}[/bold underline]\n{}",
                escape(topic),
                escape(body)
            );
            Ok(console.export_text(&markup))
        }
    }
}
