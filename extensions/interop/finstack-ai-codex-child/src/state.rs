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
    /// Observational token usage from `turn.completed`, when seen.
    pub usage: Option<CodexUsage>,
    /// Process exit code, when the process has exited.
    pub exit_code: Option<i32>,
}

/// Reduced view of one child's Codex event stream and process exit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunState {
    thread_id: Option<String>,
    last_message: Option<String>,
    usage: Option<CodexUsage>,
    failure: Option<String>,
    exit_code: Option<i32>,
    cancelled: bool,
    exited: bool,
}

impl RunState {
    pub(crate) fn apply(&mut self, event: CodexEvent) {
        match event {
            CodexEvent::ThreadStarted { thread_id } => self.thread_id = Some(thread_id),
            CodexEvent::AgentMessage { text } => self.last_message = Some(text),
            CodexEvent::TurnCompleted { usage } => {
                if usage.is_some() {
                    self.usage = usage;
                }
            }
            CodexEvent::Failed { message } => self.failure = Some(message),
            CodexEvent::Other => {}
        }
    }

    pub(crate) fn record_exit(&mut self, code: Option<i32>) {
        self.exited = true;
        self.exit_code = code;
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "consumed by cancel in the following task")
    )]
    pub(crate) fn mark_cancelled(&mut self) {
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
        CodexRunReport {
            status,
            thread_id: self.thread_id.clone(),
            last_message: self.last_message.clone(),
            usage: self.usage,
            exit_code: self.exit_code,
        }
    }
}
