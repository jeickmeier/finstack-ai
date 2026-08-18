//! Types for scanning committed deferred tool calls.

use finstack_ai_kernel::EffectDeferred;

/// One deferred tool call recovered from committed run state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutstandingDeferral {
    /// Committed first-pass deferral under the original effect id.
    pub deferred: EffectDeferred,
}
