//! Temporal-shaped mapping over the runtime workflow-driver contract.
//!
//! This crate does not depend on `temporalio` or a cluster. The in-process
//! reference driver is [`finstack_ai_runtime::LocalWorkflowDriver`]. It does
//! not plan model or tool batches.

#![warn(missing_docs)]

use finstack_ai_runtime::{
    EffectId, KernelState, ModelCapabilities, OperationLocator, RunId, ToolSpec,
    WorkflowRetryDecision, WorkflowSession, WorkflowWait, retry_decision,
};

/// Temporal-shaped worker that drives an existing [`WorkflowSession`].
pub struct TemporalShapedWorker {
    session: WorkflowSession,
    activity_retry_policy_attempts: u32,
    activity_dispatches: u32,
}

impl TemporalShapedWorker {
    /// Wrap a recovered session. `activity_retry_policy_attempts` is engine
    /// intent only; kernel [`retry_decision`] remains authoritative.
    #[must_use]
    pub const fn wrap(session: WorkflowSession, activity_retry_policy_attempts: u32) -> Self {
        Self {
            session,
            activity_retry_policy_attempts,
            activity_dispatches: 0,
        }
    }

    /// Map a Temporal-shaped workflow / run ID onto the kernel [`RunId`].
    #[must_use]
    pub fn workflow_run_id(locator: &OperationLocator) -> RunId {
        locator.run_id
    }

    /// Map a Temporal-shaped activity ID onto the original [`EffectId`].
    #[must_use]
    pub const fn activity_id(effect_id: EffectId) -> EffectId {
        effect_id
    }

    /// Engine-declared activity retry ceiling. Not a kernel limit.
    #[must_use]
    pub const fn activity_retry_policy_attempts(&self) -> u32 {
        self.activity_retry_policy_attempts
    }

    /// Count of kernel-allowed activity re-dispatches observed by this worker.
    #[must_use]
    pub const fn activity_dispatches(&self) -> u32 {
        self.activity_dispatches
    }

    /// Drive existing post-commit actions until a wait or terminal.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`WorkflowSession::drive_until_wait`].
    pub async fn drive_until_wait(
        &mut self,
    ) -> Result<WorkflowWait, finstack_ai_runtime::WorkflowDriverError> {
        self.session.drive_until_wait().await
    }

    /// Ask whether one activity may be re-dispatched.
    ///
    /// A [`WorkflowRetryDecision::Deny`] does not increment
    /// [`Self::activity_dispatches`] and must not enqueue `ExecuteEffect`.
    #[must_use]
    pub fn consider_activity_retry(
        &mut self,
        effect_id: EffectId,
        model: Option<&ModelCapabilities>,
        tool: Option<&ToolSpec>,
    ) -> WorkflowRetryDecision {
        apply_activity_retry_policy(
            self.session.last_state(),
            effect_id,
            self.activity_retry_policy_attempts,
            model,
            tool,
            &mut self.activity_dispatches,
        )
    }

    /// Persistence handoff hint after a wait-producing commit.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`WorkflowSession::persist_handoff`].
    pub fn persist_handoff(
        &self,
    ) -> Result<finstack_ai_runtime::WorkflowCheckpoint, finstack_ai_runtime::WorkflowDriverError>
    {
        self.session.persist_handoff()
    }

    /// Borrow the inner driver.
    #[must_use]
    pub const fn session(&self) -> &WorkflowSession {
        &self.session
    }

    /// Borrow the inner driver mutably.
    #[must_use]
    pub const fn session_mut(&mut self) -> &mut WorkflowSession {
        &mut self.session
    }
}

/// Map Temporal-shaped activity retry intent onto kernel [`retry_decision`].
///
/// `policy_attempts` is recorded as engine intent only. Dispatch counting
/// increments solely on [`WorkflowRetryDecision::Allow`].
///
/// # Examples
///
/// ```
/// use finstack_ai_runtime::{EffectId, KernelState, WorkflowRetryDecision};
/// use finstack_ai_workflow_temporal::apply_activity_retry_policy;
///
/// let state = KernelState::default();
/// let effect_id = EffectId::from_bytes([
///     0, 0, 0, 0, 0, 0, 0x70, 0, 0x80, 0, 0, 0, 0, 0, 0, 1,
/// ]);
/// let mut dispatches = 0;
/// let decision = apply_activity_retry_policy(&state, effect_id, 5, None, None, &mut dispatches);
/// assert_eq!(dispatches, 0);
/// assert_eq!(
///     decision,
///     WorkflowRetryDecision::Deny {
///         code: "effect_not_outstanding",
///     }
/// );
/// ```
#[must_use]
pub fn apply_activity_retry_policy(
    state: &KernelState,
    effect_id: EffectId,
    policy_attempts: u32,
    model: Option<&ModelCapabilities>,
    tool: Option<&ToolSpec>,
    dispatch_count: &mut u32,
) -> WorkflowRetryDecision {
    let _ = policy_attempts;
    let decision = retry_decision(state, effect_id, model, tool);
    if matches!(decision, WorkflowRetryDecision::Allow { .. }) {
        *dispatch_count = dispatch_count.saturating_add(1);
    }
    decision
}
