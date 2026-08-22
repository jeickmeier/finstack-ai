//! Scan committed deferred tool calls.

use finstack_ai_kernel::{ActiveToolCallStatus, EffectDeferred, KernelState};

/// One deferred tool call recovered from committed run state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutstandingDeferral {
    /// Committed first-pass deferral under the original effect id.
    pub deferred: EffectDeferred,
}

/// Collect first-pass deferred tool calls still requested on `state`.
#[must_use]
pub fn outstanding_deferrals(state: &KernelState) -> Vec<OutstandingDeferral> {
    let Some(batch) = state.active_tool_batch() else {
        return Vec::new();
    };
    batch
        .calls
        .iter()
        .filter_map(|call| match &call.status {
            ActiveToolCallStatus::Requested {
                deferred: Some(deferred),
                ..
            } => Some(OutstandingDeferral {
                deferred: deferred.clone(),
            }),
            _ => None,
        })
        .collect()
}
