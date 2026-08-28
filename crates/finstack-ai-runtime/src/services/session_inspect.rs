//! Replay-derived session inspection shared by native and browser bindings.

use std::sync::Arc;

use finstack_ai_kernel::{ContentBlock, ErrorCode, SessionId, TerminalState};

use crate::commit::CommitCoordinator;
use crate::ports::journal::{JournalStore, LoadRequest};
use crate::session::SessionError;

/// Replay-derived lifecycle phase for a stored session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionInspectPhase {
    /// The store has no committed records for the session.
    Empty,
    /// The session has committed work without a terminal record.
    InProgress,
    /// The session completed successfully.
    Completed,
    /// The session failed terminally.
    Failed,
    /// The session was cancelled terminally.
    Cancelled,
}

impl SessionInspectPhase {
    /// Stable binding-neutral phase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Canonical replay projection used by binding inspection APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInspectSnapshot {
    /// Stored session identity.
    pub session_id: SessionId,
    /// Confirmed journal head sequence.
    pub head_sequence: u64,
    /// Lifecycle phase reconstructed from committed records.
    pub phase: SessionInspectPhase,
    /// Concatenated terminal assistant text when the session completed.
    pub result_text: Option<String>,
    /// Stable kind name of the final committed record.
    pub last_record_kind: Option<String>,
}

/// Inspect one stored session through canonical recovery semantics.
///
/// This function never resumes or spawns work. Bindings should convert the
/// returned snapshot without deriving lifecycle meaning themselves.
///
/// # Errors
///
/// Returns [`SessionError::Recover`] when the journal cannot be loaded or
/// replayed.
pub async fn inspect_session(
    store: Arc<dyn JournalStore>,
    session_id: SessionId,
) -> Result<SessionInspectSnapshot, SessionError> {
    let loaded = store
        .load(LoadRequest { session_id })
        .await
        .map_err(|error| recover_error(error.code()))?;
    if loaded.head_sequence == 0 && loaded.committed_batches.is_empty() {
        return Ok(SessionInspectSnapshot {
            session_id,
            head_sequence: 0,
            phase: SessionInspectPhase::Empty,
            result_text: None,
            last_record_kind: None,
        });
    }

    let recovered = CommitCoordinator::recover(Arc::clone(&store), session_id)
        .await
        .map_err(|error| recover_error(error.code()))?;
    let state = recovered.state();
    let phase = match state.terminal() {
        Some(TerminalState::Completed(_)) => SessionInspectPhase::Completed,
        Some(TerminalState::Failed(_)) => SessionInspectPhase::Failed,
        Some(TerminalState::Cancelled(_)) => SessionInspectPhase::Cancelled,
        None => SessionInspectPhase::InProgress,
    };
    let result_text = match state.terminal() {
        Some(TerminalState::Completed(completed)) => state
            .messages()
            .iter()
            .find(|message| message.id() == &completed.result_message_id)
            .map(message_text),
        _ => None,
    };
    let last_record_kind = loaded
        .committed_batches
        .iter()
        .rev()
        .flat_map(|batch| batch.records.iter().rev())
        .next()
        .map(|record| record.body().kind_name().to_owned());

    Ok(SessionInspectSnapshot {
        session_id,
        head_sequence: loaded.head_sequence,
        phase,
        result_text,
        last_record_kind,
    })
}

fn recover_error(code: &'static str) -> SessionError {
    SessionError::Recover {
        code: ErrorCode::new(code)
            .unwrap_or_else(|_| finstack_ai_kernel::static_error_code!("session_recover_failed")),
    }
}

fn message_text(message: &finstack_ai_kernel::Message) -> String {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect()
}
