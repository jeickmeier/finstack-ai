//! Tolerant parser for `codex exec --json` stdout lines.
//!
//! Unknown event types and malformed lines never fail a run; they parse to
//! [`CodexEvent::Other`]. Shapes were verified against the Codex CLI JSONL
//! contract; only the fields consumed here are assumed.

use serde::Serialize;

/// Token usage reported by `turn.completed`. Observational only (spec D8);
/// never charged against finstack budget in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct CodexUsage {
    /// Prompt tokens consumed by the turn.
    pub input_tokens: u64,
    /// Cached prompt tokens within `input_tokens`.
    pub cached_input_tokens: u64,
    /// Completion tokens produced by the turn.
    pub output_tokens: u64,
}

// Consumed by the run-state reducer (`crate::state`) and the process
// supervisor that streams `codex exec --json` stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexEvent {
    ThreadStarted { thread_id: String },
    AgentMessage { text: String },
    TurnCompleted { usage: Option<CodexUsage> },
    Failed { message: String },
    Other,
}

pub(crate) fn parse_event(line: &str) -> CodexEvent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return CodexEvent::Other;
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("thread.started") => value
            .get("thread_id")
            .and_then(serde_json::Value::as_str)
            .map_or(CodexEvent::Other, |thread_id| CodexEvent::ThreadStarted {
                thread_id: thread_id.to_string(),
            }),
        Some("item.completed") => {
            let item = value.get("item");
            let is_message = item
                .and_then(|item| item.get("type"))
                .and_then(serde_json::Value::as_str)
                == Some("agent_message");
            let text = item
                .and_then(|item| item.get("text"))
                .and_then(serde_json::Value::as_str);
            match (is_message, text) {
                (true, Some(text)) => CodexEvent::AgentMessage {
                    text: text.to_string(),
                },
                _ => CodexEvent::Other,
            }
        }
        Some("turn.completed") => CodexEvent::TurnCompleted {
            usage: value.get("usage").map(|usage| CodexUsage {
                input_tokens: field_u64(usage, "input_tokens"),
                cached_input_tokens: field_u64(usage, "cached_input_tokens"),
                output_tokens: field_u64(usage, "output_tokens"),
            }),
        },
        Some("turn.failed") => CodexEvent::Failed {
            message: value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("codex turn failed")
                .to_string(),
        },
        Some("error") => CodexEvent::Failed {
            message: value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("codex reported an error")
                .to_string(),
        },
        _ => CodexEvent::Other,
    }
}

fn field_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}
