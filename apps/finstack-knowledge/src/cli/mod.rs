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

use rich_rust::console::Console;
use rich_rust::markup::escape;

use crate::{KnowledgeError, SELF_DOCS};

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
            let markup = format!("[bold underline]{}[/bold underline]\n{}", escape(topic),
                escape(body));
            Ok(console.export_text(&markup))
        }
    }
}
