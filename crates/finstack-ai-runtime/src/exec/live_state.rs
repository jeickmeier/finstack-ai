//! Latest-only process-local run state published after confirmed boundaries.

use std::sync::Arc;

use finstack_ai_kernel::{
    ActiveCapability, Digest, KernelState, Message, OutputValidationFailed, PendingInteraction,
    RunPhase, TerminalState,
};

use crate::SessionHeadUpdate;
use crate::run_types::RunStatus;

/// Curated latest-only state for one live or completed run owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRunState {
    /// Process-local monotonic snapshot revision.
    pub revision: u64,
    /// Last confirmed journal sequence represented by this snapshot.
    pub journal_sequence: u64,
    /// Runtime owner lifecycle.
    pub status: RunStatus,
    /// Stable fault code when the runtime owner is faulted.
    pub fault_code: Option<Arc<str>>,
    /// Current semantic run phase.
    pub phase: Option<RunPhase>,
    /// Zero-based model cycle.
    pub cycle: u64,
    /// Current prepared model context, when available.
    pub prepared_context_messages: Arc<[Message]>,
    /// Durable messages committed by this run.
    pub committed_run_messages: Arc<[Message]>,
    /// Complete sorted active capability set.
    pub active_capabilities: Arc<[ActiveCapability]>,
    /// Digest of the resolved immutable run plan.
    pub resolved_plan_digest: Option<Digest>,
    /// Outstanding typed interaction.
    pub pending_interaction: Option<PendingInteraction>,
    /// Most recent structured-output validation failure.
    pub validation_failure: Option<OutputValidationFailed>,
    /// Durable semantic retry attempts consumed.
    pub retry_attempts: u32,
    /// Applied terminal outcome, when present.
    pub terminal: Option<TerminalState>,
}

impl LiveRunState {
    pub(crate) fn initial(state: &KernelState) -> Self {
        Self::from_kernel(0, RunStatus::Running, None, state)
    }

    pub(crate) fn next_semantic(
        previous_revision: u64,
        status: RunStatus,
        fault_code: Option<Arc<str>>,
        state: &KernelState,
    ) -> Self {
        Self::from_kernel(
            previous_revision.saturating_add(1),
            status,
            fault_code,
            state,
        )
    }

    pub(crate) fn next_lifecycle(&self, status: RunStatus) -> Self {
        let mut next = self.clone();
        next.revision = next.revision.saturating_add(1);
        next.status = status;
        next.fault_code = match status {
            RunStatus::Faulted { code } => Some(Arc::from(code)),
            _ => None,
        };
        next
    }

    fn from_kernel(
        revision: u64,
        status: RunStatus,
        fault_code: Option<Arc<str>>,
        state: &KernelState,
    ) -> Self {
        Self {
            revision,
            journal_sequence: state.last_applied_sequence,
            status,
            fault_code,
            phase: state.phase,
            cycle: state.cycle,
            prepared_context_messages: state
                .current_turn
                .as_ref()
                .map_or_else(|| Arc::from([]), |turn| Arc::clone(&turn.context.messages)),
            committed_run_messages: state.messages.as_slice().into(),
            active_capabilities: Arc::clone(&state.active_capabilities),
            resolved_plan_digest: state.resolved_plan_digest,
            pending_interaction: state.pending_interaction.clone(),
            validation_failure: state.validation_failure.clone(),
            retry_attempts: state.retry.attempts,
            terminal: state.terminal.clone(),
        }
    }
}

pub(crate) trait LiveStatePublisher: Send + Sync {
    fn publish_semantic(
        &self,
        state: &KernelState,
        session: &finstack_ai_kernel::SessionProjection,
        head_checksum: Option<Digest>,
        fault_code: Option<&'static str>,
        record_kinds: &[Arc<str>],
    );
}

pub(crate) fn session_head_update(
    state: &KernelState,
    session: &finstack_ai_kernel::SessionProjection,
    head_checksum: Option<Digest>,
) -> Option<SessionHeadUpdate> {
    Some(SessionHeadUpdate {
        session_id: session.session_id()?,
        last_applied_sequence: state.last_applied_sequence,
        head_checksum,
        projection: session.clone(),
    })
}
