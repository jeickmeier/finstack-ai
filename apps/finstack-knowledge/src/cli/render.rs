//! Event renderers: human text (rich markup) and NDJSON.
//!
//! Both renderers are pure accumulators over the same `RunEvent` stream —
//! the events-are-the-contract proof. They never touch the journal, and
//! the binary owns actual stdout: it renders the accumulated markup with
//! ANSI on a terminal, while tests export it as plain text.

use std::fmt::Write as _;

use finstack_ai_kernel::{RunEvent, RunEventBody};
use rich_rust::console::Console;
use rich_rust::markup::escape;

/// A sink for run events plus the final result.
pub trait EventSink {
    /// Consume the next slice of events, in stream order.
    fn on_events(&mut self, events: &[RunEvent]);
    /// Consume the final result text after the stream closes.
    fn finish(&mut self, result_text: &str);
    /// Total events consumed so far.
    fn events_seen(&self) -> u64;
    /// Yield the accumulated output (markup for the text renderer, NDJSON
    /// for the json renderer).
    fn into_markup(self) -> String
    where
        Self: Sized;
}

/// Render accumulated rich markup to plain text (no ANSI).
#[must_use]
pub fn render_markup_plain(markup: &str) -> String {
    Console::builder()
        .markup(true)
        .build()
        .export_text(markup)
}

/// Human text renderer: deltas inline, tool lines, a completion line.
#[derive(Debug, Default)]
pub struct TextRenderer {
    markup: String,
    events: u64,
    saw_delta: bool,
}

impl TextRenderer {
    /// Empty renderer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventSink for TextRenderer {
    fn on_events(&mut self, events: &[RunEvent]) {
        for event in events {
            self.events += 1;
            match event.body() {
                RunEventBody::ModelTextDelta(delta) => {
                    self.saw_delta = true;
                    self.markup.push_str(&escape(delta.text()));
                }
                RunEventBody::ToolSettled { tool_call_id, .. } => {
                    let _ = writeln!(
                        self.markup,
                        "\n[dim]tool settled: {}[/dim]",
                        escape(&tool_call_id.to_string())
                    );
                }
                RunEventBody::InteractionRequested(_) => {
                    let _ = writeln!(
                        self.markup,
                        "\n[yellow]interaction requested — answer with the repl[/yellow]"
                    );
                }
                RunEventBody::RunFailed { error } => {
                    let _ = writeln!(
                        self.markup,
                        "\n[red]run failed: {}[/red]",
                        escape(&format!("{error:?}"))
                    );
                }
                RunEventBody::RunCancelled { .. } => {
                    let _ = writeln!(self.markup, "\n[red]run cancelled[/red]");
                }
                RunEventBody::RunCompleted { .. } => {
                    let _ = writeln!(self.markup, "\n[green]run completed[/green]");
                }
                _ => {}
            }
        }
    }

    fn finish(&mut self, result_text: &str) {
        if !self.saw_delta && !result_text.is_empty() {
            let _ = writeln!(self.markup, "{}", escape(result_text));
        }
    }

    fn events_seen(&self) -> u64 {
        self.events
    }

    fn into_markup(self) -> String {
        self.markup
    }
}

/// NDJSON renderer: one JSON object per event, kind names verbatim, no
/// reordering, nothing added.
#[derive(Debug, Default)]
pub struct JsonRenderer {
    lines: String,
    events: u64,
}

impl JsonRenderer {
    /// Empty renderer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventSink for JsonRenderer {
    fn on_events(&mut self, events: &[RunEvent]) {
        for event in events {
            self.events += 1;
            if let Ok(line) = serde_json::to_string(event) {
                self.lines.push_str(&line);
                self.lines.push('\n');
            }
        }
    }

    fn finish(&mut self, _result_text: &str) {}

    fn events_seen(&self) -> u64 {
        self.events
    }

    fn into_markup(self) -> String {
        self.lines
    }
}
