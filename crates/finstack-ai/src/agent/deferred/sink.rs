//! Notification-only child-run event sink.

use finstack_ai_kernel::{ChildRunLocator, EffectId, OperationLocator};
use finstack_ai_runtime::{EventBatch, PortFuture};

/// Context for one child-run event notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildEventContext {
    /// Parent run that owns the deferred effect.
    pub parent: OperationLocator,
    /// Child run emitting events.
    pub child: ChildRunLocator,
    /// Parent effect bound to the child.
    pub effect_id: EffectId,
}

/// Receives child-run event batches without gating the child.
///
/// Implementations must tolerate dropped progress. Errors and panics are
/// diagnostics only and must not fail the parent settlement.
pub trait ChildEventSink: Send + Sync {
    /// Observe one child event batch.
    fn on_batch(
        &self,
        context: &ChildEventContext,
        batch: &EventBatch,
    ) -> PortFuture<Result<(), ()>>;
}
