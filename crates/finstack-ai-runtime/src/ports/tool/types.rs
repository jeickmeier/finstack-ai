use std::sync::Arc;

use finstack_ai_kernel::{
    ExternalHandleRef, Metadata, RawJson, ReconciliationPolicy, Timestamp, ToolBatchId, ToolCallId,
    ToolProgress, ValidatedToolCall,
};
use serde::{Deserialize, Serialize};

use crate::{PortStream, RunCallContext, UsageDelta};

use super::error::ToolError;

/// Immutable Toolset descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsetDescriptor {
    /// Stable Toolset name.
    pub name: Arc<str>,
    /// Bounded non-secret descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

/// Committed context for one direct tool call.
///
/// `run.effect_id` is the application-level idempotency key (FR-TLS-004).
/// [`Toolset::call`], [`Toolset::reconcile`], and same-identity retry all
/// receive this frozen identity.
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    /// Shared identity, authority, deadline, budget, and cancellation context.
    pub run: RunCallContext,
    /// Owning committed tool batch.
    pub tool_batch_id: ToolBatchId,
    /// Committed call identity.
    pub tool_call_id: ToolCallId,
}

/// Provider-neutral tool result before framework envelope injection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    /// Canonical application JSON output.
    pub output: RawJson,
    /// Whether the tool reports an application-level error result.
    pub is_error: bool,
}

/// Normalized Toolset stream item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStreamItem {
    /// Transient progress update.
    Progress(ToolProgress),
    /// Cumulative usage snapshot.
    Usage(UsageDelta),
    /// Exactly one terminal result.
    Completed(ToolResult),
}

/// Boxed target-correct tool event stream.
pub type ToolEventStream = PortStream<Result<ToolStreamItem, ToolError>>;

/// Minimal outstanding direct-tool projection for reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingToolEffect {
    /// Frozen executable call.
    pub call: ValidatedToolCall,
}

/// Terminal external deferral returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDeferral {
    /// External handle retaining the original effect identity.
    pub handle: ExternalHandleRef,
    /// Reconciliation policy.
    pub reconciliation: ReconciliationPolicy,
    /// Optional next poll time.
    pub next_poll_at: Option<Timestamp>,
    /// Optional expiry.
    pub expires_at: Option<Timestamp>,
}

/// Direct-tool reconciliation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolReconcileResult {
    /// Tool reports a completed normalized output.
    Completed(ToolResult),
    /// Tool reports externally deferred work.
    Deferred(ToolDeferral),
    /// Tool proves the effect never started.
    NotStarted,
    /// Tool reports the effect is still running.
    StillRunning(ToolDeferral),
    /// Frozen input may be retried safely.
    RetrySafe,
    /// Tool cannot classify the effect.
    Unknown,
    /// Tool reports non-repeatable uncertainty.
    NonRepeatable,
}

/// Journal-first recovery action for one outstanding tool effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolResumeAction {
    /// No matching outstanding tool effect remains.
    NoOutstanding,
    /// A recorded settlement already covers the effect.
    UseRecorded,
    /// Call the Toolset reconcile hook before dispatch.
    Reconcile,
    /// Re-dispatch the original committed call. First-pass never returns this.
    Retry,
    /// Wait for an external completion or later poll.
    WaitExternal,
    /// Do not call or fabricate a completion.
    SuspendUncertain,
}
