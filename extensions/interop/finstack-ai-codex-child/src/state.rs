//! In-memory run state reduced from Codex events and process exit.

use crate::events::{CodexEvent, CodexUsage};

/// Coarse child status surfaced through `codex_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexRunStatus {
    /// Process is still running.
    Running,
    /// Process exited zero without a reported failure.
    Completed,
    /// Process exited nonzero, or Codex reported `turn.failed` / `error`.
    Failed,
    /// A cancel was requested for this run; cancellation wins over exit.
    Cancelled,
}

/// Point-in-time status snapshot for one accepted child run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRunReport {
    /// Coarse status.
    pub status: CodexRunStatus,
    /// Codex thread id from `thread.started`, when seen.
    pub thread_id: Option<String>,
    /// Last `agent_message` text, when seen.
    pub last_message: Option<String>,
    /// Bounded failure diagnostic from `turn.failed` / `error`, when seen.
    pub failure_message: Option<String>,
    /// Observational token usage from `turn.completed`, when seen.
    pub usage: Option<CodexUsage>,
    /// Process exit code, when the process has exited.
    pub exit_code: Option<i32>,
    /// Last bytes the child wrote to stderr, on a failed run only.
    ///
    /// Bounded to roughly 4 KiB and truncated from the front, so a chatty
    /// child cannot grow the report without bound.
    pub stderr_tail: Option<String>,
}

/// Upper bound on retained stderr bytes per run.
const STDERR_TAIL_BYTES: usize = 4_096;
const THREAD_ID_BYTES: usize = 256;
const MESSAGE_BYTES: usize = 4_096;

/// Reduced view of one child's Codex event stream and process exit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunState {
    thread_id: Option<String>,
    last_message: Option<String>,
    usage: Option<CodexUsage>,
    failure: Option<String>,
    exit_code: Option<i32>,
    stderr: String,
    cancelled: bool,
    exited: bool,
}

impl RunState {
    pub(crate) fn apply(&mut self, event: CodexEvent) {
        match event {
            CodexEvent::ThreadStarted { thread_id } => {
                self.thread_id = Some(truncate(&thread_id, THREAD_ID_BYTES));
            }
            CodexEvent::AgentMessage { text } => {
                self.last_message = Some(truncate(&text, MESSAGE_BYTES));
            }
            CodexEvent::TurnCompleted { usage } => {
                if usage.is_some() {
                    self.usage = usage;
                }
            }
            CodexEvent::Failed { message } => {
                self.failure = Some(truncate(&message, MESSAGE_BYTES));
            }
            CodexEvent::Other => {}
        }
    }

    pub(crate) fn record_exit(&mut self, code: Option<i32>) {
        self.exited = true;
        self.exit_code = code;
    }

    /// Append one stderr line, keeping only the trailing window.
    ///
    /// The window is trimmed from the front and snapped forward to the
    /// next char boundary, so the retained tail is always valid UTF-8.
    pub(crate) fn append_stderr(&mut self, line: &str) {
        self.stderr.push_str(line);
        self.stderr.push('\n');
        if self.stderr.len() <= STDERR_TAIL_BYTES {
            return;
        }
        let mut start = self.stderr.len() - STDERR_TAIL_BYTES;
        while start < self.stderr.len() && !self.stderr.is_char_boundary(start) {
            start += 1;
        }
        let tail = self.stderr.get(start..).unwrap_or_default().to_string();
        self.stderr = tail;
    }

    /// Record a cancel request. Terminal runs are immutable: once the
    /// process exited, its recorded outcome is the truth, so a late cancel
    /// must not rewrite `Completed`/`Failed` into `Cancelled`.
    pub(crate) fn mark_cancelled(&mut self) {
        if self.exited {
            return;
        }
        self.cancelled = true;
    }

    pub(crate) fn report(&self) -> CodexRunReport {
        let status = if self.cancelled {
            CodexRunStatus::Cancelled
        } else if !self.exited {
            CodexRunStatus::Running
        } else if self.failure.is_some() || self.exit_code != Some(0) {
            CodexRunStatus::Failed
        } else {
            CodexRunStatus::Completed
        };
        let stderr_tail = if status == CodexRunStatus::Failed && !self.stderr.trim().is_empty() {
            Some(self.stderr.clone())
        } else {
            None
        };
        CodexRunReport {
            status,
            thread_id: self.thread_id.clone(),
            last_message: self.last_message.clone(),
            failure_message: self.failure.clone(),
            usage: self.usage,
            exit_code: self.exit_code,
            stderr_tail,
        }
    }
}

fn truncate(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.get(..end).unwrap_or_default().to_owned()
}
