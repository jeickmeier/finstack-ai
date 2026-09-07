//! Notification-only child-run event sink.

use finstack_ai_kernel::{ChildRunLocator, EffectId, OperationLocator};
use finstack_ai_runtime::events::EventBatch;
use finstack_ai_runtime::ports::PortFuture;

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
/// Delivery is best effort: at most one callback runs per settlement, with a
/// one-second deadline and no queued batches. Notifications arriving while it
/// is busy are dropped. The callback is dropped when event pumping finishes or
/// its settlement future is cancelled. Errors and panics never fail the parent.
/// Implementations must poll cooperatively and tolerate dropped notifications.
pub trait ChildEventSink: Send + Sync {
    /// Observe one child event batch.
    fn on_batch(
        &self,
        context: &ChildEventContext,
        batch: &EventBatch,
    ) -> PortFuture<Result<(), ()>>;
}
